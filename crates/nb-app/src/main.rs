//! `no-border` — partage souris/clavier/presse-papiers sur LAN, cross-OS.
//!
//! Au lancement : recherche d'un serveur via mDNS ; s'il en existe un, on le
//! rejoint en **client (écran)** ; sinon on devient **serveur (contrôleur)**.
//!
//! Contrainte de threads (cf. `nb-input`) : la capture rdev est bloquante et doit
//! tourner sur le thread principal (macOS). On y exécute donc la capture côté
//! serveur, le runtime tokio (réseau + orchestration) vivant sur d'autres threads.

mod config;
mod edge;
mod inject_thread;
mod orchestrator;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use nb_core::layout::Screen;
use nb_core::Os;
use nb_input::Captured;
use nb_net::{Endpoint, Identity};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::inject_thread::InjectCmd;

#[derive(Parser, Debug)]
#[command(name = "no-border", version, about = "Souris/clavier/presse-papiers partagés sur LAN")]
struct Cli {
    /// Nom affiché du nœud (par défaut : valeur du fichier de config).
    #[arg(long)]
    name: Option<String>,
    /// Port d'écoute serveur.
    #[arg(long)]
    port: Option<u16>,
    /// Forcer le rôle serveur (sans recherche préalable).
    #[arg(long)]
    server: bool,
    /// Durée de recherche d'un serveur, en secondes.
    #[arg(long, default_value_t = 2)]
    discovery_secs: u64,
    /// Intervalle de sondage du presse-papiers, en millisecondes.
    #[arg(long, default_value_t = 400)]
    clip_poll_ms: u64,
    /// Chemin du fichier de configuration.
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();

    let config_path = cli
        .config
        .clone()
        .or_else(Config::default_path)
        .ok_or_else(|| anyhow::anyhow!("impossible de déterminer le chemin de configuration"))?;
    let mut cfg = Config::load_or_create(&config_path)?;
    if let Some(name) = cli.name.clone() {
        cfg.name = name;
    }
    if let Some(port) = cli.port {
        cfg.port = port;
    }

    let screen = detect_screen();
    let identity = Identity {
        id: cfg.node_id(),
        name: cfg.name.clone(),
        os: Os::current(),
        screen,
    };
    info!(node = %identity.id, name = %identity.name,
          screen = format!("{}x{}", screen.width, screen.height), "démarrage no-border");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let net_cfg = nb_net::Config {
        port: cfg.port,
        discovery_timeout: Duration::from_secs(cli.discovery_secs),
        force_server: cli.server,
    };
    let endpoint = rt.block_on(nb_net::start(identity.clone(), net_cfg))?;

    // Thread d'injection (propriétaire d'enigo).
    let (inject_tx, inject_rx) = std::sync::mpsc::channel::<InjectCmd>();
    std::thread::Builder::new()
        .name("nb-inject".into())
        .spawn(move || {
            if let Err(e) = inject_thread::run(screen, inject_rx) {
                error!(error = %e, "thread d'injection arrêté");
            }
        })?;

    // Thread presse-papiers (propriétaire d'arboard).
    let (clip_cmd_tx, clip_cmd_rx) = std::sync::mpsc::channel();
    let (clip_change_tx, clip_change_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let clip_poll = Duration::from_millis(cli.clip_poll_ms);
    std::thread::Builder::new()
        .name("nb-clip".into())
        .spawn(move || {
            let on_change = move |text: String| {
                let _ = clip_change_tx.send(text);
            };
            if let Err(e) = nb_clip::run(clip_poll, clip_cmd_rx, on_change) {
                error!(error = %e, "thread presse-papiers arrêté");
            }
        })?;

    match endpoint {
        Endpoint::Server(srv) => {
            info!("rôle SERVEUR (contrôleur) — capture clavier/souris locale");
            print_permissions_hint();

            let grabbing = Arc::new(AtomicBool::new(false));
            let (cap_tx, cap_rx) = tokio::sync::mpsc::unbounded_channel::<Captured>();

            rt.spawn(orchestrator::run_server(
                srv,
                identity.clone(),
                cap_rx,
                inject_tx,
                clip_cmd_tx,
                clip_change_rx,
                grabbing.clone(),
            ));

            // Capture bloquante sur le thread principal. La fermeture décide de la
            // suppression locale : on supprime clavier/boutons/molette tant qu'on
            // contrôle un client, mais on laisse passer les mouvements souris
            // (nécessaires pour continuer à produire des deltas via le recentrage).
            let grab_flag = grabbing.clone();
            nb_input::capture::grab(move |captured| {
                let suppress = grab_flag.load(Ordering::Relaxed)
                    && !matches!(captured, Captured::MouseMoveAbs { .. });
                let _ = cap_tx.send(captured);
                suppress
            })?;
        }
        Endpoint::Client(handle) => {
            info!("rôle CLIENT (écran) — injection des événements du serveur");
            print_permissions_hint();
            rt.block_on(orchestrator::run_client(
                handle,
                inject_tx,
                clip_cmd_tx,
                clip_change_rx,
                screen,
            ));
        }
    }

    Ok(())
}

/// Détecte la résolution de l'écran principal.
fn detect_screen() -> Screen {
    match display_info::DisplayInfo::all() {
        Ok(displays) => {
            let chosen = displays
                .iter()
                .find(|d| d.is_primary)
                .or_else(|| displays.first());
            match chosen {
                Some(d) => Screen::new(d.width, d.height),
                None => fallback_screen(),
            }
        }
        Err(e) => {
            error!(error = %e, "détection écran impossible, valeur par défaut");
            fallback_screen()
        }
    }
}

fn fallback_screen() -> Screen {
    Screen::new(1920, 1080)
}

#[cfg(target_os = "macos")]
fn print_permissions_hint() {
    info!(
        "macOS : autorisez no-border dans Réglages Système → Confidentialité et sécurité → \
         Accessibilité ET Surveillance des entrées, sinon capture/injection seront inopérantes."
    );
}

#[cfg(not(target_os = "macos"))]
fn print_permissions_hint() {}
