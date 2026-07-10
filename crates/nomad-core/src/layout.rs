//! Modèle de disposition des écrans dans un **plan virtuel 2D** et calcul des
//! transitions de bord.
//!
//! Chaque nœud est un écran (sa résolution principale) posé à une **position**
//! `(x, y)` en pixels dans un plan partagé. Les adjacences (qui est à
//! gauche/droite/au-dessus/en dessous de qui) ne sont pas stockées : elles sont
//! **dérivées** des rectangles qui se touchent. Le curseur transite par un bord
//! là où deux écrans sont au contact et où leur intervalle perpendiculaire se
//! recouvre — ce qui gère nativement le haut/bas, les écrans décalés et
//! plusieurs voisins sur un même côté.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::protocol::{NodeId, Os};

/// Résolution d'un écran, en pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
}

impl Screen {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// Côté d'un écran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    /// Le côté opposé (par lequel on *entre* chez le voisin).
    pub fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }

    /// `true` si le côté est horizontal (gauche/droite).
    pub fn is_horizontal(self) -> bool {
        matches!(self, Side::Left | Side::Right)
    }
}

/// Rectangle d'un écran dans le plan virtuel (origine en haut-gauche).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn left(&self) -> f64 {
        self.x as f64
    }
    pub fn top(&self) -> f64 {
        self.y as f64
    }
    pub fn right(&self) -> f64 {
        self.x as f64 + self.w as f64
    }
    pub fn bottom(&self) -> f64 {
        self.y as f64 + self.h as f64
    }

    /// Chevauchement d'aire strictement positive avec `other`.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.left() < other.right()
            && other.left() < self.right()
            && self.top() < other.bottom()
            && other.top() < self.bottom()
    }
}

/// Description d'un nœud connu de la disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: NodeId,
    pub name: String,
    pub os: Os,
    pub screen: Screen,
}

/// Disposition globale, détenue par le serveur et diffusée aux clients.
///
/// La géométrie tient entièrement dans `positions` ; les adjacences en sont
/// dérivées à la volée par [`Layout::neighbor_at`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub nodes: Vec<NodeInfo>,
    /// Origine (coin haut-gauche) de chaque écran dans le plan virtuel.
    pub positions: HashMap<NodeId, (i32, i32)>,
}

/// Tolérance de contact entre deux bords, en pixels.
const CONTACT_TOL: f64 = 2.0;

impl Layout {
    /// Construit une disposition à partir de positions explicites.
    pub fn from_positions(nodes: Vec<NodeInfo>, positions: HashMap<NodeId, (i32, i32)>) -> Self {
        Self { nodes, positions }
    }

    /// Rangée horizontale par défaut (gauche → droite) dans l'ordre fourni,
    /// alignée en haut ; le premier nœud (serveur) est à l'origine.
    pub fn row(nodes: Vec<NodeInfo>) -> Self {
        let mut positions = HashMap::new();
        let mut x = 0i32;
        for n in &nodes {
            positions.insert(n.id, (x, 0));
            x = x.saturating_add(n.screen.width as i32);
        }
        Self { nodes, positions }
    }

    pub fn node(&self, id: NodeId) -> Option<&NodeInfo> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Rectangle d'un nœud dans le plan (position + résolution).
    pub fn rect_of(&self, id: NodeId) -> Option<Rect> {
        let node = self.node(id)?;
        let &(x, y) = self.positions.get(&id).unwrap_or(&(0, 0));
        Some(Rect { x, y, w: node.screen.width, h: node.screen.height })
    }

