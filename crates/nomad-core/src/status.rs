//! Modèle d'état applicatif partagé entre l'orchestrateur et l'UI.
//!
//! Volontairement *pur* (aucune dépendance OS) : l'orchestrateur mute cet état
//! au fil des événements réseau, l'UI se contente de le lire. Le couplage est
//! réduit à un [`SharedStatus`] partagé + un compteur de génération : l'UI sonde
//! [`SharedStatus::generation`] et ne reconstruit son affichage que lorsqu'il
//! change, sans plomberie de notification inter-thread.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::layout::Screen;
use crate::protocol::{NodeId, Os};

/// Rôle courant du nœud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Un pair connu de la disposition (hors soi-même).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub id: NodeId,
    pub name: String,
}

/// Instantané de l'état applicatif présentable à l'utilisateur.
#[derive(Debug, Clone, PartialEq)]
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
        s.update(|st| st.peers.push(PeerInfo { id: NodeId::random(), name: "a".into() }));
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
