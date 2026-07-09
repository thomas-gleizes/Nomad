//! Orchestration : relie capture, réseau et injection selon le rôle.
//!
//! - [`run_server`] : le serveur est le **contrôleur**. Il capture les entrées
//!   locales, fait tourner la machine d'edge-switching ([`crate::edge`]),
//!   maintient la disposition, route les événements vers le client actif et
//!   synchronise le presse-papiers.
//! - [`run_client`] : le client est un **écran**. Il injecte les événements
//!   reçus et synchronise son presse-papiers avec le serveur.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender as StdSender;
use std::sync::Arc;

use nomad_clip::ClipCmd;
use nomad_core::layout::{Layout, NodeInfo, Screen};
use nomad_core::{Button, InputEvent, Key, Message, NodeId};
use nomad_input::Captured;
use nomad_net::{ClientHandle, Identity, ServerEvent, ServerHandle};
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, info, warn};

use crate::edge::{EdgeController, MoveOutcome};
use crate::inject_thread::InjectCmd;
use crate::motion::{edge_anchor, entry_px, MotionTracker};

/// Marge (px) de ré-entrée sur l'écran local : hors de la zone de
/// déclenchement des bords, sinon le retour rebondit aussitôt vers le distant.
const REENTRY_MARGIN: f64 = 8.0;
/// Tolérance (px) de reconnaissance de l'atterrissage d'un warp de recentrage.
const WARP_TOLERANCE: f64 = 2.0;
/// Retrait (px) du point d'ancrage par rapport au bord de sortie. Garde une
/// marge suffisante pour que les mouvements de retour vers l'écran distant
/// restent mesurables entre deux warps (les positions rdev sont clampées au bord).
const EDGE_INSET: f64 = 50.0;
/// Écart (px, par axe) entre la position capturée et l'ancre au-delà duquel on
/// re-warpe le curseur réel sur le bord. Doit rester `> WARP_TOLERANCE` pour ne
/// pas re-warper indéfiniment l'atterrissage d'un warp précédent.
const ANCHOR_SLACK: f64 = 4.0;

/// État de l'orchestrateur côté **serveur / contrôleur**.
struct Server {
    srv: ServerHandle,
    self_id: NodeId,
    screen: Screen,
    center: (i32, i32),
    nodes: Vec<NodeInfo>,
    ctrl: EdgeController,
    tracker: MotionTracker,
    /// Touches/boutons dont l'appui a été transmis au client actif : leur
    /// relâchement doit partir au même endroit (sinon touche coincée).
    held_keys: HashSet<Key>,
    held_buttons: HashSet<Button>,
    grabbing: Arc<AtomicBool>,
    inject_tx: StdSender<InjectCmd>,
    clip_cmd_tx: StdSender<ClipCmd>,
}

/// Boucle d'orchestration côté **serveur / contrôleur**.
pub async fn run_server(
    srv: ServerHandle,
    identity: Identity,
    mut capture_rx: UnboundedReceiver<Captured>,
    inject_tx: StdSender<InjectCmd>,
    clip_cmd_tx: StdSender<ClipCmd>,
    mut clip_change_rx: UnboundedReceiver<String>,
    grabbing: Arc<AtomicBool>,
) {
    let self_id = identity.id;
    let screen = identity.screen;

    // La liste des nœuds commence par le serveur lui-même (à gauche).
    let nodes = vec![NodeInfo {
        id: self_id,
        name: identity.name.clone(),
        os: identity.os,
        screen,
    }];
    let mut state = Server {
        srv,
        self_id,
        screen,
        center: ((screen.width / 2) as i32, (screen.height / 2) as i32),
        ctrl: EdgeController::new(self_id, screen, Layout::horizontal_row(nodes.clone())),
        nodes,
        tracker: MotionTracker::new(WARP_TOLERANCE),
        held_keys: HashSet::new(),
        held_buttons: HashSet::new(),
        grabbing,
        inject_tx,
        clip_cmd_tx,
    };

    loop {
        tokio::select! {
            Some(event) = state.srv.recv() => state.handle_server_event(event),
            Some(cap) = capture_rx.recv() => state.handle_capture(cap),
            Some(text) = clip_change_rx.recv() => {
                debug!(len = text.len(), "diffusion presse-papiers local");
                state.srv.broadcast(Message::Clipboard { text });
            }
            else => break,
        }
    }
    warn!("orchestrateur serveur arrêté");
}

