//! `nomad-net` — découverte LAN, élection de rôle et transport TCP.
//!
//! Point d'entrée : [`start`]. Il effectue la découverte mDNS puis :
//! - si un serveur existe → se connecte en **client** ;
//! - sinon → démarre un **serveur** et publie le service mDNS.

pub mod client;
pub mod discovery;
pub mod server;
pub mod transport;

use std::time::Duration;

use nomad_core::layout::Screen;
use nomad_core::{Message, NodeId, Os};
use tracing::{info, warn};

pub use client::ClientHandle;
pub use discovery::DiscoveredServer;
pub use server::{ServerEvent, ServerHandle};

/// Identité locale annoncée lors de la connexion.
#[derive(Debug, Clone)]
pub struct Identity {
    pub id: NodeId,
    pub name: String,
    pub os: Os,
    pub screen: Screen,
}

impl Identity {
    /// Construit le message `Hello` correspondant.
    pub fn hello(&self) -> Message {
        Message::Hello {
            node_id: self.id,
            name: self.name.clone(),
            os: self.os,
            screen: self.screen,
        }
    }
}

/// Paramètres réseau.
#[derive(Debug, Clone)]
pub struct Config {
    /// Port d'écoute en mode serveur (0 = éphémère).
    pub port: u16,
    /// Durée de recherche d'un serveur avant de s'auto-promouvoir.
    pub discovery_timeout: Duration,
    /// Force le rôle serveur sans recherche préalable.
    pub force_server: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 47800,
            discovery_timeout: Duration::from_secs(2),
            force_server: false,
        }
    }
}

/// Endpoint réseau, selon le rôle élu.
pub enum Endpoint {
    Server(ServerHandle),
    Client {
        handle: ClientHandle,
        /// Serveur auquel on s'est connecté (adresse + id mDNS si annoncé).
        server: DiscoveredServer,
    },
}

/// Effectue la découverte + l'élection de rôle, puis met en place le transport.
pub async fn start(identity: Identity, cfg: Config) -> anyhow::Result<Endpoint> {
    if !cfg.force_server {
        let timeout = cfg.discovery_timeout;
        let self_id = identity.id;
        let found =
            tokio::task::spawn_blocking(move || discovery::browse_once(timeout, self_id)).await??;
        if let Some(server) = found {
            info!(addr = %server.addr, server_id = ?server.node_id, "serveur trouvé → rôle client");
            // Une annonce peut être périmée (serveur disparu) : l'échec de
            // connexion n'est pas fatal, on se promeut serveur.
            match client::connect(server.addr, identity.hello()).await {
                Ok(client) => return Ok(Endpoint::Client { handle: client, server }),
                Err(e) => {
                    warn!(error = %e, "connexion au serveur découvert impossible → auto-promotion en serveur");
                }
            }
        } else {
            info!("aucun serveur trouvé → auto-promotion en serveur");
        }
    } else {
        info!("rôle serveur forcé");
    }

    let (mut handle, port) = server::start(identity.id, cfg.port).await?;
    let registration = discovery::Registration::announce(identity.id, &identity.name, port)?;
    handle._registration = Some(registration);
    info!(port, node = %identity.id, "serveur démarré");
    Ok(Endpoint::Server(handle))
}
