//! Machine à états *pure* du passage de bord (edge-switching), côté contrôleur.
//!
//! Modèle « serveur = contrôleur » : le serveur possède le clavier/souris physique
//! et un curseur *virtuel* qui se déplace dans l'espace de l'écran actif. Quand
//! le curseur réel atteint un bord de l'écran serveur (mode local), ou quand le
//! curseur virtuel franchit un bord de l'écran distant actif, on bascule vers le
//! voisin défini par la disposition.
//!
//! Tout ici est sans effet de bord et déterministe : l'orchestrateur traduit les
//! [`MoveOutcome`] en messages réseau et en warps de curseur.

use nomad_core::layout::{Layout, Screen, Side};
use nomad_core::NodeId;

/// Convertit un point en pixels locaux (`0..dim`) vers un ratio `0..1` de
/// l'écran. Même convention que le reste du protocole (le client remultiplie).
fn to_ratio((px, py): (f64, f64), s: Screen) -> (f64, f64) {
    (px / s.width as f64, py / s.height as f64)
}

/// Résultat d'un mouvement traité par le contrôleur.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MoveOutcome {
    /// Si une transition de nœud actif a eu lieu : ratio d'entrée (0..=1) sur le
    /// *nouvel* écran actif (qu'il soit local ou distant).
    pub entry: Option<(f64, f64)>,
    /// Position absolue (ratios 0..=1) à appliquer sur le nœud distant actif,
    /// lorsqu'on reste sur le même écran distant.
    pub remote_abs: Option<(f64, f64)>,
}

impl MoveOutcome {
    fn none() -> Self {
        Self::default()
    }
}

/// Contrôleur d'edge-switching.
pub struct EdgeController {
    self_id: NodeId,
    self_screen: Screen,
    layout: Layout,
    active: NodeId,
    /// Position du curseur virtuel, en pixels, dans l'espace de l'écran actif
    /// (significative uniquement quand `active != self_id`).
    virtual_pos: (f64, f64),
    /// Bord de l'écran **local** par lequel le contrôle est parti en mode
    /// distant (`None` en local). Stable pendant toute la session distante,
    /// y compris lors des sauts distant→distant : le curseur réel du serveur
    /// reste collé à ce bord pour indiquer la position sur l'écran distant.
    exit_side: Option<Side>,
}

impl EdgeController {
    pub fn new(self_id: NodeId, self_screen: Screen, layout: Layout) -> Self {
        Self {
            self_id,
            self_screen,
            layout,
            active: self_id,
            virtual_pos: (0.0, 0.0),
            exit_side: None,
        }
    }

    pub fn active(&self) -> NodeId {
        self.active
    }

    pub fn is_local(&self) -> bool {
        self.active == self.self_id
    }

    /// Bord de l'écran local par lequel le contrôle est parti (`None` en local).
    pub fn exit_side(&self) -> Option<Side> {
        self.exit_side
    }

