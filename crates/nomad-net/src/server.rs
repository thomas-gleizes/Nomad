//! Serveur TCP : hub en étoile acceptant N clients.
//!
//! Le serveur ne décide d'aucune sémantique applicative : il expose les
//! événements (arrivée/départ/message) et permet d'envoyer vers un nœud précis
//! ou en diffusion. C'est l'orchestrateur (`nomad-app`, côté serveur) qui décide du
//! routage en réagissant aux événements.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nomad_core::layout::Screen;
use nomad_core::{Message, NodeId, Os};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, error};

use crate::discovery::Registration;
use crate::transport::{read_message, write_message};

/// Événement remonté par le serveur à l'orchestrateur.
#[derive(Debug)]
pub enum ServerEvent {
    Joined {
        node: NodeId,
        name: String,
        os: Os,
        screen: Screen,
    },
    Left {
        node: NodeId,
    },
    Message {
        from: NodeId,
        msg: Message,
    },
}

type ConnMap = Arc<Mutex<HashMap<NodeId, mpsc::UnboundedSender<Message>>>>;

/// Poignée de contrôle du serveur.
pub struct ServerHandle {
    events: mpsc::UnboundedReceiver<ServerEvent>,
    conns: ConnMap,
    local_id: NodeId,
    /// Maintient l'annonce mDNS vivante tant que le serveur tourne.
    pub(crate) _registration: Option<Registration>,
}

impl ServerHandle {
    pub fn local_id(&self) -> NodeId {
        self.local_id
    }

    /// Prochain événement serveur (`None` si le serveur s'est arrêté).
    pub async fn recv(&mut self) -> Option<ServerEvent> {
        self.events.recv().await
    }

    /// Envoie un message à un nœud connecté (silencieux s'il est absent).
    pub fn send_to(&self, node: NodeId, msg: Message) {
        if let Some(tx) = self.conns.lock().unwrap().get(&node) {
            let _ = tx.send(msg);
        }
    }

    /// Diffuse à tous les nœuds connectés.
    pub fn broadcast(&self, msg: Message) {
        for tx in self.conns.lock().unwrap().values() {
            let _ = tx.send(msg.clone());
        }
    }

    /// Diffuse à tous sauf `except`.
    pub fn broadcast_except(&self, except: NodeId, msg: Message) {
        for (id, tx) in self.conns.lock().unwrap().iter() {
            if *id != except {
                let _ = tx.send(msg.clone());
            }
        }
    }

    /// Liste des nœuds actuellement connectés.
    pub fn connected_nodes(&self) -> Vec<NodeId> {
        self.conns.lock().unwrap().keys().copied().collect()
    }
}

/// Démarre le serveur sur `port` (0 = port éphémère). Retourne la poignée et le
/// port effectivement écouté (à publier en mDNS).
pub async fn start(local_id: NodeId, port: u16) -> anyhow::Result<(ServerHandle, u16)> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let actual_port = listener.local_addr()?.port();

    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let conns: ConnMap = Arc::new(Mutex::new(HashMap::new()));

    let accept_conns = conns.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _peer)) => {
                    let ev_tx = ev_tx.clone();
                    let conns = accept_conns.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_conn(stream, ev_tx, conns).await {
                            debug!(error = %e, "connexion client terminée");
                        }
                    });
                }
                Err(e) => {
                    error!(error = %e, "échec accept(), arrêt de la boucle");
                    break;
                }
            }
        }
    });

    let handle = ServerHandle {
        events: ev_rx,
        conns,
        local_id,
        _registration: None,
    };
    Ok((handle, actual_port))
}

async fn handle_conn(
    stream: TcpStream,
    ev_tx: mpsc::UnboundedSender<ServerEvent>,
    conns: ConnMap,
) -> anyhow::Result<()> {
    let _ = stream.set_nodelay(true); // latence d'abord
    let (mut rh, mut wh) = stream.into_split();

    // Le premier message doit être un Hello.
    let (node, name, os, screen) = match read_message(&mut rh).await? {
        Message::Hello {
            node_id,
            name,
            os,
            screen,
        } => (node_id, name, os, screen),
        other => anyhow::bail!("premier message inattendu: {other:?}"),
    };

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    conns.lock().unwrap().insert(node, out_tx);
    let _ = ev_tx.send(ServerEvent::Joined {
        node,
        name,
        os,
        screen,
    });

    // Tâche d'écriture dédiée pour ce client.
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if write_message(&mut wh, &msg).await.is_err() {
                break;
            }
        }
    });

    // Boucle de lecture jusqu'à déconnexion.
    let result = loop {
        match read_message(&mut rh).await {
            Ok(msg) => {
                if ev_tx.send(ServerEvent::Message { from: node, msg }).is_err() {
                    break Ok(());
                }
            }
            Err(e) => break Err(e),
        }
    };

    conns.lock().unwrap().remove(&node);
    let _ = ev_tx.send(ServerEvent::Left { node });
    writer.abort();
    result
}
