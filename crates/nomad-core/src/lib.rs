//! `nomad-core` — types partagés entre tous les crates de nomad.
//!
//! Ce crate est volontairement *pur* (pas de tokio, pas de dépendance OS) afin de
//! rester trivialement testable : protocole réseau, événements d'entrée portables,
//! modèle de disposition d'écrans, et le codec de trame (length-prefixed bincode).

pub mod codec;
pub mod error;
pub mod input;
pub mod layout;
pub mod protocol;
pub mod status;

pub use error::{Error, Result};
pub use input::{Button, InputEvent, Key};
pub use layout::{first_overlap, Layout, NodeInfo, Rect, Screen, Side};
pub use protocol::{Message, NodeId, Os};
pub use status::{AppStatus, KnownPeer, PeerInfo, Role, ScreenGeom, SharedStatus};
