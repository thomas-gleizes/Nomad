//! Client de test de l'API de contrôle IPC.
//!
//! ```sh
//! cargo run -p nomad-ipc --example ipcctl -- status
//! cargo run -p nomad-ipc --example ipcctl -- watch
//! cargo run -p nomad-ipc --example ipcctl -- rename atlas
//! cargo run -p nomad-ipc --example ipcctl -- force-server
//! cargo run -p nomad-ipc --example ipcctl -- reconnect
//! cargo run -p nomad-ipc --example ipcctl -- quit
//! ```
//!
//! Socket : `--socket <chemin>` sinon l'emplacement par défaut (à côté de la
//! config, `~/Library/Application Support/dev.nomad.nomad/nomad.sock` sur macOS).

use std::path::PathBuf;

use nomad_ipc::VERSION;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    let socket = take_option(&mut args, "--socket")
        .map(PathBuf::from)
        .or_else(default_socket)
        .ok_or_else(|| anyhow::anyhow!("chemin de socket introuvable, utilisez --socket"))?;

    let cmd = args.first().cloned().unwrap_or_else(|| "status".into());

    let stream = UnixStream::connect(&socket).await.map_err(|e| {
        anyhow::anyhow!("connexion à {} impossible: {e} (le démon tourne-t-il ?)", socket.display())
    })?;
    let (rh, mut wh) = stream.into_split();
    let mut lines = BufReader::new(rh).lines();

    let request = match cmd.as_str() {
        "status" => format!(r#"{{"v":{VERSION},"id":1,"cmd":"status"}}"#),
        "watch" => format!(r#"{{"v":{VERSION},"id":1,"cmd":"subscribe"}}"#),
        "rename" => {
            let name = args.get(1).ok_or_else(|| anyhow::anyhow!("usage: rename <nom>"))?;
            format!(r#"{{"v":{VERSION},"id":1,"cmd":"rename","name":"{name}"}}"#)
        }
        "force-server" => format!(r#"{{"v":{VERSION},"id":1,"cmd":"force_server"}}"#),
        "reconnect" => format!(r#"{{"v":{VERSION},"id":1,"cmd":"reconnect"}}"#),
        "quit" => format!(r#"{{"v":{VERSION},"id":1,"cmd":"quit"}}"#),
        other => anyhow::bail!("commande inconnue: {other}"),
    };

    wh.write_all(request.as_bytes()).await?;
    wh.write_all(b"\n").await?;
    wh.flush().await?;

    // `watch` : boucle jusqu'à Ctrl-C. Les autres : une réponse suffit.
    if cmd == "watch" {
        while let Some(line) = lines.next_line().await? {
            println!("{line}");
        }
    } else if let Some(line) = lines.next_line().await? {
        println!("{line}");
    }
    Ok(())
}

/// Extrait `--opt <valeur>` de la liste d'arguments (et les en retire).
fn take_option(args: &mut Vec<String>, opt: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == opt)?;
    if pos + 1 >= args.len() {
        return None;
    }
    let value = args.remove(pos + 1);
    args.remove(pos);
    Some(value)
}

/// Chemin de socket par défaut (miroir de `nomad-app`, cas sans `--config`).
fn default_socket() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "nomad", "nomad")
        .map(|d| d.config_dir().join("nomad.sock"))
}
