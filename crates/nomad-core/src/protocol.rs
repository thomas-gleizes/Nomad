//! Messages échangés sur le réseau, sérialisés en bincode.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::input::InputEvent;
use crate::layout::{Layout, Screen};

/// Identifiant stable et unique d'un nœud (généré au premier lancement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub Uuid);

impl NodeId {
    pub fn random() -> Self {
        NodeId(Uuid::new_v4())
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Forme courte (8 premiers caractères) pour des logs lisibles.
        write!(f, "{:.8}", self.0.simple())
    }
}

/// Système d'exploitation d'un nœud (utile pour adapter le mapping clavier / l'UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Os {
    MacOs,
    Windows,
    Linux,
    Other,
}

impl Os {
    /// L'OS sur lequel ce binaire est compilé.
    pub fn current() -> Os {
        #[cfg(target_os = "macos")]
        {
            Os::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Os::Windows
        }
        #[cfg(target_os = "linux")]
        {
            Os::Linux
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Os::Other
        }
    }
}

/// Le protocole applicatif. Chaque variante est une trame indépendante.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Message {
    /// Premier message d'un client après connexion TCP.
    Hello {
        node_id: NodeId,
        name: String,
        os: Os,
        screen: Screen,
    },
    /// Réponse du serveur : confirme l'arrivée et envoie la disposition courante.
    Welcome { layout: Layout },
    /// Diffusion d'une nouvelle disposition (arrivée/départ d'un nœud).
    LayoutUpdate { layout: Layout },

    /// Le contrôle entre sur l'écran `node` à la position relative donnée.
    EnterScreen { node: NodeId, rx: f64, ry: f64 },
    /// Le contrôle quitte l'écran courant (le curseur local doit être restauré).
    LeaveScreen,

    /// Un événement d'entrée à appliquer sur le nœud actif.
    Input { event: InputEvent },

    /// Synchronisation du presse-papiers (texte UTF-8 pour le MVP).
    Clipboard { text: String },

    /// Battement de cœur applicatif.
    Ping,
    Pong,
}
