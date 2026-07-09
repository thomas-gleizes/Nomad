//! Repli no-op sur les plateformes sans socket Unix (Windows).
//!
//! Le démon y reste piloté par le tray `nomad-ui` ; une future coquille native
//! Windows apportera son propre transport (named pipe). `nomad-app` gate déjà
//! l'IPC derrière [`crate::SUPPORTED`], donc [`bind`] n'est jamais appelée ici —
//! ces définitions n'existent que pour que le binaire compile partout.

use std::path::Path;

use nomad_core::SharedStatus;

use crate::protocol::DaemonAction;

#[derive(Debug, thiserror::Error)]
pub enum BindError {
    #[error("une instance de nomad est déjà en cours d'exécution")]
    AlreadyRunning,
    #[error("l'API de contrôle IPC n'est pas supportée sur cette plateforme")]
    Io(#[from] std::io::Error),
}

pub struct Listener;

pub async fn bind(_path: &Path) -> Result<Listener, BindError> {
    Err(BindError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "IPC non supporté",
    )))
}

pub async fn serve<F>(_listener: Listener, _status: SharedStatus, _on_action: F) -> anyhow::Result<()>
where
    F: Fn(DaemonAction) + Send + Sync + 'static,
{
    Ok(())
}