    /// Voisin atteint en quittant `from` par `side`, à la position `coord` du
    /// plan **le long du bord** (coordonnée `y` du plan pour gauche/droite,
    /// `x` du plan pour haut/bas).
    ///
    /// Retourne le voisin et le **point d'entrée en pixels locaux** de ce voisin
    /// (déjà clampé dans son rectangle), ou `None` s'il n'y a pas d'écran au
    /// contact contenant `coord` (bord du monde → l'appelant clampe).
    ///
    /// Les chevauchements étant interdits, au plus un voisin contient `coord`.
    pub fn neighbor_at(&self, from: NodeId, side: Side, coord: f64) -> Option<(NodeId, (f64, f64))> {
        let f = self.rect_of(from)?;
        for n in &self.nodes {
            if n.id == from {
                continue;
            }
            let Some(r) = self.rect_of(n.id) else { continue };
            let hit = match side {
                Side::Right => contact(r.left(), f.right()) && within(coord, r.top(), r.bottom()),
                Side::Left => contact(r.right(), f.left()) && within(coord, r.top(), r.bottom()),
                Side::Bottom => contact(r.top(), f.bottom()) && within(coord, r.left(), r.right()),
                Side::Top => contact(r.bottom(), f.top()) && within(coord, r.left(), r.right()),
            };
            if hit {
                return Some((n.id, entry_point(side, coord, r)));
            }
        }
        None
    }
}

/// Première paire de rectangles qui se chevauchent (par identifiant), s'il y en
/// a une. Utilisé pour valider une disposition (les chevauchements rendent
/// l'adjacence ambiguë).
pub fn first_overlap(rects: &[(NodeId, Rect)]) -> Option<(NodeId, NodeId)> {
    for i in 0..rects.len() {
        for j in (i + 1)..rects.len() {
            if rects[i].1.overlaps(&rects[j].1) {
                return Some((rects[i].0, rects[j].0));
            }
        }
    }
    None
}

/// Deux bords sont au contact s'ils coïncident à `CONTACT_TOL` près.
fn contact(a: f64, b: f64) -> bool {
    (a - b).abs() <= CONTACT_TOL
}

/// `coord` est dans l'intervalle `[lo, hi)` (demi-ouvert : départage deux
/// voisins empilés sur la limite commune).
fn within(coord: f64, lo: f64, hi: f64) -> bool {
    coord >= lo && coord < hi
}

