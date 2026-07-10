//! Orchestration : relie capture, réseau et injection selon le rôle.
//!
//! - [`run_server`] : le serveur est le **contrôleur**. Il capture les entrées
//!   locales, fait tourner la machine d'edge-switching ([`crate::edge`]),
//!   maintient la disposition, route les événements vers le client actif et
//!   synchronise le presse-papiers.
//! - [`run_client`] : le client est un **écran**. Il injecte les événements
//!   reçus et synchronise son presse-papiers avec le serveur.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender as StdSender;
use std::sync::Arc;
use std::time::Instant;

use nomad_clip::ClipCmd;
use nomad_core::layout::{Layout, NodeInfo, Screen};
use nomad_core::status::{PeerInfo, SharedStatus};
use nomad_core::{Button, InputEvent, Key, KnownPeer, Message, NodeId};
use nomad_input::Captured;
use nomad_net::{ClientHandle, Identity, ServerEvent, ServerHandle};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::edge::{EdgeController, MoveOutcome};
use crate::inject_thread::InjectCmd;
use crate::known;
use crate::motion::{edge_anchor, entry_px, MotionTracker};

/// Intervalle de mesure de latence (Ping applicatif), dans les deux sens.
const PING_INTERVAL: Duration = Duration::from_secs(5);

/// Commande de contrôle à chaud (via l'API IPC) traitée par l'orchestrateur
/// serveur sans relance du process.
#[derive(Debug)]
pub enum ControlCmd {
    /// Oublier une machine connue actuellement déconnectée.
    Forget(NodeId),
}

/// Secondes epoch courantes (pour horodater la dernière présence d'un pair).
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

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
    status: SharedStatus,
    /// Adresse réseau de chaque client connecté (jamais diffusée sur le réseau).
    peer_addrs: HashMap<NodeId, SocketAddr>,
    /// Ping en vol vers chaque client (au plus un), pour le calcul du RTT.
    pings: HashMap<NodeId, Instant>,
    /// Dernière latence mesurée par client, en ms.
    latencies: HashMap<NodeId, u32>,
    /// Machines déjà vues (connectées ou non) ; persistées en config.
    known: Vec<KnownPeer>,
    /// Chemin de config, pour persister `known`.
    config_path: PathBuf,
}

/// Boucle d'orchestration côté **serveur / contrôleur**.
#[allow(clippy::too_many_arguments)]
pub async fn run_server(
    srv: ServerHandle,
    identity: Identity,
    mut capture_rx: UnboundedReceiver<Captured>,
    inject_tx: StdSender<InjectCmd>,
    clip_cmd_tx: StdSender<ClipCmd>,
    mut clip_change_rx: UnboundedReceiver<String>,
    grabbing: Arc<AtomicBool>,
    status: SharedStatus,
    mut control_rx: UnboundedReceiver<ControlCmd>,
    config_path: PathBuf,
    known: Vec<KnownPeer>,
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
        status,
        peer_addrs: HashMap::new(),
        pings: HashMap::new(),
        latencies: HashMap::new(),
        known,
        config_path,
    };

    // Publie l'état initial des machines connues (toutes hors ligne au démarrage).
    state.sync_status_known();

    let mut ping_timer = interval(PING_INTERVAL);

    loop {
        tokio::select! {
            Some(event) = state.srv.recv() => state.handle_server_event(event),
            Some(cap) = capture_rx.recv() => state.handle_capture(cap),
            Some(text) = clip_change_rx.recv() => {
                debug!(len = text.len(), "diffusion presse-papiers local");
                state.srv.broadcast(Message::Clipboard { text });
            }
            Some(cmd) = control_rx.recv() => state.handle_control(cmd),
            _ = ping_timer.tick() => state.send_pings(),
        }
    }
}

