//! Connexion cliente vers le serveur.

use std::net::SocketAddr;

use nomad_core::Message;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::transport::{read_message, write_message};

/// Poignée d'une connexion cliente.
pub struct ClientHandle {
    incoming: mpsc::UnboundedReceiver<Message>,
    outgoing: mpsc::UnboundedSender<Message>,
}

impl ClientHandle {
    /// Prochain message du serveur (`None` si la connexion est rompue).
    pub async fn recv(&mut self) -> Option<Message> {
        self.incoming.recv().await
    }

    /// Envoie un message au serveur (silencieux si la connexion est rompue).
    pub fn send(&self, msg: Message) {
        let _ = self.outgoing.send(msg);
    }
}

/// Se connecte au serveur et envoie immédiatement le `hello`.
pub async fn connect(addr: SocketAddr, hello: Message) -> anyhow::Result<ClientHandle> {
    let stream = TcpStream::connect(addr).await?;
    let _ = stream.set_nodelay(true);
    let (mut rh, mut wh) = stream.into_split();

    write_message(&mut wh, &hello).await?;

    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();

    // Tâche d'écriture.
    tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if write_message(&mut wh, &msg).await.is_err() {
                break;
            }
        }
    });

    // Tâche de lecture : fermer `in_tx` (en sortant) signale la déconnexion.
    tokio::spawn(async move {
        loop {
            match read_message(&mut rh).await {
                Ok(msg) => {
                    if in_tx.send(msg).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    Ok(ClientHandle {
        incoming: in_rx,
        outgoing: out_tx,
    })
}
