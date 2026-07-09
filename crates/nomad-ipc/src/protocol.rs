//! Protocole de l'API de contrôle locale, en **JSON Lines** (une trame JSON par
//! ligne, UTF-8).
//!
//! Volontairement indépendant du wire réseau interne (`nomad-core::codec`,
//! bincode) : ce protocole doit rester lisible et trivial à réimplémenter côté
//! coquille native (Swift `Codable`, etc.). Il est versionné dès le premier jour
//! via le champ [`VERSION`] : le démon et l'app évoluent séparément.
//!
//! Sens client → démon : [`Request`]. Sens démon → client : [`Response`]
//! (corrélée par `id`) et [`Event`] (poussée, sans `id`).

use serde::{Deserialize, Serialize};

use nomad_core::AppStatus;

/// Version courante du protocole. Toute requête doit l'annoncer.
pub const VERSION: u32 = 1;

/// Requête envoyée par un client (l'app native, `ipcctl`, un script…).
///
/// Le champ `cmd` porte la commande ; les paramètres éventuels sont des champs
/// frères optionnels (aujourd'hui seul `name` pour `rename`). `id` est un jeton
/// opaque recopié tel quel dans la [`Response`] correspondante.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub v: u32,
    #[serde(default)]
    pub id: u64,
    pub cmd: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// Réponse à une [`Request`], corrélée par `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AppStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: u64) -> Self {
        Self { v: VERSION, id, ok: true, status: None, error: None }
    }

    pub fn with_status(id: u64, status: AppStatus) -> Self {
        Self { v: VERSION, id, ok: true, status: Some(status), error: None }
    }

    pub fn error(id: u64, message: impl Into<String>) -> Self {
        Self { v: VERSION, id, ok: false, status: None, error: Some(message.into()) }
    }
}

/// Événement poussé par le démon aux connexions abonnées (`subscribe`).
///
/// Un seul type aujourd'hui (`"status"`) : l'état applicatif complet, émis à
/// l'abonnement puis à chaque changement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub v: u32,
    pub event: String,
    pub status: AppStatus,
}

impl Event {
    pub fn status(status: AppStatus) -> Self {
        Self { v: VERSION, event: "status".into(), status }
    }
}

/// Action déclenchée par une commande de contrôle. Le démon (`nomad-app`) est
/// libre de l'exécuter comme il l'entend ; aujourd'hui chacune passe par une
/// relance propre du process (sauf `Quit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonAction {
    /// Quitter l'application.
    Quit,
    /// Relancer (redécouverte de rôle).
    Reconnect,
    /// Relancer en forçant le rôle serveur.
    ForceServer,
    /// Renommer le nœud (nouveau nom déjà validé, non vide).
    Rename(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_parses_minimal() {
        let r: Request = serde_json::from_str(r#"{"v":1,"id":7,"cmd":"status"}"#).unwrap();
        assert_eq!(r.v, 1);
        assert_eq!(r.id, 7);
        assert_eq!(r.cmd, "status");
        assert!(r.name.is_none());
    }

    #[test]
    fn request_parses_rename_with_name() {
        let r: Request = serde_json::from_str(r#"{"v":1,"id":1,"cmd":"rename","name":"atlas"}"#).unwrap();
        assert_eq!(r.cmd, "rename");
        assert_eq!(r.name.as_deref(), Some("atlas"));
    }

    #[test]
    fn response_omits_empty_fields() {
        let line = serde_json::to_string(&Response::ok(3)).unwrap();
        assert_eq!(line, r#"{"v":1,"id":3,"ok":true}"#);
    }

    #[test]
    fn error_response_roundtrips() {
        let src = Response::error(9, "commande inconnue");
        let line = serde_json::to_string(&src).unwrap();
        let back: Response = serde_json::from_str(&line).unwrap();
        assert!(!back.ok);
        assert_eq!(back.error.as_deref(), Some("commande inconnue"));
    }
}
