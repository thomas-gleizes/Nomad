//! Modèle d'état applicatif partagé entre l'orchestrateur et l'UI.
//!
//! Volontairement *pur* (aucune dépendance OS) : l'orchestrateur mute cet état
//! au fil des événements réseau, l'UI se contente de le lire. Le couplage est
//! réduit à un [`SharedStatus`] partagé + un compteur de génération : l'UI sonde
//! [`SharedStatus::generation`] et ne reconstruit son affichage que lorsqu'il
//! change, sans plomberie de notification inter-thread.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::layout::Screen;
use crate::protocol::{NodeId, Os};

/// Rôle courant du nœud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Server,
    Client,
}

impl Role {
    /// Libellé lisible (français, comme le reste de l'UI).
    pub fn label(self) -> &'static str {
        match self {
            Role::Server => "Serveur",
            Role::Client => "Client",
        }
    }
}

/// Un pair connecté (hors soi-même).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: NodeId,
    pub name: String,
    pub os: Os,
    pub screen: Screen,
    /// Adresse réseau (`ip:port`) — connue du serveur uniquement.
    #[serde(default)]
    pub addr: Option<String>,
    /// Latence aller-retour mesurée, en millisecondes (indicative).
    #[serde(default)]
    pub latency_ms: Option<u32>,
}

/// Géométrie d'un écran dans le plan virtuel, exposée à l'UI (page Disposition).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenGeom {
    pub id: NodeId,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Une machine déjà vue mais actuellement déconnectée. Persistée entre les
/// sessions dans la configuration ; le serveur en est l'unique gestionnaire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownPeer {
    pub id: NodeId,
    pub name: String,
    pub os: Os,
    /// Dernière adresse réseau connue.
    #[serde(default)]
    pub last_addr: Option<String>,
    /// Horodatage de dernière présence, en secondes epoch.
    pub last_seen_unix: u64,
}

/// Instantané de l'état applicatif présentable à l'utilisateur.
///
/// Sérialisable tel quel : c'est le payload `status` exposé par l'API de
/// contrôle IPC (`nomad-ipc`) et consommé par les coquilles natives. Les champs
/// ajoutés après coup portent `#[serde(default)]` pour rester compatibles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppStatus {
    pub role: Role,
    pub self_id: NodeId,
    pub node_name: String,
    pub os: Os,
    pub screen: Screen,
    /// Pairs connectés (hors soi-même).
    pub peers: Vec<PeerInfo>,
    /// Nœud dont l'écran est actuellement contrôlé (`None` = écran local).
    pub active: Option<NodeId>,
    /// Machines connues mais déconnectées (serveur uniquement).
    #[serde(default)]
    pub known_offline: Vec<KnownPeer>,
    /// Adresse du serveur (`ip:port`) — côté client uniquement.
    #[serde(default)]
    pub server_addr: Option<String>,
    /// Disposition courante : géométrie de chaque écran dans le plan virtuel.
    #[serde(default)]
    pub layout: Vec<ScreenGeom>,
}

impl AppStatus {
    /// État initial au démarrage, avant toute connexion.
    pub fn new(role: Role, self_id: NodeId, node_name: String, os: Os, screen: Screen) -> Self {
        Self {
            role,
            self_id,
            node_name,
            os,
            screen,
            peers: Vec::new(),
            active: None,
            known_offline: Vec::new(),
            server_addr: None,
            layout: Vec::new(),
        }
    }
}

struct StatusInner {
    status: Mutex<AppStatus>,
    /// Incrémenté à chaque mutation ; l'UI l'utilise pour détecter les changements.
    generation: AtomicU64,
}

/// Poignée partagée, clonable, vers l'état applicatif.
#[derive(Clone)]
pub struct SharedStatus(Arc<StatusInner>);

impl SharedStatus {
    pub fn new(initial: AppStatus) -> Self {
        Self(Arc::new(StatusInner {
            status: Mutex::new(initial),
            generation: AtomicU64::new(0),
        }))
    }

    /// Copie de l'état courant.
    pub fn snapshot(&self) -> AppStatus {
        self.0.status.lock().unwrap().clone()
    }

    /// Numéro de génération courant (change à chaque mutation effective).
    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::Acquire)
    }

    /// Applique une mutation puis incrémente la génération. La génération n'est
    /// bumpée que si `f` a réellement modifié l'état, pour éviter des rebuilds
    /// d'UI inutiles.
    pub fn update(&self, f: impl FnOnce(&mut AppStatus)) {
        let mut guard = self.0.status.lock().unwrap();
        let before = guard.clone();
        f(&mut guard);
        if *guard != before {
            self.0.generation.fetch_add(1, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SharedStatus {
        SharedStatus::new(AppStatus::new(
            Role::Server,
            NodeId::random(),
            "test".into(),
            Os::Linux,
            Screen::new(1920, 1080),
        ))
    }

    #[test]
    fn generation_bumps_only_on_real_change() {
        let s = sample();
        assert_eq!(s.generation(), 0);
        // Mutation effective.
        s.update(|st| {
            st.peers.push(PeerInfo {
                id: NodeId::random(),
                name: "a".into(),
                os: Os::Linux,
                screen: Screen::new(1920, 1080),
                addr: None,
                latency_ms: None,
            })
        });
        assert_eq!(s.generation(), 1);
        // Mutation sans effet : pas de bump.
        s.update(|st| {
            st.node_name = st.node_name.clone();
        });
        assert_eq!(s.generation(), 1);
    }

    #[test]
    fn snapshot_reflects_updates() {
        let s = sample();
        let id = NodeId::random();
        s.update(|st| st.active = Some(id));
        assert_eq!(s.snapshot().active, Some(id));
    }
}
