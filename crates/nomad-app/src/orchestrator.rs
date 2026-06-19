//! Orchestration : relie capture, réseau et injection selon le rôle.
//!
//! - [`run_server`] : le serveur est le **contrôleur**. Il capture les entrées
//!   locales, fait tourner la machine d'edge-switching ([`crate::edge`]),
//!   maintient la disposition, route les événements vers le client actif et
//!   synchronise le presse-papiers.
//! - [`run_client`] : le client est un **écran**. Il injecte les événements
//!   reçus et synchronise son presse-papiers avec le serveur.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender as StdSender;
use std::sync::Arc;

use nomad_clip::ClipCmd;
use nomad_core::layout::{Layout, NodeInfo, Screen};
use nomad_core::{InputEvent, Message, NodeId};
use nomad_input::Captured;
use nomad_net::{ClientHandle, Identity, ServerEvent, ServerHandle};
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, info, warn};

use crate::edge::{EdgeController, MoveOutcome};
use crate::inject_thread::InjectCmd;

/// Boucle d'orchestration côté **serveur / contrôleur**.
#[allow(clippy::too_many_arguments)]
pub async fn run_server(
    mut srv: ServerHandle,
    identity: Identity,
    mut capture_rx: UnboundedReceiver<Captured>,
    inject_tx: StdSender<InjectCmd>,
    clip_cmd_tx: StdSender<ClipCmd>,
    mut clip_change_rx: UnboundedReceiver<String>,
    grabbing: Arc<AtomicBool>,
) {
    let self_id = identity.id;
    let screen = identity.screen;
    let center = ((screen.width / 2) as i32, (screen.height / 2) as i32);

    // La liste des nœuds commence par le serveur lui-même (à gauche).
    let mut nodes: Vec<NodeInfo> = vec![NodeInfo {
        id: self_id,
        name: identity.name.clone(),
        os: identity.os,
        screen,
    }];
    let mut ctrl = EdgeController::new(self_id, screen, Layout::horizontal_row(nodes.clone()));

    loop {
        tokio::select! {
            Some(event) = srv.recv() => {
                handle_server_event(event, &srv, &mut nodes, &mut ctrl, &grabbing,
                                     &inject_tx, &clip_cmd_tx, center);
            }
            Some(cap) = capture_rx.recv() => {
                handle_capture(cap, &srv, &mut ctrl, &grabbing, &inject_tx,
                               self_id, screen, center);
            }
            Some(text) = clip_change_rx.recv() => {
                debug!(len = text.len(), "diffusion presse-papiers local");
                srv.broadcast(Message::Clipboard { text });
            }
            else => break,
        }
    }
    warn!("orchestrateur serveur arrêté");
}