impl Server {
    fn handle_server_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Joined { node, name, os, screen } => {
                info!(%node, %name, "client connecté");
                self.nodes.retain(|n| n.id != node);
                self.nodes.push(NodeInfo { id: node, name, os, screen });
                let layout = Layout::horizontal_row(self.nodes.clone());
                self.ctrl.set_layout(layout.clone());
                self.srv.send_to(node, Message::Welcome { layout: layout.clone() });
                self.srv.broadcast(Message::LayoutUpdate { layout });
            }
            ServerEvent::Left { node } => {
                info!(%node, "client déconnecté");
                self.nodes.retain(|n| n.id != node);
                let layout = Layout::horizontal_row(self.nodes.clone());
                let was_active = self.ctrl.active() == node;
                self.ctrl.set_layout(layout.clone());
                if was_active {
                    // On contrôlait ce client : retour forcé en local.
                    self.grabbing.store(false, Ordering::Relaxed);
                    self.tracker.reset();
                    self.held_keys.clear();
                    self.held_buttons.clear();
                    let _ = self.inject_tx.send(InjectCmd::Warp(self.center.0, self.center.1));
                }
                self.srv.broadcast(Message::LayoutUpdate { layout });
            }
            ServerEvent::Message { from, msg } => match msg {
                Message::Clipboard { text } => {
                    let _ = self.clip_cmd_tx.send(ClipCmd::SetText(text.clone()));
                    self.srv.broadcast_except(from, Message::Clipboard { text });
                }
                Message::Pong => {}
                other => debug!(?other, %from, "message client ignoré"),
            },
        }
    }

    fn handle_capture(&mut self, cap: Captured) {
        match cap {
            Captured::MouseMoveAbs { x, y } => {
                let before = self.ctrl.active();
                let out = if self.ctrl.is_local() {
                    self.ctrl.local_move(x, y)
                } else {
                    // Deltas entre positions capturées successives ; les
                    // atterrissages des warps de recentrage sont avalés.
                    let Some((dx, dy)) = self.tracker.delta(x, y) else {
                        return;
                    };
                    let out = self.ctrl.remote_advance(dx, dy);
                    // Garde le curseur réel collé au bord de sortie, glissant le
                    // long de ce bord pour indiquer la position sur l'écran
                    // distant. On re-warpe seulement s'il a dérivé de l'ancre
                    // (sinon il suit déjà naturellement le mouvement). En cas de
                    // transition, `apply_transition` fait le warp d'entrée.
                    if let (Some((rx, ry)), Some(side)) = (out.remote_abs, self.ctrl.exit_side()) {
                        let (ax, ay) = edge_anchor(side, rx, ry, self.screen, EDGE_INSET);
                        if (x - ax as f64).abs() > ANCHOR_SLACK || (y - ay as f64).abs() > ANCHOR_SLACK
                        {
                            self.tracker.expect_warp(ax as f64, ay as f64);
                            let _ = self.inject_tx.send(InjectCmd::Warp(ax, ay));
                        }
                    }
                    out
                };
                let after = self.ctrl.active();
                self.apply_transition(before, after, out);
            }
            // Boutons / clavier / molette : transférés au client actif si on en contrôle un.
            Captured::Key { key, pressed } => {
                if !self.ctrl.is_local() {
                    // Un relâchement n'est transmis que si l'appui l'a été :
                    // sinon il appartient au serveur (appui antérieur à la transition).
                    let forward = if pressed {
                        self.held_keys.insert(key);
                        true
                    } else {
                        self.held_keys.remove(&key)
                    };
                    if forward {
                        self.srv
                            .send_to(self.ctrl.active(), Message::Input { event: InputEvent::Key { key, pressed } });
                    }
                }
            }
            Captured::MouseButton { button, pressed } => {
                if !self.ctrl.is_local() {
                    let forward = if pressed {
                        self.held_buttons.insert(button);
                        true
                    } else {
                        self.held_buttons.remove(&button)
                    };
                    if forward {
                        self.srv.send_to(
                            self.ctrl.active(),
                            Message::Input { event: InputEvent::MouseButton { button, pressed } },
                        );
                    }
                }
            }
            Captured::MouseWheel { dx, dy } => {
                if !self.ctrl.is_local() {
                    self.srv
                        .send_to(self.ctrl.active(), Message::Input { event: InputEvent::MouseWheel { dx, dy } });
                }
            }
        }
    }

    /// Applique les conséquences d'un mouvement : transitions de nœud actif +
    /// envoi de la position absolue au client distant.
    fn apply_transition(&mut self, before: NodeId, after: NodeId, out: MoveOutcome) {
        if before != after {
            if before != self.self_id {
                // On quitte l'ancien écran : on y relâche tout ce qui est
                // encore appuyé, sinon touche/bouton y restent coincés.
                self.release_held(before);
                self.srv.send_to(before, Message::LeaveScreen);
            }
            let now_remote = after != self.self_id;
            self.grabbing.store(now_remote, Ordering::Relaxed);
            self.tracker.reset();

            match (now_remote, out.entry) {
                (true, Some((rx, ry))) => {
                    // On commence à contrôler un client : on l'informe (il
                    // positionne son curseur) et on gare le curseur local collé
                    // au bord de sortie, à la même hauteur/largeur, pour indiquer
                    // la position sur l'écran distant.
                    self.srv.send_to(after, Message::EnterScreen { node: after, rx, ry });
                    let (ax, ay) = match self.ctrl.exit_side() {
                        Some(side) => edge_anchor(side, rx, ry, self.screen, EDGE_INSET),
                        None => self.center,
                    };
                    self.tracker.expect_warp(ax as f64, ay as f64);
                    let _ = self.inject_tx.send(InjectCmd::Warp(ax, ay));
                    info!(%after, "contrôle transféré au client");
                }
                (false, Some((rx, ry))) => {
                    // Retour en local : curseur replacé en retrait du bord.
                    let x = entry_px(rx, self.screen.width, REENTRY_MARGIN);
                    let y = entry_px(ry, self.screen.height, REENTRY_MARGIN);
                    let _ = self.inject_tx.send(InjectCmd::Warp(x, y));
                    info!("contrôle revenu en local");
                }
                _ => {}
            }
        }

        if let Some((rx, ry)) = out.remote_abs {
            self.srv.send_to(after, Message::Input { event: InputEvent::MouseAbs { rx, ry } });
        }
    }

    /// Envoie à `node` le relâchement de toutes les touches/boutons transmis
    /// encore appuyés, puis oublie cet état.
    fn release_held(&mut self, node: NodeId) {
        for key in self.held_keys.drain() {
            self.srv.send_to(node, Message::Input { event: InputEvent::Key { key, pressed: false } });
        }
        for button in self.held_buttons.drain() {
            self.srv
                .send_to(node, Message::Input { event: InputEvent::MouseButton { button, pressed: false } });
        }
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
                        // Coordonnées valides : 0..=width-1.
                        let x = (rx * screen.width.saturating_sub(1) as f64).round() as i32;
                        let y = (ry * screen.height.saturating_sub(1) as f64).round() as i32;
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
