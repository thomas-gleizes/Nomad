//! `nomad-ui` — interface native légère (icône de barre de menus / systray).
//!
//! Backend actuel : la pile Tauri `tao` (event loop) + `tray-icon`
//! (`NSStatusItem` sur macOS, `Shell_NotifyIcon` sur Windows) + `muda` (menus,
//! réexporté par `tray-icon`). Le crate est cfg-gaté macOS/Windows ; sur les
//! autres plateformes il fournit un [`run`] no-op afin que le binaire compile
//! partout (Linux tourne headless).
//!
//! L'UI est purement *lectrice* : elle sonde un [`nomad_core::SharedStatus`]
//! (via son compteur de génération) et reconstruit son menu quand l'état change.
//! Les interactions utilisateur remontent au binaire via [`UiAction`].

/// `true` si un backend d'UI natif existe pour la plateforme courante.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// Action déclenchée par l'utilisateur depuis le menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    /// Quitter l'application.
    Quit,
    /// Relancer (redécouverte de rôle).
    Reconnect,
    /// Relancer en forçant le rôle serveur.
    ForceServer,
    /// Renommer le nœud (nouveau nom déjà saisi).
    Rename(String),
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod tray;

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use tray::run;

/// Repli no-op sur les plateformes sans backend natif : bloque le thread
/// courant (le travail réel tourne sur d'autres threads). Ne devrait pas être
/// appelé — le binaire bascule en mode headless quand [`SUPPORTED`] est faux.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn run(_status: nomad_core::SharedStatus, _on_action: impl FnMut(UiAction) + 'static) -> ! {
    loop {
        std::thread::park();
    }
}
