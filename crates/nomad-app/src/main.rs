//! `nomad` — partage souris/clavier/presse-papiers sur LAN, cross-OS.
//!
//! Au lancement : recherche d'un serveur via mDNS ; s'il en existe un, on le
//! rejoint en **client (écran)** ; sinon on devient **serveur (contrôleur)**.
//!
//! Contrainte de threads (cf. `nomad-input`) : la capture rdev est bloquante et doit
//! tourner sur le thread principal (macOS). On y exécute donc la capture côté
//! serveur, le runtime tokio (réseau + orchestration) vivant sur d'autres threads.

mod config;
mod edge;
mod inject_thread;
mod motion;
mod orchestrator;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use nomad_core::layout::Screen;
use nomad_core::status::{AppStatus, Role, SharedStatus};
use nomad_core::{Button, Key, Os};
use nomad_input::Captured;
use nomad_net::{Endpoint, Identity};
use nomad_ui::UiAction;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::inject_thread::InjectCmd;

#[derive(Parser, Debug)]
#[command(name = "nomad", version, about = "Souris/clavier/presse-papiers partagés sur LAN")]
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
    /// Désactiver l'interface (icône de barre de menus) : mode headless.
    #[arg(long)]
    headless: bool,
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
          screen = format!("{}x{}", screen.width, screen.height), "démarrage nomad");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let net_cfg = nomad_net::Config {
        port: cfg.port,
        discovery_timeout: Duration::from_secs(cli.discovery_secs),
        force_server: cli.server,
    };
    let endpoint = rt.block_on(nomad_net::start(identity.clone(), net_cfg))?;

    // Thread d'injection (propriétaire d'enigo).
    let (inject_tx, inject_rx) = std::sync::mpsc::channel::<InjectCmd>();
    std::thread::Builder::new()
        .name("nomad-inject".into())
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
        .name("nomad-clip".into())
        .spawn(move || {
            let on_change = move |text: String| {
                let _ = clip_change_tx.send(text);
            };
            if let Err(e) = nomad_clip::run(clip_poll, clip_cmd_rx, on_change) {
                error!(error = %e, "thread presse-papiers arrêté");
            }
        })?;

    // État partagé lu par l'UI ; l'orchestrateur le met à jour.
    let role = match &endpoint {
        Endpoint::Server(_) => Role::Server,
        Endpoint::Client(_) => Role::Client,
    };
    let status = SharedStatus::new(AppStatus::new(
        role,
        identity.id,
        identity.name.clone(),
        identity.os,
        screen,
    ));

    // UI native seulement si la plateforme la supporte et qu'on n'est pas en headless.
    let ui_enabled = nomad_ui::SUPPORTED && !cli.headless;

    // Actions du menu : elles passent par une relance propre du process (réutilise
    // tout le chemin de démarrage) plutôt qu'une reconfiguration réseau à chaud.
    let on_action = {
        let cfg = cfg.clone();
        let config_path = config_path.clone();
        let original_args: Vec<String> = std::env::args().skip(1).collect();
        let base_args: Vec<String> = original_args
            .iter()
            .filter(|a| a.as_str() != "--server")
            .cloned()
            .collect();
        move |action: UiAction| match action {
            UiAction::Quit => std::process::exit(0),
            UiAction::Reconnect => relaunch(&base_args),
            UiAction::ForceServer => {
                let mut a = base_args.clone();
                a.push("--server".into());
                relaunch(&a)
            }
            UiAction::Rename(name) => {
                let mut c = cfg.clone();
                c.name = name;
                if let Err(e) = c.save(&config_path) {
                    error!(error = %e, "sauvegarde du nom impossible");
                }
                relaunch(&original_args) // conserve le rôle courant
            }
        }
    };

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
                status.clone(),
            ));

            // Capture globale bloquante. La fermeture décide de la suppression
            // locale : on supprime clavier/boutons/molette tant qu'on contrôle un
            // client, mais on laisse passer les mouvements souris (nécessaires pour
            // continuer à produire des deltas via le recentrage).
            //
            // Un relâchement n'est supprimé que si son appui l'a été : une touche
            // appuyée avant la transition doit être relâchée localement (et
            // inversement), sinon elle reste coincée d'un côté.
            let grab_flag = grabbing.clone();
            let capture = move || -> anyhow::Result<()> {
                let suppressed_keys = std::sync::Mutex::new(std::collections::HashSet::<Key>::new());
                let suppressed_buttons =
                    std::sync::Mutex::new(std::collections::HashSet::<Button>::new());
                nomad_input::capture::grab(move |captured| {
                    let grabbing = grab_flag.load(Ordering::Relaxed);
                    let suppress = match captured {
                        Captured::MouseMoveAbs { .. } => false,
                        Captured::MouseWheel { .. } => grabbing,
                        Captured::Key { key, pressed: true } => {
                            if grabbing {
                                suppressed_keys.lock().unwrap().insert(key);
                            }
                            grabbing
                        }
                        Captured::Key { key, pressed: false } => {
                            suppressed_keys.lock().unwrap().remove(&key)
                        }
                        Captured::MouseButton { button, pressed: true } => {
                            if grabbing {
                                suppressed_buttons.lock().unwrap().insert(button);
                            }
                            grabbing
                        }
                        Captured::MouseButton { button, pressed: false } => {
                            suppressed_buttons.lock().unwrap().remove(&button)
                        }
                    };
                    let _ = cap_tx.send(captured);
                    suppress
                })
            };

            if ui_enabled {
                // L'UI possède le thread principal (requis macOS) ; la capture rdev
                // déménage sur un thread dédié.
                std::thread::Builder::new()
                    .name("nomad-capture".into())
                    .spawn(move || {
                        if let Err(e) = capture() {
                            error!(error = %e, "capture arrêtée");
                        }
                    })?;
                nomad_ui::run(status, on_action)
            } else {
                // Headless : capture bloquante sur le thread principal (comportement
                // d'origine).
                capture()?;
                Ok(())
            }
        }
        Endpoint::Client(handle) => {
            info!("rôle CLIENT (écran) — injection des événements du serveur");
            print_permissions_hint();
            let client = orchestrator::run_client(
                handle,
                inject_tx,
                clip_cmd_tx,
                clip_change_rx,
                screen,
                identity.id,
                status.clone(),
            );
            if ui_enabled {
                rt.spawn(client);
                nomad_ui::run(status, on_action)
            } else {
                rt.block_on(client);
                Ok(())
            }
        }
    }
}

/// Relance le binaire avec les arguments donnés, puis termine le process courant.
/// Utilisé par les actions du menu (reconnecter / forcer serveur / renommer).
fn relaunch(args: &[String]) -> ! {
    match std::env::current_exe() {
        Ok(exe) => {
            let _ = std::process::Command::new(exe).args(args).spawn();
        }
        Err(e) => error!(error = %e, "current_exe indisponible, relance impossible"),
    }
    std::process::exit(0);
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
        "macOS : autorisez nomad dans Réglages Système → Confidentialité et sécurité → \
         Accessibilité ET Surveillance des entrées, sinon capture/injection seront inopérantes."
    );
}

#[cfg(not(target_os = "macos"))]
fn print_permissions_hint() {}