/// Point d'entrée, en pixels locaux du voisin `r`, quand on sort par `side` à la
/// position `coord` du plan le long du bord.
fn entry_point(side: Side, coord: f64, r: Rect) -> (f64, f64) {
    let maxx = r.w.saturating_sub(1) as f64;
    let maxy = r.h.saturating_sub(1) as f64;
    let along_h = (coord - r.top()).clamp(0.0, maxy); // position verticale locale
    let along_w = (coord - r.left()).clamp(0.0, maxx); // position horizontale locale
    match side {
        // On entre collé au bord opposé, à la même position le long du bord.
        Side::Right => (0.0, along_h),
        Side::Left => (maxx, along_h),
        Side::Bottom => (along_w, 0.0),
        Side::Top => (along_w, maxy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: NodeId, w: u32, h: u32) -> NodeInfo {
        NodeInfo { id, name: id.to_string(), os: Os::Linux, screen: Screen::new(w, h) }
    }

    fn ids() -> (NodeId, NodeId, NodeId) {
        (NodeId::random(), NodeId::random(), NodeId::random())
    }

    #[test]
    fn row_places_left_to_right_top_aligned() {
        let (s, a, b) = ids();
        let l = Layout::row(vec![node(s, 100, 100), node(a, 100, 100), node(b, 100, 100)]);
        assert_eq!(l.positions[&s], (0, 0));
        assert_eq!(l.positions[&a], (100, 0));
        assert_eq!(l.positions[&b], (200, 0));
    }

    #[test]
    fn row_right_neighbor_matches_old_ratio_for_equal_heights() {
        let (s, a, _b) = ids();
        let l = Layout::row(vec![node(s, 100, 100), node(a, 100, 100)]);
        // Sortie serveur par la droite à mi-hauteur : coord plan y = 50.
        let (nb, entry) = l.neighbor_at(s, Side::Right, 50.0).unwrap();
        assert_eq!(nb, a);
        assert_eq!(entry, (0.0, 50.0)); // bord gauche de A, même hauteur
    }

    #[test]
    fn vertical_stack_transition_top() {
        // A au-dessus de S (même largeur), contact sur le bord haut de S.
        let (s, a, _b) = ids();
        let mut pos = HashMap::new();
        pos.insert(s, (0, 0));
        pos.insert(a, (0, -100));
        let l = Layout::from_positions(vec![node(s, 100, 100), node(a, 100, 100)], pos);
        // Sortie de S par le haut à coord plan x = 30.
        let (nb, entry) = l.neighbor_at(s, Side::Top, 30.0).unwrap();
        assert_eq!(nb, a);
        assert_eq!(entry, (30.0, 99.0)); // entre par le bas de A, à x=30
    }

    #[test]
    fn offset_neighbor_preserves_plane_coordinate() {
        // A à droite de S mais décalé de 40 px vers le bas.
        let (s, a, _b) = ids();
        let mut pos = HashMap::new();
        pos.insert(s, (0, 0));
        pos.insert(a, (100, 40));
        let l = Layout::from_positions(vec![node(s, 100, 100), node(a, 100, 100)], pos);
        // Sortie de S par la droite à coord plan y = 60 (dans [40,140)).
        let (nb, entry) = l.neighbor_at(s, Side::Right, 60.0).unwrap();
        assert_eq!(nb, a);
        assert_eq!(entry, (0.0, 20.0)); // 60 - 40 = 20 en local
        // Coord plan y = 20 : au-dessus de A, aucun voisin.
        assert_eq!(l.neighbor_at(s, Side::Right, 20.0), None);
    }

    #[test]
    fn two_neighbors_on_same_side_are_split_by_coordinate() {
        // A (haut) et B (bas) empilés à droite de S (200 de haut).
        let (s, a, b) = ids();
        let mut pos = HashMap::new();
        pos.insert(s, (0, 0));
        pos.insert(a, (100, 0));
        pos.insert(b, (100, 100));
        let l = Layout::from_positions(
            vec![node(s, 100, 200), node(a, 100, 100), node(b, 100, 100)],
            pos,
        );
        assert_eq!(l.neighbor_at(s, Side::Right, 30.0).unwrap().0, a);
        assert_eq!(l.neighbor_at(s, Side::Right, 150.0).unwrap().0, b);
        // Pile sur la limite : demi-ouvert → B.
        assert_eq!(l.neighbor_at(s, Side::Right, 100.0).unwrap().0, b);
    }

    #[test]
    fn corner_without_contact_has_no_neighbor() {
        // A posé en haut à droite, ne touchant pas le bord droit de S.
        let (s, a, _b) = ids();
        let mut pos = HashMap::new();
        pos.insert(s, (0, 0));
        pos.insert(a, (100, -100)); // touche le coin, pas le bord droit sur [0,100)
        let l = Layout::from_positions(vec![node(s, 100, 100), node(a, 100, 100)], pos);
        // Bord droit de S : A couvre y ∈ [-100,0), aucun recouvrement avec [0,100).
        assert_eq!(l.neighbor_at(s, Side::Right, 50.0), None);
    }

    #[test]
    fn contact_tolerance_allows_small_gap() {
        let (s, a, _b) = ids();
        let mut pos = HashMap::new();
        pos.insert(s, (0, 0));
        pos.insert(a, (101, 0)); // 1 px de jeu, sous la tolérance
        let l = Layout::from_positions(vec![node(s, 100, 100), node(a, 100, 100)], pos);
        assert_eq!(l.neighbor_at(s, Side::Right, 50.0).unwrap().0, a);
    }

    #[test]
    fn entry_never_lands_outside_target_rect() {
        // Voisin plus court : l'entrée est clampée dans son rectangle.
        let (s, a, _b) = ids();
        let mut pos = HashMap::new();
        pos.insert(s, (0, 0));
        pos.insert(a, (100, 0));
        let l = Layout::from_positions(vec![node(s, 100, 200), node(a, 100, 50)], pos);
        // Sortie à y=40 (dans [0,50)) → local 40 ; à y=48 → 48 (< 49 max).
        let (_, e) = l.neighbor_at(s, Side::Right, 40.0).unwrap();
        assert_eq!(e, (0.0, 40.0));
        assert!(e.1 <= 49.0);
    }

    #[test]
    fn overlaps_detects_intersection() {
        let r1 = Rect { x: 0, y: 0, w: 100, h: 100 };
        let r2 = Rect { x: 50, y: 50, w: 100, h: 100 };
        let r3 = Rect { x: 100, y: 0, w: 100, h: 100 }; // flush, pas de chevauchement
        assert!(r1.overlaps(&r2));
        assert!(!r1.overlaps(&r3));
    }
}
