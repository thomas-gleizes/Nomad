//! Thread propriétaire de l'injecteur `enigo`.
//!
//! `enigo::Enigo` est conservé sur un thread unique ; l'orchestrateur lui envoie
//! des commandes via un canal. Côté client, on injecte des événements ; côté
//! serveur, on s'en sert uniquement pour recentrer (warp) le curseur local.

use std::sync::mpsc::Receiver;

use nb_core::layout::Screen;
use nb_core::InputEvent;
use nb_input::Injector;
use tracing::warn;

/// Commande adressée au thread d'injection.
#[derive(Debug)]
pub enum InjectCmd {
    /// Injecter un événement d'entrée reçu du réseau (clients).
    Event(InputEvent),
    /// Déplacer le curseur local à une position absolue en pixels (warp).
    Warp(i32, i32),
}

/// Boucle d'injection. **Bloquante** : à lancer dans un thread dédié.
pub fn run(screen: Screen, rx: Receiver<InjectCmd>) -> anyhow::Result<()> {
    let mut injector = Injector::new(screen)?;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            InjectCmd::Event(ev) => {
                if let Err(e) = injector.inject(&ev) {
                    warn!(error = %e, "échec d'injection");
                }
            }
            InjectCmd::Warp(x, y) => injector.warp_cursor(x, y),
        }
    }
    Ok(())
}
