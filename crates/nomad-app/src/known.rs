//! Gestion **pure** des machines connues (déjà vues, actuellement en ligne ou
//! non). Le serveur en est l'unique gestionnaire ; ces fonctions ne font aucune
//! E/S ni horodatage — le temps est injecté — afin de rester trivialement
//! testables, comme `edge.rs`/`motion.rs`.

use std::collections::HashSet;

use nomad_core::{KnownPeer, NodeId, Os};

/// Enregistre (ou met à jour) une machine vue à l'instant `now_unix`.
///
/// Upsert par identifiant : nom, OS, adresse et horodatage sont rafraîchis si
/// la machine était déjà connue.
pub fn record_seen(
    known: &mut Vec<KnownPeer>,
    id: NodeId,
    name: String,
    os: Os,
    addr: Option<String>,
    now_unix: u64,
) {
    if let Some(entry) = known.iter_mut().find(|k| k.id == id) {
        entry.name = name;
        entry.os = os;
        if addr.is_some() {
            entry.last_addr = addr;
        }
        entry.last_seen_unix = now_unix;
    } else {
        known.push(KnownPeer {
            id,
            name,
            os,
            last_addr: addr,
            last_seen_unix: now_unix,
        });
    }
}

/// Retire une machine connue. Renvoie `true` si elle existait.
pub fn forget(known: &mut Vec<KnownPeer>, id: NodeId) -> bool {
    let before = known.len();
    known.retain(|k| k.id != id);
    known.len() != before
}

/// Machines connues qui ne sont **pas** dans `connected`, triées de la plus
/// récemment vue à la plus ancienne.
pub fn offline(known: &[KnownPeer], connected: &HashSet<NodeId>) -> Vec<KnownPeer> {
    let mut out: Vec<KnownPeer> = known
        .iter()
        .filter(|k| !connected.contains(&k.id))
        .cloned()
        .collect();
    out.sort_by(|a, b| b.last_seen_unix.cmp(&a.last_seen_unix));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> NodeId {
        NodeId(uuid::Uuid::from_u128(n))
    }

    #[test]
    fn record_inserts_then_updates_in_place() {
        let mut k = Vec::new();
        record_seen(&mut k, id(1), "forge".into(), Os::Windows, Some("1.1.1.1:1".into()), 100);
        assert_eq!(k.len(), 1);

        // Même id : mise à jour sur place (pas de doublon).
        record_seen(&mut k, id(1), "forge2".into(), Os::Windows, Some("2.2.2.2:2".into()), 200);
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].name, "forge2");
        assert_eq!(k[0].last_addr.as_deref(), Some("2.2.2.2:2"));
        assert_eq!(k[0].last_seen_unix, 200);
    }

    #[test]
    fn record_keeps_last_addr_when_none_provided() {
        let mut k = Vec::new();
        record_seen(&mut k, id(1), "forge".into(), Os::Windows, Some("1.1.1.1:1".into()), 100);
        record_seen(&mut k, id(1), "forge".into(), Os::Windows, None, 150);
        assert_eq!(k[0].last_addr.as_deref(), Some("1.1.1.1:1"));
    }

    #[test]
    fn forget_removes_existing_only() {
        let mut k = Vec::new();
        record_seen(&mut k, id(1), "a".into(), Os::Linux, None, 1);
        assert!(forget(&mut k, id(1)));
        assert!(k.is_empty());
        assert!(!forget(&mut k, id(1)));
    }

    #[test]
    fn offline_excludes_connected_and_sorts_desc() {
        let mut k = Vec::new();
        record_seen(&mut k, id(1), "vieux".into(), Os::Linux, None, 100);
        record_seen(&mut k, id(2), "recent".into(), Os::Linux, None, 300);
        record_seen(&mut k, id(3), "connecte".into(), Os::Linux, None, 200);

        let connected: HashSet<NodeId> = [id(3)].into_iter().collect();
        let off = offline(&k, &connected);

        assert_eq!(off.len(), 2);
        assert_eq!(off[0].name, "recent"); // last_seen le plus grand d'abord
        assert_eq!(off[1].name, "vieux");
    }
}
