//! Backend systray/barre de menus (macOS + Windows) via `tao` + `tray-icon`.

use std::time::{Duration, Instant};

use nomad_core::status::{AppStatus, SharedStatus};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::UiAction;

// Identifiants des items d'action (les items d'info n'en ont pas besoin).
const ID_RENAME: &str = "nomad.rename";
const ID_FORCE_SERVER: &str = "nomad.force_server";
const ID_RECONNECT: &str = "nomad.reconnect";
const ID_QUIT: &str = "nomad.quit";

/// Intervalle de sondage de l'état partagé (reconstruction du menu si changé).
const POLL: Duration = Duration::from_millis(500);

/// Lance l'UI native sur le **thread courant** (doit être le thread principal
/// sur macOS). Ne rend jamais la main : la boucle d'événements possède le thread.
pub fn run(status: SharedStatus, mut on_action: impl FnMut(UiAction) + 'static) -> ! {
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::new().build();
    // macOS : rôle « accessoire » → pas d'icône dans le Dock, juste la barre de menus.
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }

    let menu_channel = MenuEvent::receiver();
    // Le TrayIcon doit être créé après l'initialisation de l'app (macOS) : on le
    // construit dans `StartCause::Init`.
    let mut tray: Option<TrayIcon> = None;
    let mut last_gen: Option<u64> = None;

    event_loop.run(move |event, _target, control_flow| {
        // Réveil périodique pour sonder les changements d'état.
        *control_flow = ControlFlow::WaitUntil(Instant::now() + POLL);

        if let Event::NewEvents(StartCause::Init) = event {
            let snap = status.snapshot();
            match TrayIconBuilder::new()
                .with_tooltip("nomad")
                .with_icon(app_icon())
                .with_menu(Box::new(build_menu(&snap)))
                .build()
            {
                Ok(t) => {
                    tracing::info!("icône systray créée");
                    tray = Some(t);
                }
                Err(e) => tracing::error!(error = %e, "création de l'icône systray impossible"),
            }
            last_gen = Some(status.generation());
        }

        // Reconstruit le menu si l'état a changé depuis le dernier passage.
        let gen = status.generation();
        if last_gen != Some(gen) {
            if let Some(t) = &tray {
                t.set_menu(Some(Box::new(build_menu(&status.snapshot()))));
            }
            last_gen = Some(gen);
        }

        // Traite les clics de menu.
        while let Ok(ev) = menu_channel.try_recv() {
            if let Some(action) = action_for(&ev.id, &status) {
                on_action(action);
            }
        }
    })
}

/// Traduit l'identifiant d'item cliqué en [`UiAction`] (saisie du nom incluse
/// pour « Renommer »). Renvoie `None` pour un item d'information.
fn action_for(id: &MenuId, status: &SharedStatus) -> Option<UiAction> {
    match id.as_ref() {
        ID_QUIT => Some(UiAction::Quit),
        ID_RECONNECT => Some(UiAction::Reconnect),
        ID_FORCE_SERVER => Some(UiAction::ForceServer),
        ID_RENAME => {
            let current = status.snapshot().node_name;
            prompt_rename(&current).map(UiAction::Rename)
        }
        _ => None,
    }
}

/// Dialogue natif de saisie du nouveau nom (annulation → `None`, nom vide ignoré).
fn prompt_rename(current: &str) -> Option<String> {
    tinyfiledialogs::input_box("nomad", "Nouveau nom du nœud :", current)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != current)
}

/// Construit le menu déroulant reflétant l'état courant.
fn build_menu(s: &AppStatus) -> Menu {
    let menu = Menu::new();
    // Items d'information : désactivés (non cliquables).
    let info = |text: String| MenuItem::new(text, false, None);

    let _ = menu.append(&info(format!("Rôle : {}", s.role.label())));
    let _ = menu.append(&info(format!("Nom : {}", s.node_name)));
    let _ = menu.append(&info(format!("Écran : {}×{}", s.screen.width, s.screen.height)));

    if s.peers.is_empty() {
        let _ = menu.append(&info("Pairs : aucun".to_string()));
    } else {
        let word = if s.peers.len() > 1 { "connectés" } else { "connecté" };
        let _ = menu.append(&info(format!("Pairs : {} {}", s.peers.len(), word)));
        for p in &s.peers {
            let active = s.active == Some(p.id);
            let suffix = if active { " (actif)" } else { "" };
            let _ = menu.append(&info(format!("   • {}{}", p.name, suffix)));
        }
    }
    // Côté client : signale qu'on est contrôlé à distance.
    if s.active == Some(s.self_id) {
        let _ = menu.append(&info("Contrôlé à distance".to_string()));
    }

    let _ = menu.append(&PredefinedMenuItem::separator());

    let action = |id: &str, text: &str, enabled: bool| {
        MenuItem::with_id(MenuId::new(id), text, enabled, None)
    };
    let is_server = matches!(s.role, nomad_core::status::Role::Server);
    let _ = menu.append(&action(ID_RENAME, "Renommer…", true));
    // Forcer le rôle serveur n'a de sens que si on ne l'est pas déjà.
    let _ = menu.append(&action(ID_FORCE_SERVER, "Forcer le rôle serveur", !is_server));
    let _ = menu.append(&action(ID_RECONNECT, "Reconnecter", true));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&action(ID_QUIT, "Quitter", true));

    menu
}

/// Petite icône générée en code (disque bicolore) — évite tout asset externe.
fn app_icon() -> Icon {
    const SIZE: u32 = 32;
    let c = (SIZE as f32 - 1.0) / 2.0;
    let r_out = SIZE as f32 * 0.46;
    let r_in = SIZE as f32 * 0.24;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let d = (dx * dx + dy * dy).sqrt();
            let (r, g, b, a) = if d <= r_in {
                (0xF2, 0xF6, 0xFF, 0xFF) // cœur clair
            } else if d <= r_out {
                (0x2E, 0x7D, 0xFF, 0xFF) // anneau bleu
            } else {
                (0, 0, 0, 0) // transparent
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("icône RGBA valide")
}
