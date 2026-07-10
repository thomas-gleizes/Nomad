//! Politique **pure** de placement des écrans dans le plan virtuel.
//!
//! Une machine reprend sa position sauvegardée si elle existe et ne chevauche
//! personne ; sinon elle est collée à droite de l'écran le plus à droite déjà
//! placé (alignée en haut). Le serveur (premier nœud) est placé en premier.
//! Sans effet de bord, testable comme `known.rs`/`edge.rs`.

use std::collections::HashMap;

use nomad_core::layout::{Layout, NodeInfo, Rect};
use nomad_core::NodeId;

/// Construit une disposition à partir des nœuds et des positions sauvegardées.
pub fn build_layout(nodes: Vec<NodeInfo>, saved: &HashMap<NodeId, (i32, i32)>) -> Layout {
    let mut positions: HashMap<NodeId, (i32, i32)> = HashMap::new();
    let mut placed: Vec<Rect> = Vec::new();

    for n in &nodes {
        let (w, h) = (n.screen.width, n.screen.height);
        // Position sauvegardée si elle ne chevauche aucun écran déjà placé.
        let saved_ok = saved.get(&n.id).copied().filter(|&(x, y)| {
            let r = Rect { x, y, w, h };
            !placed.iter().any(|p| p.overlaps(&r))
        });
        let pos = saved_ok.unwrap_or_else(|| {
            let x = placed.iter().map(|r| r.right() as i32).max().unwrap_or(0);
            (x, 0)
        });
        positions.insert(n.id, pos);
        placed.push(Rect { x: pos.0, y: pos.1, w, h });
    }

    Layout::from_positions(nodes, positions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nomad_core::layout::Screen;
    use nomad_core::Os;

    fn node(id: NodeId, w: u32, h: u32) -> NodeInfo {
        NodeInfo { id, name: id.to_string(), os: Os::Linux, screen: Screen::new(w, h) }
    }

    #[test]
    fn empty_saved_falls_back_to_row() {
        let (s, a) = (NodeId::random(), NodeId::random());
        let l = build_layout(vec![node(s, 100, 100), node(a, 100, 100)], &HashMap::new());
        assert_eq!(l.positions[&s], (0, 0));
        assert_eq!(l.positions[&a], (100, 0)); // collé à droite
    }

    #[test]
    fn saved_position_is_honored() {
        let (s, a) = (NodeId::random(), NodeId::random());
        let mut saved = HashMap::new();
        saved.insert(s, (0, 0));
        saved.insert(a, (0, -100)); // au-dessus, ne chevauche pas
        let l = build_layout(vec![node(s, 100, 100), node(a, 100, 100)], &saved);
        assert_eq!(l.positions[&a], (0, -100));
    }

    #[test]
    fn overlapping_saved_position_is_dropped_and_appended() {
        let (s, a) = (NodeId::random(), NodeId::random());
        let mut saved = HashMap::new();
        saved.insert(s, (0, 0));
        saved.insert(a, (50, 50)); // chevauche S -> ignoré, append à droite
        let l = build_layout(vec![node(s, 100, 100), node(a, 100, 100)], &saved);
        assert_eq!(l.positions[&a], (100, 0));
    }

    #[test]
    fn appends_right_of_rightmost() {
        let (s, a, b) = (NodeId::random(), NodeId::random(), NodeId::random());
        let mut saved = HashMap::new();
        saved.insert(a, (300, 0)); // A explicitement loin à droite
        let l = build_layout(
            vec![node(s, 100, 100), node(a, 100, 100), node(b, 100, 100)],
            &saved,
        );
        assert_eq!(l.positions[&s], (0, 0));
        assert_eq!(l.positions[&a], (300, 0));
        assert_eq!(l.positions[&b], (400, 0)); // à droite de A (le plus à droite)
    }
}
