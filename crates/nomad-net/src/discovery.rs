//! Découverte de service mDNS/DNS-SD via `mdns-sd`.
//!
//! Un serveur nomad publie le service `_nomad._tcp.local.` avec, en TXT,
//! son identifiant de nœud. Les nouveaux venus parcourent ce service pour
//! décider s'ils doivent rejoindre un serveur existant ou en devenir un.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use nomad_core::NodeId;
use tracing::{debug, warn};

/// Type de service DNS-SD réservé à nomad.
pub const SERVICE_TYPE: &str = "_nomad._tcp.local.";

/// Clé TXT transportant l'UUID du nœud serveur.
const TXT_NODE_ID: &str = "node_id";

/// Un serveur découvert sur le LAN.
#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub addr: SocketAddr,
    pub node_id: Option<NodeId>,
}

/// Parcourt le LAN pendant `timeout` et retourne le premier serveur résolu.
///
/// Bloquant (utilise le daemon mDNS sur un thread interne) : à appeler depuis
/// `tokio::task::spawn_blocking`.
pub fn browse_once(timeout: Duration) -> anyhow::Result<Option<DiscoveredServer>> {
    let daemon = ServiceDaemon::new()?;
    let receiver = daemon.browse(SERVICE_TYPE)?;
    let deadline = Instant::now() + timeout;

    let found = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break None;
        }
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                if let Some(server) = resolve(&info) {
                    break Some(server);
                }
            }
            Ok(other) => debug!(?other, "événement mDNS ignoré"),
            Err(_) => break None, // timeout
        }
    };

    let _ = daemon.shutdown();
    Ok(found)
}

fn resolve(info: &ServiceInfo) -> Option<DiscoveredServer> {
    let port = info.get_port();
    // Préfère une adresse IPv4 routable.
    let ip: IpAddr = info
        .get_addresses()
        .iter()
        .find(|a| a.is_ipv4() && !a.is_loopback())
        .or_else(|| info.get_addresses().iter().next())
        .copied()?;

    let node_id = info
        .get_property_val_str(TXT_NODE_ID)
        .and_then(|s| s.parse().ok())
        .map(NodeId);

    Some(DiscoveredServer {
        addr: SocketAddr::new(ip, port),
        node_id,
    })
}

/// Enregistrement mDNS actif tant que ce garde est vivant.
pub struct Registration {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Registration {
    /// Publie le service serveur sur le LAN.
    pub fn announce(node_id: NodeId, name: &str, port: u16) -> anyhow::Result<Registration> {
        let daemon = ServiceDaemon::new()?;
        let instance = sanitize_instance(name);
        let host = format!("nomad-{node_id}.local.");
        let properties = [(TXT_NODE_ID, node_id.0.to_string())];

        // `enable_addr_auto` laisse le daemon renseigner les adresses de toutes
        // les interfaces réseau accessibles.
        let info = ServiceInfo::new(SERVICE_TYPE, &instance, &host, "", port, &properties[..])?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon.register(info)?;
        debug!(%fullname, port, "service mDNS publié");
        Ok(Registration { daemon, fullname })
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Err(e) = self.daemon.unregister(&self.fullname) {
            warn!(error = %e, "échec du retrait mDNS");
        }
        let _ = self.daemon.shutdown();
    }
}

/// Les noms d'instance DNS-SD ne doivent pas contenir de point.
fn sanitize_instance(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '.' { '-' } else { c })
        .collect();
    if cleaned.is_empty() {
        "nomad".to_string()
    } else {
        cleaned
    }
}
