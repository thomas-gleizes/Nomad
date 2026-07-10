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
mod known;
mod motion;
mod orchestrator;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use nomad_core::layout::Screen;
use nomad_core::status::{AppStatus, Role, SharedStatus};
use nomad_core::{Button, Key, Os};
use nomad_input::Captured;
use nomad_ipc::DaemonAction;
use nomad_net::{Endpoint, Identity};
use nomad_ui::UiAction;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::inject_thread::InjectCmd;
use crate::orchestrator::ControlCmd;

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
    /// Chemin du socket de l'API de contrôle (par défaut : à côté de la config).
    #[arg(long)]
    ipc_socket: Option<PathBuf>,
    /// Désactiver l'API de contrôle locale (socket IPC).
    #[arg(long)]
    no_ipc: bool,
}

/// Exécute les actions de contrôle, quelle qu'en soit la source (menu du tray
/// ou API IPC). Toutes passent par une relance propre du process — réutilisant
/// tout le chemin de démarrage — plutôt qu'une reconfiguration à chaud.
///
/// Clonable et `Send + Sync` : une copie va au tray (thread principal), une
/// autre au serveur IPC (runtime tokio).
#[derive(Clone)]
struct ActionHandler {
    cfg: Config,
    config_path: PathBuf,
    /// Arguments de lancement d'origine (conservent le rôle courant).
    original_args: Vec<String>,
    /// Arguments d'origine privés de `--server` (pour re-décider du rôle).
    base_args: Vec<String>,
    /// Vers l'orchestrateur serveur, pour les commandes à chaud (`forget`).
    control_tx: UnboundedSender<ControlCmd>,
}

impl ActionHandler {
    fn new(cfg: Config, config_path: PathBuf, control_tx: UnboundedSender<ControlCmd>) -> Self {
        let original_args: Vec<String> = std::env::args().skip(1).collect();
        let base_args = original_args
            .iter()
            .filter(|a| a.as_str() != "--server")
            .cloned()
            .collect();
        Self { cfg, config_path, original_args, base_args, control_tx }
    }

    fn handle(&self, action: DaemonAction) {
        match action {
            DaemonAction::Quit => std::process::exit(0),
            DaemonAction::Reconnect => relaunch(&self.base_args),
            DaemonAction::ForceServer => {
                let mut a = self.base_args.clone();
                a.push("--server".into());
                relaunch(&a)
            }
            DaemonAction::Rename(name) => {
                // Relire depuis le disque avant d'écrire : l'orchestrateur peut
                // avoir persisté des `known_peers` depuis le démarrage.
                let mut c = Config::load_or_create(&self.config_path)
                    .unwrap_or_else(|_| self.cfg.clone());
                c.name = name;
                if let Err(e) = c.save(&self.config_path) {
                    error!(error = %e, "sauvegarde du nom impossible");
                }
                relaunch(&self.original_args) // conserve le rôle courant
            }
            DaemonAction::Forget(id) => {
                // Commande à chaud : pas de relance. Sans serveur actif (rôle
                // client), le récepteur est absent — on ignore.
                if self.control_tx.send(ControlCmd::Forget(id)).is_err() {
                    warn!("commande « oublier » ignorée (aucun orchestrateur serveur)");
                }
            }
        }
    }
}

