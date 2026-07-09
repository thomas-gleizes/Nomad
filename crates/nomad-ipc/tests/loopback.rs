//! Test d'intégration : un serveur de contrôle IPC + un client sur un socket
//! Unix temporaire (sans démon réel).

use std::path::{Path, PathBuf};
use std::time::Duration;

use nomad_core::layout::Screen;
use nomad_core::status::{AppStatus, PeerInfo, Role, SharedStatus};
use nomad_core::{NodeId, Os};
use nomad_ipc::{DaemonAction, Event, Response, VERSION};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

type Reader = Lines<BufReader<OwnedReadHalf>>;

/// Connexion cliente persistante au socket de contrôle.
struct Client {
    wh: OwnedWriteHalf,
    lines: Reader,
}

impl Client {
    async fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).await.expect("connexion IPC");
        let (rh, wh) = stream.into_split();
        Self { wh, lines: BufReader::new(rh).lines() }
    }

    async fn send(&mut self, line: &str) {
        self.wh.write_all(line.as_bytes()).await.unwrap();
        self.wh.write_all(b"\n").await.unwrap();
        self.wh.flush().await.unwrap();
    }

    async fn recv_line(&mut self) -> String {
        timeout(self.lines.next_line())
            .await
            .expect("ligne IPC")
            .expect("connexion IPC non fermée")
    }

    async fn recv_response(&mut self) -> Response {
        serde_json::from_str(&self.recv_line().await).expect("Response JSON")
    }

    async fn recv_event(&mut self) -> Event {
        serde_json::from_str(&self.recv_line().await).expect("Event JSON")
    }
}

#[tokio::test]
async fn status_returns_current_state() {
    let path = temp_socket("status");
    let status = sample_status();
    let _actions = start(&path, status.clone()).await;

    let mut client = Client::connect(&path).await;
    client.send(&req("status", None)).await;
    let resp = client.recv_response().await;

    assert!(resp.ok);
    let s = resp.status.expect("status présent");
    assert_eq!(s.node_name, "atlas");
    assert_eq!(s.role, Role::Server);
    assert!(s.peers.is_empty());
}

#[tokio::test]
async fn subscribe_streams_status_changes() {
    let path = temp_socket("subscribe");
    let status = sample_status();
    let _actions = start(&path, status.clone()).await;

    let mut client = Client::connect(&path).await;
    client.send(&req("subscribe", None)).await;

    // Accusé de réception, puis l'état initial poussé immédiatement.
    assert!(client.recv_response().await.ok);
    let initial = client.recv_event().await;
    assert_eq!(initial.event, "status");
    assert!(initial.status.peers.is_empty());

    // Une mutation de l'état produit un nouvel événement.
    status.update(|st| st.peers.push(PeerInfo { id: NodeId::random(), name: "forge".into() }));
    let changed = client.recv_event().await;
    assert_eq!(changed.status.peers.len(), 1);
    assert_eq!(changed.status.peers[0].name, "forge");
}

#[tokio::test]
async fn command_routes_to_action() {
    let path = temp_socket("command");
    let status = sample_status();
    let mut actions = start(&path, status.clone()).await;

    let mut client = Client::connect(&path).await;
    client.send(&req("rename", Some("atlas2"))).await;

    // La réponse `ok` précède l'exécution de l'action.
    assert!(client.recv_response().await.ok);
    let action = timeout(actions.recv()).await.expect("action reçue");
    assert_eq!(action, DaemonAction::Rename("atlas2".into()));
}

#[tokio::test]
async fn malformed_request_keeps_connection() {
    let path = temp_socket("malformed");
    let status = sample_status();
    let _actions = start(&path, status.clone()).await;

    let mut client = Client::connect(&path).await;

    // Trame illisible → réponse d'erreur, mais la connexion survit.
    client.send("ceci n'est pas du json").await;
    let err = client.recv_response().await;
    assert!(!err.ok);
    assert!(err.error.is_some());

    // La même connexion reste utilisable.
    client.send(&req("status", None)).await;
    assert!(client.recv_response().await.ok);
}

#[tokio::test]
async fn unknown_command_is_rejected() {
    let path = temp_socket("unknown");
    let status = sample_status();
    let _actions = start(&path, status.clone()).await;

    let mut client = Client::connect(&path).await;
    client.send(&req("teleport", None)).await;
    let resp = client.recv_response().await;
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("teleport"));
}

#[tokio::test]
async fn version_mismatch_is_rejected() {
    let path = temp_socket("version");
    let status = sample_status();
    let _actions = start(&path, status.clone()).await;

    let mut client = Client::connect(&path).await;
    client.send(r#"{"v":999,"id":1,"cmd":"status"}"#).await;
    assert!(!client.recv_response().await.ok);
}

#[tokio::test]
async fn second_bind_detects_running_instance() {
    let path = temp_socket("instance");
    let status = sample_status();
    let _actions = start(&path, status.clone()).await;

    // Un second bind sur le même socket doit détecter l'instance vivante.
    match nomad_ipc::bind(&path).await {
        Err(nomad_ipc::BindError::AlreadyRunning) => {}
        Err(e) => panic!("attendu AlreadyRunning, reçu erreur {e:?}"),
        Ok(_) => panic!("attendu AlreadyRunning, mais le bind a réussi"),
    }
}

// ---- Utilitaires ----

fn sample_status() -> SharedStatus {
    SharedStatus::new(AppStatus::new(
        Role::Server,
        NodeId::random(),
        "atlas".into(),
        Os::MacOs,
        Screen::new(2560, 1440),
    ))
}

/// Lie le socket et lance le serveur ; renvoie le récepteur des actions
/// déclenchées (le callback n'exécute rien de fatal, il enregistre).
async fn start(path: &Path, status: SharedStatus) -> mpsc::UnboundedReceiver<DaemonAction> {
    let (tx, rx) = mpsc::unbounded_channel();
    let listener = nomad_ipc::bind(path).await.expect("bind IPC");
    tokio::spawn(nomad_ipc::serve(listener, status, move |action| {
        let _ = tx.send(action);
    }));
    // Laisse la boucle d'acceptation démarrer.
    tokio::time::sleep(Duration::from_millis(20)).await;
    rx
}

fn req(cmd: &str, name: Option<&str>) -> String {
    match name {
        Some(n) => format!(r#"{{"v":{VERSION},"id":1,"cmd":"{cmd}","name":"{n}"}}"#),
        None => format!(r#"{{"v":{VERSION},"id":1,"cmd":"{cmd}"}}"#),
    }
}

fn temp_socket(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("nomad-ipc-{tag}-{}-{nanos}.sock", std::process::id()))
}

async fn timeout<F: std::future::Future>(fut: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .expect("timeout en attendant une trame IPC")
}
