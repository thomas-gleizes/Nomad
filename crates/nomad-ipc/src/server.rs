//! Serveur de l'API de contrôle sur **socket Unix** (`tokio::net::UnixListener`).
//!
//! `nomad-app` appelle [`bind`] tôt (détection d'instance unique, avant même la
//! mise en place réseau/capture), puis [`serve`] dans le runtime tokio. Le
//! serveur est purement *lecteur* de l'état : il expose [`SharedStatus`] via des
//! réponses et un flux d'événements, et relaie les commandes au démon via un
//! callback `on_action`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nomad_core::status::ScreenGeom;
use nomad_core::{first_overlap, NodeId, Rect, SharedStatus};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tracing::{debug, error};

use crate::protocol::{DaemonAction, Event, Request, Response, VERSION};

/// Intervalle de sondage de la génération de l'état (mêmes principes que le
/// tray : on ne pousse un événement que lorsque l'état change réellement).
const POLL: Duration = Duration::from_millis(250);

/// Échec de mise en place du socket de contrôle.
#[derive(Debug, thiserror::Error)]
pub enum BindError {
    /// Un démon `nomad` répond déjà sur ce socket : une seule instance à la fois.
    #[error("une instance de nomad est déjà en cours d'exécution")]
    AlreadyRunning,
    /// Erreur d'E/S à l'ouverture du socket.
    #[error("ouverture du socket IPC impossible: {0}")]
    Io(#[from] std::io::Error),
}

/// Socket de contrôle lié, prêt à être servi. Supprime le fichier socket à la
/// destruction (utile en tests ; à l'arrêt normal du démon le process sort sans
/// dérouler la pile, d'où le nettoyage des sockets orphelins par [`bind`]).
pub struct Listener {
    inner: UnixListener,
    path: PathBuf,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Lie le socket de contrôle, en garantissant l'**instance unique**.
///
/// Si un socket existe déjà : on tente de s'y connecter. Une connexion qui
/// aboutit signale un démon vivant → [`BindError::AlreadyRunning`]. Sinon le
/// socket est orphelin (démon mort sans nettoyage) : on le supprime et on lie.
pub async fn bind(path: &Path) -> Result<Listener, BindError> {
    if path.exists() {
        match UnixStream::connect(path).await {
            Ok(_) => return Err(BindError::AlreadyRunning),
            Err(_) => {
                // Socket orphelin : le retirer avant de (re)lier.
                let _ = std::fs::remove_file(path);
            }
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let inner = UnixListener::bind(path)?;
    Ok(Listener { inner, path: path.to_path_buf() })
}

/// Sert le socket jusqu'à l'arrêt. Chaque connexion est traitée par une tâche
/// dédiée ; une tâche de sondage diffuse les changements d'état aux abonnés.
pub async fn serve<F>(listener: Listener, status: SharedStatus, on_action: F) -> anyhow::Result<()>
where
    F: Fn(DaemonAction) + Send + Sync + 'static,
{
    let shared = Arc::new(Shared {
        subscribers: Mutex::new(Vec::new()),
        on_action: Arc::new(on_action),
    });

    tokio::spawn(poll_status(status.clone(), shared.clone()));

    loop {
        match listener.inner.accept().await {
            Ok((stream, _addr)) => {
                let status = status.clone();
                let shared = shared.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, status, shared).await {
                        debug!(error = %e, "connexion IPC terminée");
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "accept() IPC en échec, arrêt du serveur de contrôle");
                break;
            }
        }
    }
    Ok(())
}

/// État partagé entre les connexions et la tâche de sondage.
struct Shared {
    /// Émetteurs des connexions abonnées (`subscribe`). Purgés à la première
    /// écriture en échec (connexion fermée).
    subscribers: Mutex<Vec<mpsc::UnboundedSender<Out>>>,
    on_action: Arc<dyn Fn(DaemonAction) + Send + Sync>,
}

/// Message vers la tâche d'écriture d'une connexion.
#[derive(Debug)]
enum Out {
    /// Écrire cette ligne.
    Line(String),
    /// Écrire cette ligne **puis** exécuter l'action. L'écriture (avec flush)
    /// précède l'action : la réponse `ok` atteint le client avant une éventuelle
    /// relance du process qui couperait la connexion.
    LineThenAction(String, DaemonAction),
}

/// Boucle de sondage : pousse un événement `status` aux abonnés quand la
/// génération change.
async fn poll_status(status: SharedStatus, shared: Arc<Shared>) {
    let mut last = status.generation();
    loop {
        tokio::time::sleep(POLL).await;
        let gen = status.generation();
        if gen == last {
            continue;
        }
        last = gen;
        let line = encode(&Event::status(status.snapshot()));
        let mut subs = shared.subscribers.lock().unwrap();
        subs.retain(|tx| tx.send(Out::Line(line.clone())).is_ok());
    }
}

async fn handle_conn(
    stream: UnixStream,
    status: SharedStatus,
    shared: Arc<Shared>,
) -> anyhow::Result<()> {
    let (rh, mut wh) = stream.into_split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Out>();

    // Tâche d'écriture dédiée : sérialise les lignes sortantes et exécute les
    // actions après flush.
    let on_action = shared.on_action.clone();
    let writer = tokio::spawn(async move {
        while let Some(item) = out_rx.recv().await {
            let (line, action) = match item {
                Out::Line(l) => (l, None),
                Out::LineThenAction(l, a) => (l, Some(a)),
            };
            if wh.write_all(line.as_bytes()).await.is_err()
                || wh.write_all(b"\n").await.is_err()
                || wh.flush().await.is_err()
            {
                break;
            }
            if let Some(action) = action {
                on_action(action);
            }
        }
    });

    let mut lines = BufReader::new(rh).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let mut outgoing: Vec<Out> = Vec::new();

        match serde_json::from_str::<Request>(&line) {
            Err(e) => {
                // `id` inconnu (trame illisible) : on répond avec id 0.
                outgoing.push(line_out(&Response::error(0, format!("JSON invalide: {e}"))));
            }
            Ok(req) if req.v != VERSION => {
                outgoing.push(line_out(&Response::error(
                    req.id,
                    format!("version {} non supportée (attendu {VERSION})", req.v),
                )));
            }
            Ok(req) => match req.cmd.as_str() {
                "status" => {
                    outgoing.push(line_out(&Response::with_status(req.id, status.snapshot())));
                }
                "subscribe" => {
                    shared.subscribers.lock().unwrap().push(out_tx.clone());
                    outgoing.push(line_out(&Response::ok(req.id)));
                    outgoing.push(line_out(&Event::status(status.snapshot())));
                }
                "rename" => {
                    match req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                        Some(name) => outgoing.push(Out::LineThenAction(
                            encode(&Response::ok(req.id)),
                            DaemonAction::Rename(name.to_string()),
                        )),
                        None => outgoing
                            .push(line_out(&Response::error(req.id, "nom vide ou manquant"))),
                    }
                }
                "forget" => {
                    match req.node.as_deref().and_then(|s| s.parse::<NodeId>().ok()) {
                        Some(id) => outgoing.push(Out::LineThenAction(
                            encode(&Response::ok(req.id)),
                            DaemonAction::Forget(id),
                        )),
                        None => outgoing.push(line_out(&Response::error(
                            req.id,
                            "identifiant de nœud invalide ou manquant",
                        ))),
                    }
                }
                "set_layout" => {
                    // Validé ici (contre l'état courant) pour répondre ok/erreur
                    // avant d'appliquer ; l'orchestrateur revalide par sécurité.
                    match validate_set_layout(&status.snapshot().layout, req.layout.as_deref()) {
                        Ok(entries) => outgoing.push(Out::LineThenAction(
                            encode(&Response::ok(req.id)),
                            DaemonAction::SetLayout(entries),
                        )),
                        Err(e) => outgoing.push(line_out(&Response::error(req.id, e))),
                    }
                }
                "force_server" => outgoing.push(Out::LineThenAction(
                    encode(&Response::ok(req.id)),
                    DaemonAction::ForceServer,
                )),
                "reconnect" => outgoing.push(Out::LineThenAction(
                    encode(&Response::ok(req.id)),
                    DaemonAction::Reconnect,
                )),
                "quit" => outgoing.push(Out::LineThenAction(
                    encode(&Response::ok(req.id)),
                    DaemonAction::Quit,
                )),
                other => outgoing
                    .push(line_out(&Response::error(req.id, format!("commande inconnue: {other}")))),
            },
        }

        for item in outgoing {
            if out_tx.send(item).is_err() {
                // Tâche d'écriture disparue : connexion fermée.
                writer.abort();
                return Ok(());
            }
        }
    }

