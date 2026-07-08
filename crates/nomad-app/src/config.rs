//! Configuration persistante (identité stable du nœud + préférences).
//!
//! Stockée en TOML dans le répertoire de configuration utilisateur
//! (`~/.config/nomad/config.toml` sous Linux/macOS).

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use nomad_core::NodeId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// UUID stable du nœud (généré au premier lancement).
    pub node_id: Uuid,
    /// Nom lisible (par défaut : nom de machine).
    pub name: String,
    /// Port d'écoute serveur.
    pub port: u16,
}

impl Config {
    pub fn node_id(&self) -> NodeId {
        NodeId(self.node_id)
    }

    /// Chemin par défaut du fichier de configuration.
    pub fn default_path() -> Option<PathBuf> {
        ProjectDirs::from("dev", "nomad", "nomad")
            .map(|d| d.config_dir().join("config.toml"))
    }

    /// Charge la config depuis `path`, ou en crée une par défaut (et la sauvegarde).
    pub fn load_or_create(path: &Path) -> anyhow::Result<Config> {
        if path.exists() {
            let text = std::fs::read_to_string(path)?;
            Ok(toml::from_str(&text)?)
        } else {
            let cfg = Config::default();
            cfg.save(path)?;
            Ok(cfg)
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        let name = hostname().unwrap_or_else(|| "nomad".to_string());
        Self {
            node_id: Uuid::new_v4(),
            name,
            port: 47800,
        }
    }
}

/// Nom de machine : variables d'environnement usuelles, puis la commande
/// `hostname` (les variables sont rarement exportées, notamment sous macOS).
fn hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}
