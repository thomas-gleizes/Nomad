//! `nomad-ipc` — API de contrôle locale du démon `nomad`.
//!
//! Expose l'état applicatif ([`nomad_core::SharedStatus`]) et relaie les
//! commandes de contrôle à un processus externe (l'app native macOS, l'outil
//! `ipcctl`, un script) via un **socket Unix** parlant JSON Lines. Le crate est
//! purement *lecteur* de l'état : il ne porte aucune logique métier, il rend
//! `nomad` pilotable sans dupliquer son cœur.
//!
//! Transport gaté par plateforme : socket Unix sur `unix`, no-op ailleurs
//! (cf. [`SUPPORTED`]). Le protocole est décrit dans [`protocol`].

pub mod protocol;

pub use protocol::{DaemonAction, Event, Request, Response, VERSION};

/// `true` si un backend IPC natif existe pour la plateforme courante.
pub const SUPPORTED: bool = cfg!(unix);

#[cfg(unix)]
mod server;
#[cfg(unix)]
pub use server::{bind, serve, BindError, Listener};

#[cfg(not(unix))]
mod stub;
#[cfg(not(unix))]
pub use stub::{bind, serve, BindError, Listener};