    writer.abort();
    Ok(())
}

/// Sérialise une valeur en ligne JSON. Les types du protocole ne contiennent
/// que des structures sérialisables sans faille (pas de clés non-chaîne) : un
/// échec traduirait un bug, d'où le `expect`.
fn encode<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("sérialisation JSON du protocole IPC")
}

fn line_out<T: Serialize>(value: &T) -> Out {
    Out::Line(encode(value))
}

/// Valide une commande `set_layout` contre la géométrie courante (`geom`).
///
/// Vérifie que chaque entrée référence un écran connu, que son UUID est valide,
/// et que le résultat fusionné ne produit aucun chevauchement. Renvoie les
/// entrées prêtes à appliquer, ou un message d'erreur lisible.
fn validate_set_layout(
    geom: &[ScreenGeom],
    entries: Option<&[crate::protocol::LayoutEntryDTO]>,
) -> Result<Vec<(NodeId, i32, i32)>, String> {
    let entries = entries.ok_or("champ « layout » manquant")?;
    if geom.is_empty() {
        return Err("aucune disposition connue (démon en cours de connexion ?)".into());
    }

    let mut parsed = Vec::with_capacity(entries.len());
    // Positions résultantes par nœud (défaut = position actuelle).
    let mut merged: std::collections::HashMap<NodeId, (i32, i32)> =
        geom.iter().map(|g| (g.id, (g.x, g.y))).collect();

    for e in entries {
        let id: NodeId = e.node.parse().map_err(|_| format!("UUID invalide : {}", e.node))?;
        if !merged.contains_key(&id) {
            return Err(format!("écran inconnu : {id}"));
        }
        merged.insert(id, (e.x, e.y));
        parsed.push((id, e.x, e.y));
    }

    // Chevauchements sur l'ensemble des écrans connus, avec leurs tailles.
    let rects: Vec<(NodeId, Rect)> = geom
        .iter()
        .map(|g| {
            let &(x, y) = merged.get(&g.id).unwrap_or(&(g.x, g.y));
            (g.id, Rect { x, y, w: g.width, h: g.height })
        })
        .collect();
    if let Some((a, b)) = first_overlap(&rects) {
        return Err(format!("les écrans {a} et {b} se chevauchent"));
    }

    Ok(parsed)
}