impl Server {
    fn handle_server_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Joined { node, name, os, screen, addr } => {
                info!(%node, %name, "client connecté");
                self.nodes.retain(|n| n.id != node);
                self.nodes.push(NodeInfo { id: node, name: name.clone(), os, screen });
                if let Some(a) = addr {
                    self.peer_addrs.insert(node, a);
                }
                known::record_seen(
                    &mut self.known,
                    node,
                    name,
                    os,
                    addr.map(|a| a.to_string()),
                    now_unix(),
                );
                self.persist_known();
                let layout = Layout::horizontal_row(self.nodes.clone());
                self.ctrl.set_layout(layout.clone());
                self.srv.send_to(node, Message::Welcome { layout: layout.clone() });
                self.srv.broadcast(Message::LayoutUpdate { layout });
                self.sync_status_peers();
                self.sync_status_known();
            }
            ServerEvent::Left { node } => {
                info!(%node, "client déconnecté");
                // Horodate la dernière présence avant de retirer le nœud.
                if let Some(n) = self.nodes.iter().find(|n| n.id == node) {
                    let addr = self.peer_addrs.get(&node).map(|a| a.to_string());
                    known::record_seen(&mut self.known, node, n.name.clone(), n.os, addr, now_unix());
                    self.persist_known();
                }
                self.nodes.retain(|n| n.id != node);
                self.peer_addrs.remove(&node);
                self.pings.remove(&node);
                self.latencies.remove(&node);
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
                self.sync_status_peers();
                self.sync_status_known();
                if was_active {
                    self.sync_status_active();
                }
            }
            ServerEvent::Message { from, msg } => match msg {
                Message::Clipboard { text } => {
                    let _ = self.clip_cmd_tx.send(ClipCmd::SetText(text.clone()));
                    self.srv.broadcast_except(from, Message::Clipboard { text });
                }
                // Un client mesure sa latence : on lui renvoie le Pong.
                Message::Ping => self.srv.send_to(from, Message::Pong),
                // Réponse à notre propre Ping : calcul du RTT.
                Message::Pong => {
                    if let Some(sent) = self.pings.remove(&from) {
                        let rtt = sent.elapsed().as_millis().min(u32::MAX as u128) as u32;
                        self.latencies.insert(from, rtt);
                        self.sync_status_peers();
                    }
                }
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
            self.sync_status_active();

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

    /// Recopie la liste des pairs (tous les nœuds hors soi-même) dans l'état
    /// partagé lu par l'UI, enrichie de l'adresse et de la latence connues.
    fn sync_status_peers(&self) {
        let peers: Vec<PeerInfo> = self
            .nodes
            .iter()
            .filter(|n| n.id != self.self_id)
            .map(|n| PeerInfo {
                id: n.id,
                name: n.name.clone(),
                os: n.os,
                screen: n.screen,
                addr: self.peer_addrs.get(&n.id).map(|a| a.to_string()),
                latency_ms: self.latencies.get(&n.id).copied(),
            })
            .collect();
        self.status.update(|st| st.peers = peers.clone());
    }

    /// Recalcule les machines hors ligne (connues − connectées) dans l'état.
    fn sync_status_known(&self) {
        let connected: HashSet<NodeId> = self.nodes.iter().map(|n| n.id).collect();
        let offline = known::offline(&self.known, &connected);
        self.status.update(|st| st.known_offline = offline.clone());
    }

    /// Envoie un Ping à chaque client connecté et note l'instant d'émission
    /// (au plus un ping en vol par client — un ping non répondu est écrasé).
    fn send_pings(&mut self) {
        let now = Instant::now();
        let targets: Vec<NodeId> = self
            .nodes
            .iter()
            .map(|n| n.id)
            .filter(|id| *id != self.self_id)
            .collect();
        for node in targets {
            self.pings.insert(node, now);
            self.srv.send_to(node, Message::Ping);
        }
    }

    /// Traite une commande de contrôle à chaud (API IPC).
    fn handle_control(&mut self, cmd: ControlCmd) {
        match cmd {
            ControlCmd::Forget(id) => {
                if known::forget(&mut self.known, id) {
                    info!(%id, "machine oubliée");
                    self.persist_known();
                    self.sync_status_known();
                }
            }
        }
    }

    /// Persiste la liste des machines connues dans la config. On **relit** le
    /// fichier avant d'écrire pour ne pas écraser un autre champ (ex. le nom)
    /// modifié entre-temps par une autre voie.
    fn persist_known(&self) {
        match Config::load_or_create(&self.config_path) {
            Ok(mut c) => {
                c.known_peers = self.known.clone();
                if let Err(e) = c.save(&self.config_path) {
                    warn!(error = %e, "sauvegarde des machines connues impossible");
                }
            }
            Err(e) => warn!(error = %e, "relecture de la config impossible (machines connues)"),
        }
    }

    /// Recopie le nœud actif (`None` si local) dans l'état partagé.
    fn sync_status_active(&self) {
        let active = self.ctrl.active();
        let active = (active != self.self_id).then_some(active);
        self.status.update(|st| st.active = active);
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

/// Construit la liste des pairs côté client à partir de la disposition reçue,
/// en attribuant la latence mesurée au pair identifié comme serveur.
fn client_peers(
    nodes: &[NodeInfo],
    self_id: NodeId,
    server_id: Option<NodeId>,
    server_latency: Option<u32>,
) -> Vec<PeerInfo> {
    nodes
        .iter()
        .filter(|n| n.id != self_id)
        .map(|n| PeerInfo {
            id: n.id,
            name: n.name.clone(),
            os: n.os,
            screen: n.screen,
            addr: None, // un client ne connaît pas les adresses des autres
            latency_ms: (server_id == Some(n.id)).then_some(server_latency).flatten(),
        })
        .collect()
}

/// Boucle d'orchestration côté **client / écran**.
#[allow(clippy::too_many_arguments)]
pub async fn run_client(
    mut cli: ClientHandle,
    inject_tx: StdSender<InjectCmd>,
    clip_cmd_tx: StdSender<ClipCmd>,
    mut clip_change_rx: UnboundedReceiver<String>,
    screen: Screen,
    self_id: NodeId,
    status: SharedStatus,
    server_addr: Option<String>,
    server_id: Option<NodeId>,
) {
    // Adresse du serveur, connue dès la connexion.
    status.update(|st| st.server_addr = server_addr.clone());

    let mut layout_nodes: Vec<NodeInfo> = Vec::new();
    let mut server_latency: Option<u32> = None;
    let mut ping_sent: Option<Instant> = None;
    let mut ping_timer = interval(PING_INTERVAL);

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
                        // On est désormais contrôlé à distance.
                        status.update(|st| st.active = Some(self_id));
                    }
                    Message::Clipboard { text } => {
                        let _ = clip_cmd_tx.send(ClipCmd::SetText(text));
                    }
                    Message::Welcome { layout } | Message::LayoutUpdate { layout } => {
                        debug!(nodes = layout.nodes.len(), "disposition reçue");
                        layout_nodes = layout.nodes;
                        let peers = client_peers(&layout_nodes, self_id, server_id, server_latency);
                        status.update(|st| st.peers = peers.clone());
                    }
                    // Réponse à notre propre Ping : latence vers le serveur.
                    Message::Pong => {
                        if let Some(sent) = ping_sent.take() {
                            server_latency = Some(sent.elapsed().as_millis().min(u32::MAX as u128) as u32);
                            let peers = client_peers(&layout_nodes, self_id, server_id, server_latency);
                            status.update(|st| st.peers = peers.clone());
                        }
                    }
                    Message::Ping => cli.send(Message::Pong),
                    Message::LeaveScreen => {
                        // Le contrôle distant nous quitte.
                        status.update(|st| st.active = None);
                    }
                    other => debug!(?other, "message serveur ignoré"),
                }
            }
            Some(text) = clip_change_rx.recv() => {
                cli.send(Message::Clipboard { text });
            }
            _ = ping_timer.tick() => {
                ping_sent = Some(Instant::now());
                cli.send(Message::Ping);
            }
        }
    }
}