    /// Remplace la disposition. Si le nœud actif disparaît, on revient en local.
    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
        if self.active != self.self_id && self.layout.node(self.active).is_none() {
            self.active = self.self_id;
            self.exit_side = None;
        }
    }

    fn screen_of(&self, id: NodeId) -> Screen {
        if id == self.self_id {
            self.self_screen
        } else {
            self.layout
                .node(id)
                .map(|n| n.screen)
                .unwrap_or(self.self_screen)
        }
    }

    /// Origine (coin haut-gauche) d'un écran dans le plan virtuel.
    fn origin_of(&self, id: NodeId) -> (f64, f64) {
        self.layout
            .positions
            .get(&id)
            .map(|&(x, y)| (x as f64, y as f64))
            .unwrap_or((0.0, 0.0))
    }

    /// Mouvement en **mode local** : `(x, y)` est la position absolue réelle du
    /// curseur sur l'écran serveur. Détecte une sortie par un bord adjacent.
    pub fn local_move(&mut self, x: f64, y: f64) -> MoveOutcome {
        let w = self.self_screen.width as f64;
        let h = self.self_screen.height as f64;

        let side = if x >= w - 1.0 {
            Side::Right
        } else if x <= 0.0 {
            Side::Left
        } else if y <= 0.0 {
            Side::Top
        } else if y >= h - 1.0 {
            Side::Bottom
        } else {
            return MoveOutcome::none();
        };

        // Coordonnée du plan le long du bord de sortie.
        let (ox, oy) = self.origin_of(self.self_id);
        let coord = if side.is_horizontal() { oy + y } else { ox + x };

        let Some((neighbor, entry_local)) = self.layout.neighbor_at(self.self_id, side, coord)
        else {
            return MoveOutcome::none();
        };

        let ns = self.screen_of(neighbor);
        self.active = neighbor;
        self.exit_side = Some(side);
        self.virtual_pos = entry_local;
        MoveOutcome {
            entry: Some(to_ratio(entry_local, ns)),
            remote_abs: None,
        }
    }

    /// Mouvement en **mode distant** : `(dx, dy)` est le déplacement relatif.
    /// Avance le curseur virtuel et gère les franchissements de bord.
    pub fn remote_advance(&mut self, dx: f64, dy: f64) -> MoveOutcome {
        self.virtual_pos.0 += dx;
        self.virtual_pos.1 += dy;

        let s = self.screen_of(self.active);
        let w = s.width as f64;
        let h = s.height as f64;
        let (vx, vy) = self.virtual_pos;

        let side = if vx < 0.0 {
            Some(Side::Left)
        } else if vx > w {
            Some(Side::Right)
        } else if vy < 0.0 {
            Some(Side::Top)
        } else if vy > h {
            Some(Side::Bottom)
        } else {
            None
        };

        let Some(side) = side else {
            // Pas de franchissement : on transmet la position absolue.
            return MoveOutcome {
                entry: None,
                remote_abs: Some(((vx / w).clamp(0.0, 1.0), (vy / h).clamp(0.0, 1.0))),
            };
        };

        // Coordonnée du plan le long du bord franchi.
        let (ox, oy) = self.origin_of(self.active);
        let coord = if side.is_horizontal() { oy + vy } else { ox + vx };

        match self.layout.neighbor_at(self.active, side, coord) {
            None => {
                // Bord du monde : on reste collé au bord.
                self.virtual_pos = (vx.clamp(0.0, w), vy.clamp(0.0, h));
                MoveOutcome {
                    entry: None,
                    remote_abs: Some((self.virtual_pos.0 / w, self.virtual_pos.1 / h)),
                }
            }
            Some((next, entry_local)) => {
                self.active = next;
                if next != self.self_id {
                    self.virtual_pos = entry_local;
                } else {
                    // Retour en local : plus de bord de sortie.
                    self.exit_side = None;
                }
                let ns = self.screen_of(next);
                MoveOutcome {
                    entry: Some(to_ratio(entry_local, ns)),
                    remote_abs: None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nomad_core::layout::{NodeInfo, Screen};
    use nomad_core::Os;
    use std::collections::HashMap;

    fn node(id: NodeId) -> NodeInfo {
        NodeInfo {
            id,
            name: id.to_string(),
            os: Os::Linux,
            screen: Screen::new(100, 100),
        }
    }

    /// Construit S—A—B en rangée et un contrôleur centré sur S.
    fn setup() -> (EdgeController, NodeId, NodeId, NodeId) {
        let (s, a, b) = (NodeId::random(), NodeId::random(), NodeId::random());
        let layout = Layout::row(vec![node(s), node(a), node(b)]);
        let ctrl = EdgeController::new(s, Screen::new(100, 100), layout);
        (ctrl, s, a, b)
    }

    #[test]
    fn local_right_edge_enters_first_neighbor() {
        let (mut c, _s, a, _b) = setup();
        assert!(c.is_local());
        let out = c.local_move(99.0, 50.0);
        assert_eq!(c.active(), a);
        assert!(!c.is_local());
        assert_eq!(out.entry, Some((0.0, 0.5))); // entre par le bord gauche de A
    }

    #[test]
    fn middle_local_move_does_nothing() {
        let (mut c, s, _a, _b) = setup();
        let out = c.local_move(50.0, 50.0);
        assert_eq!(out, MoveOutcome::none());
        assert_eq!(c.active(), s);
    }

    #[test]
    fn crossing_remote_right_switches_to_next_neighbor() {
        let (mut c, _s, a, b) = setup();
        c.local_move(99.0, 50.0); // -> A à (0,50)
        let out = c.remote_advance(40.0, 0.0); // -> (40,50), pas de bord
        assert_eq!(out.remote_abs, Some((0.4, 0.5)));
        assert_eq!(c.active(), a);

        let out = c.remote_advance(70.0, 0.0); // -> (110,50) franchit la droite de A
        assert_eq!(c.active(), b);
        assert_eq!(out.entry, Some((0.0, 0.5))); // entre par la gauche de B
    }

    #[test]
    fn crossing_back_left_returns_to_local() {
        let (mut c, s, _a, _b) = setup();
        c.local_move(99.0, 50.0); // -> A à (0,50)
        c.remote_advance(50.0, 0.0); // -> A (50,50)
        let out = c.remote_advance(-60.0, 0.0); // -> (-10,50) franchit la gauche de A -> S
        assert_eq!(c.active(), s);
        assert!(c.is_local());
        // Rentre par le bord droit de S : dernier pixel (99/100 = 0.99), même hauteur.
        assert_eq!(out.entry, Some((0.99, 0.5)));
    }

    #[test]
    fn exit_side_tracks_departure_edge() {
        let (mut c, _s, a, b) = setup();
        assert_eq!(c.exit_side(), None); // local
        c.local_move(99.0, 50.0); // sortie par la droite -> A
        assert_eq!(c.active(), a);
        assert_eq!(c.exit_side(), Some(Side::Right));
        // Saut distant→distant A -> B : le bord de sortie local reste le même.
        c.remote_advance(110.0, 0.0);
        assert_eq!(c.active(), b);
        assert_eq!(c.exit_side(), Some(Side::Right));
    }

    #[test]
    fn exit_side_clears_on_return_local() {
        let (mut c, _s, _a, _b) = setup();
        c.local_move(99.0, 50.0); // -> A
        assert_eq!(c.exit_side(), Some(Side::Right));
        c.remote_advance(-60.0, 0.0); // franchit la gauche de A -> retour local
        assert!(c.is_local());
        assert_eq!(c.exit_side(), None);
    }

    #[test]
    fn world_edge_clamps_without_switch() {
        let (mut c, _s, a, _b) = setup();
        c.local_move(99.0, 50.0); // -> A
        // Bord haut de A : aucun voisin en haut -> reste sur A, collé.
        let out = c.remote_advance(0.0, -200.0);
        assert_eq!(c.active(), a);
        assert_eq!(out.remote_abs, Some((0.0, 0.0)));
    }

    /// Écran A posé **au-dessus** de S : la sortie par le haut y transite.
    #[test]
    fn local_top_edge_enters_screen_above() {
        let (s, a) = (NodeId::random(), NodeId::random());
        let mut pos = HashMap::new();
        pos.insert(s, (0, 0));
        pos.insert(a, (0, -100));
        let layout = Layout::from_positions(vec![node(s), node(a)], pos);
        let mut c = EdgeController::new(s, Screen::new(100, 100), layout);

        let out = c.local_move(30.0, 0.0); // bord haut à x=30
        assert_eq!(c.active(), a);
        assert_eq!(c.exit_side(), Some(Side::Top));
        // Entre par le bas de A (dernier pixel, 99/100), à x=30.
        assert_eq!(out.entry, Some((0.3, 0.99)));
    }

    /// Sortie vers une zone de bord sans voisin : rien ne se passe.
    #[test]
    fn edge_without_neighbor_does_nothing() {
        // A à droite mais décalé vers le bas de 60 px.
        let (s, a) = (NodeId::random(), NodeId::random());
        let mut pos = HashMap::new();
        pos.insert(s, (0, 0));
        pos.insert(a, (100, 60));
        let layout = Layout::from_positions(vec![node(s), node(a)], pos);
        let mut c = EdgeController::new(s, Screen::new(100, 100), layout);

        // Sortie à droite en haut (y=20 plan) : A couvre [60,160), pas de voisin.
        let out = c.local_move(99.0, 20.0);
        assert_eq!(out, MoveOutcome::none());
        assert!(c.is_local());
        // Sortie à droite en bas (y=80 plan) : dans A → transition.
        let out = c.local_move(99.0, 80.0);
        assert_eq!(c.active(), a);
        assert_eq!(out.entry, Some((0.0, 0.2))); // 80 - 60 = 20 local, /100
    }

    /// Retour vertical vers l'écran local depuis l'écran du dessus.
    #[test]
    fn crossing_down_returns_to_local() {
        let (s, a) = (NodeId::random(), NodeId::random());
        let mut pos = HashMap::new();
        pos.insert(s, (0, 0));
        pos.insert(a, (0, -100));
        let layout = Layout::from_positions(vec![node(s), node(a)], pos);
        let mut c = EdgeController::new(s, Screen::new(100, 100), layout);

        c.local_move(30.0, 0.0); // -> A par le haut ; on atterrit en bas de A (y=99)
        assert_eq!(c.active(), a);
        c.remote_advance(0.0, -40.0); // remonte dans A -> (30, 59)
        let out = c.remote_advance(0.0, 60.0); // franchit le bas de A -> retour S
        assert!(c.is_local());
        assert_eq!(c.exit_side(), None);
        assert_eq!(out.entry, Some((0.3, 0.0))); // rentre par le haut de S, à x=30
    }
}