/// Traduit une action du menu tray vers l'action de contrôle unifiée.
fn ui_to_daemon(action: UiAction) -> DaemonAction {
    match action {
        UiAction::Quit => DaemonAction::Quit,
        UiAction::Reconnect => DaemonAction::Reconnect,
        UiAction::ForceServer => DaemonAction::ForceServer,
        UiAction::Rename(name) => DaemonAction::Rename(name),
    }
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

    // Canal de contrôle à chaud : le handler y pousse (`forget`), l'orchestrateur
    // serveur le consomme. En rôle client, le récepteur est simplement ignoré.
    let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel::<ControlCmd>();

    // Handler d'actions partagé entre le tray et l'API IPC.
    let handler = ActionHandler::new(cfg.clone(), config_path.clone(), control_tx);

    // API de contrôle IPC : liée **tôt** (avant réseau/capture) afin de détecter
    // une instance déjà en cours et de sortir proprement. Le serveur lui-même est
    // démarré plus bas, une fois l'état partagé construit.
    let ipc_enabled = nomad_ipc::SUPPORTED && !cli.no_ipc;
    let ipc_listener = if ipc_enabled {
        let path = resolve_ipc_socket(&cli, &config_path);
        match rt.block_on(nomad_ipc::bind(&path)) {
            Ok(listener) => {
                info!(socket = %path.display(), "API de contrôle IPC liée");
                Some(listener)
            }
            Err(nomad_ipc::BindError::AlreadyRunning) => {
                eprintln!(
                    "nomad est déjà en cours d'exécution (socket {}). Une seule instance à la fois.",
                    path.display()
                );
                std::process::exit(3);
            }
            Err(e) => {
                warn!(error = %e, "API de contrôle IPC indisponible, poursuite sans");
                None
            }
        }
    } else {
        None
    };

    // État partagé lu par l'UI et l'API IPC. Construit **avant** la découverte
    // (bloquante) avec un rôle provisoire, pour que l'API de contrôle réponde
    // immédiatement — en particulier à la sonde d'instance unique d'un second
    // process. Le rôle réel est corrigé juste après l'élection.
    let provisional_role = if cli.server { Role::Server } else { Role::Client };
    let status = SharedStatus::new(AppStatus::new(
        provisional_role,
        identity.id,
        identity.name.clone(),
        identity.os,
        screen,
    ));

    // Démarre le serveur de l'API de contrôle sur le socket déjà lié. Disponible
    // dans les deux rôles et en headless.
    if let Some(listener) = ipc_listener {
        let status = status.clone();
        let handler = handler.clone();
        rt.spawn(nomad_ipc::serve(listener, status, move |action| handler.handle(action)));
    }

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

    // Corrige le rôle provisoire une fois l'élection faite ; l'orchestrateur
    // continue ensuite de mettre à jour l'état (pairs, écran actif).
    let role = match &endpoint {
        Endpoint::Server(_) => Role::Server,
        Endpoint::Client { .. } => Role::Client,
    };
    status.update(|st| st.role = role);

    // UI native seulement si la plateforme la supporte et qu'on n'est pas en headless.
    let ui_enabled = nomad_ui::SUPPORTED && !cli.headless;

    // Actions du menu tray : relayées vers le même handler que l'IPC.
    let on_action = {
        let handler = handler.clone();
        move |action: UiAction| handler.handle(ui_to_daemon(action))
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
                control_rx,
                config_path.clone(),
                cfg.known_peers.clone(),
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
        Endpoint::Client { handle, server } => {
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
                Some(server.addr.to_string()),
                server.node_id,
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

/// Résout le chemin du socket IPC : `--ipc-socket` si fourni, sinon à côté de la
/// configuration. Repli sur le répertoire temporaire si le chemin dépasse la
/// limite `sun_path` des sockets Unix (~104 octets), fréquente sous macOS où la
/// config vit dans `~/Library/Application Support`.
fn resolve_ipc_socket(cli: &Cli, config_path: &Path) -> PathBuf {
    if let Some(path) = &cli.ipc_socket {
        return path.clone();
    }
    let default = config_path
        .parent()
        .map(|p| p.join("nomad.sock"))
        .or_else(Config::default_socket_path)
        .unwrap_or_else(|| PathBuf::from("nomad.sock"));
    if default.as_os_str().len() > 100 {
        let fallback = std::env::temp_dir().join("nomad.sock");
        warn!(
            path = %default.display(),
            fallback = %fallback.display(),
            "chemin de socket trop long, repli sur le répertoire temporaire"
        );
        fallback
    } else {
        default
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
