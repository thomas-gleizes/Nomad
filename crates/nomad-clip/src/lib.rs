//! `nomad-clip` — synchronisation du presse-papiers (texte, MVP) via `arboard`.
//!
//! `arboard::Clipboard` n'est pas `Send` sur toutes les plateformes : un **unique
//! thread** en est propriétaire et gère les deux sens via [`run`] :
//! - sondage périodique du presse-papiers local → callback `on_change` ;
//! - application des textes distants reçus par le canal de commandes.
//!
//! Gérer les deux sens au même endroit évite l'écho (re-diffusion d'un texte
//! qu'on vient soi-même d'écrire) : toute écriture met à jour la dernière valeur
//! connue, que le sondage compare ensuite.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use arboard::Clipboard;
use tracing::{debug, warn};

/// Commande adressée au thread presse-papiers.
#[derive(Debug)]
pub enum ClipCmd {
    /// Écrit ce texte dans le presse-papiers local (provenant d'un nœud distant).
    SetText(String),
}

/// Boucle propriétaire du presse-papiers. **Bloquante** : à lancer dans un thread dédié.
///
/// `on_change` est appelé avec le nouveau texte chaque fois que le presse-papiers
/// local change (par action de l'utilisateur). La boucle s'arrête quand
/// `cmd_rx` est fermé.
pub fn run<F>(poll: Duration, cmd_rx: Receiver<ClipCmd>, mut on_change: F) -> anyhow::Result<()>
where
    F: FnMut(String),
{
    let mut clipboard = Clipboard::new()?;
    let mut last = clipboard.get_text().ok();

    loop {
        match cmd_rx.recv_timeout(poll) {
            Ok(ClipCmd::SetText(text)) => {
                if last.as_deref() != Some(text.as_str()) {
                    if let Err(e) = clipboard.set_text(text.clone()) {
                        warn!(error = %e, "échec écriture presse-papiers");
                    } else {
                        debug!(len = text.len(), "presse-papiers distant appliqué");
                        last = Some(text);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Ok(current) = clipboard.get_text() {
                    if !current.is_empty() && last.as_deref() != Some(current.as_str()) {
                        debug!(len = current.len(), "changement presse-papiers local détecté");
                        last = Some(current.clone());
                        on_change(current);
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}