#[allow(clippy::too_many_arguments)]
fn handle_server_event(
    event: ServerEvent,
    srv: &ServerHandle,
    nodes: &mut Vec<NodeInfo>,
    ctrl: &mut EdgeController,
    grabbing: &AtomicBool,
    inject_tx: &StdSender<InjectCmd>,
    clip_cmd_tx: &StdSender<ClipCmd>,
    center: (i32, i32),
) {
    match event {
        ServerEvent::Joined { node, name, os, screen } => {
            info!(%node, %name, "client connecté");
            nodes.retain(|n| n.id != node);
            nodes.push(NodeInfo { id: node, name, os, screen });
            let layout = Layout::horizontal_row(nodes.clone());
            ctrl.set_layout(layout.clone());
            srv.send_to(node, Message::Welcome { layout: layout.clone() });
            srv.broadcast(Message::LayoutUpdate { layout });
        }
        ServerEvent::Left { node } => {
            info!(%node, "client déconnecté");
            nodes.retain(|n| n.id != node);
            let layout = Layout::horizontal_row(nodes.clone());
            let was_active = ctrl.active() == node;
            ctrl.set_layout(layout.clone());
            if was_active {
                // On contrôlait ce client : retour forcé en local.
                grabbing.store(false, Ordering::Relaxed);
                let _ = inject_tx.send(InjectCmd::Warp(center.0, center.1));
            }
            srv.broadcast(Message::LayoutUpdate { layout });
        }
        ServerEvent::Message { from, msg } => match msg {
            Message::Clipboard { text } => {
                let _ = clip_cmd_tx.send(ClipCmd::SetText(text.clone()));
                srv.broadcast_except(from, Message::Clipboard { text });
            }
            Message::Pong => {}
            other => debug!(?other, %from, "message client ignoré"),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_capture(
    cap: Captured,
    srv: &ServerHandle,
    ctrl: &mut EdgeController,
    grabbing: &AtomicBool,
    inject_tx: &StdSender<InjectCmd>,
    self_id: NodeId,
    screen: Screen,
    center: (i32, i32),
) {
    match cap {
        Captured::MouseMoveAbs { x, y } => {
            let before = ctrl.active();
            let out = if ctrl.is_local() {
                ctrl.local_move(x, y)
            } else {
                let (cx, cy) = (center.0 as f64, center.1 as f64);
                let (dx, dy) = (x - cx, y - cy);
                if dx == 0.0 && dy == 0.0 {
                    return; // événement d'atterrissage du recentrage : ignoré
                }
                // Recentre le curseur réel pour continuer à produire des deltas.
                let _ = inject_tx.send(InjectCmd::Warp(center.0, center.1));
                ctrl.remote_advance(dx, dy)
            };
            let after = ctrl.active();
            apply_transition(srv, inject_tx, grabbing, self_id, screen, center, before, after, out);
        }
        other => {
            // Boutons / clavier / molette : transférés au client actif si on en contrôle un.
            if !ctrl.is_local() {
                if let Some(event) = to_input_event(other) {
                    srv.send_to(ctrl.active(), Message::Input { event });
                }
            }
        }
    }
}

/// Applique les conséquences d'un mouvement : transitions de nœud actif + envoi
/// de la position absolue au client distant.
#[allow(clippy::too_many_arguments)]
fn apply_transition(
    srv: &ServerHandle,
    inject_tx: &StdSender<InjectCmd>,
    grabbing: &AtomicBool,
    self_id: NodeId,
    screen: Screen,
    center: (i32, i32),
    before: NodeId,
    after: NodeId,
    out: MoveOutcome,
) {
    if before != after {
        // On quitte l'ancien écran.
        if before != self_id {
            srv.send_to(before, Message::LeaveScreen);
        }
        let now_remote = after != self_id;
        grabbing.store(now_remote, Ordering::Relaxed);

        match (now_remote, out.entry) {
            (true, Some((rx, ry))) => {
                // On commence à contrôler un client : on l'informe et on positionne
                // son curseur, puis on gare le curseur local au centre.
                srv.send_to(after, Message::EnterScreen { node: after, rx, ry });
                srv.send_to(after, Message::Input { event: InputEvent::MouseAbs { rx, ry } });
                let _ = inject_tx.send(InjectCmd::Warp(center.0, center.1));
                info!(%after, "contrôle transféré au client");
            }
            (false, Some((rx, ry))) => {
                // Retour en local : on replace le curseur réel à l'entrée.
                let x = (rx * screen.width as f64).round() as i32;
                let y = (ry * screen.height as f64).round() as i32;
                let _ = inject_tx.send(InjectCmd::Warp(x, y));
                info!("contrôle revenu en local");
            }
            _ => {}
        }
    }

    if let Some((rx, ry)) = out.remote_abs {
        srv.send_to(after, Message::Input { event: InputEvent::MouseAbs { rx, ry } });
    }
}

fn to_input_event(c: Captured) -> Option<InputEvent> {
    match c {
        Captured::MouseButton { button, pressed } => Some(InputEvent::MouseButton { button, pressed }),
        Captured::MouseWheel { dx, dy } => Some(InputEvent::MouseWheel { dx, dy }),
        Captured::Key { key, pressed } => Some(InputEvent::Key { key, pressed }),
        Captured::MouseMoveAbs { .. } => None,
    }
}

/// Boucle d'orchestration côté **client / écran**.
pub async fn run_client(
    mut cli: ClientHandle,
    inject_tx: StdSender<InjectCmd>,
    clip_cmd_tx: StdSender<ClipCmd>,
    mut clip_change_rx: UnboundedReceiver<String>,
    screen: Screen,
) {
    loop {
        tokio::select! {
            msg = cli.recv() => {
                let Some(msg) = msg else {
                    warn!("connexion au serveur perdue");
                    break;
                };
                match msg {
                    Message::Input { event } => {
                        let _ = inject_tx.send(InjectCmd::Event(event));
                    }
                    Message::EnterScreen { rx, ry, .. } => {
                        let x = (rx * screen.width as f64).round() as i32;
                        let y = (ry * screen.height as f64).round() as i32;
                        let _ = inject_tx.send(InjectCmd::Warp(x, y));
                    }
                    Message::Clipboard { text } => {
                        let _ = clip_cmd_tx.send(ClipCmd::SetText(text));
                    }
                    Message::Welcome { layout } | Message::LayoutUpdate { layout } => {
                        debug!(nodes = layout.nodes.len(), "disposition reçue");
                    }
                    Message::Ping => cli.send(Message::Pong),
                    Message::LeaveScreen => {}
                    other => debug!(?other, "message serveur ignoré"),
                }
            }
            Some(text) = clip_change_rx.recv() => {
                cli.send(Message::Clipboard { text });
            }
            else => break,
        }
    }
}
