//! Modèle de disposition des écrans (qui est à gauche/droite de qui) et calcul
//! des points d'entrée lors d'une traversée de bord.
//!
//! Pour rester simple, chaque nœud est représenté par **un** écran virtuel
//! (sa résolution principale). La disposition par défaut aligne les nœuds en
//! une rangée horizontale, dans l'ordre d'arrivée, de gauche à droite.

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

/// Description d'un nœud connu de la disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: NodeId,
    pub name: String,
    pub os: Os,
    pub screen: Screen,
}

/// Disposition globale, détenue par le serveur et diffusée aux clients.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub nodes: Vec<NodeInfo>,
    /// Graphe d'adjacence : `(nœud, côté) -> voisin`.
    pub neighbors: HashMap<(NodeId, Side), NodeId>,
}

impl Layout {
    /// Construit une rangée horizontale (gauche → droite) dans l'ordre fourni.
    pub fn horizontal_row(nodes: Vec<NodeInfo>) -> Self {
        let mut neighbors = HashMap::new();
        for pair in nodes.windows(2) {
            let (l, r) = (&pair[0], &pair[1]);
            neighbors.insert((l.id, Side::Right), r.id);
            neighbors.insert((r.id, Side::Left), l.id);
        }
        Self { nodes, neighbors }
    }

    pub fn node(&self, id: NodeId) -> Option<&NodeInfo> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Le voisin atteint en quittant `id` par `side`, s'il existe.
    pub fn neighbor(&self, id: NodeId, side: Side) -> Option<NodeId> {
        self.neighbors.get(&(id, side)).copied()
    }

    /// Point d'entrée chez le voisin (ratios 0.0..=1.0) lorsqu'on sort par
    /// `exit_side` à la position perpendiculaire `perp_ratio`.
    ///
    /// Exemple : sortie par la droite à mi-hauteur (`perp_ratio = 0.5`) → entrée
    /// sur le bord gauche du voisin, à `rx = 0.0`, `ry = 0.5`.
    pub fn entry_ratio(exit_side: Side, perp_ratio: f64) -> (f64, f64) {
        let perp = perp_ratio.clamp(0.0, 1.0);
        match exit_side {
            // On entre collé au bord opposé, à la même hauteur.
            Side::Right => (0.0, perp),
            Side::Left => (1.0, perp),
            Side::Bottom => (perp, 0.0),
            Side::Top => (perp, 1.0),
        }
    }
}
