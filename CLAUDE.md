# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`nomad` shares **mouse / keyboard / clipboard** across machines on a LAN — a minimal *Mouse Without Borders* clone, written in Rust, cross-OS (macOS, Windows, Linux/X11; **Wayland not supported**).

At launch a node browses mDNS for a server: if one is found it joins as a **client (screen)**; otherwise it auto-promotes to **server (controller)**. The server owns the physical keyboard/mouse; its cursor crosses to a neighbor machine when it hits a screen edge (extended-desktop model). Note: code comments, docs, and CLI help are in **French** — match that when editing.

## Commands

```sh
cargo build --release            # binary at target/release/nomad
cargo test --workspace           # all tests (codec, keymap, edge-switching, net loopback)
cargo test -p nomad-core         # tests for a single crate
cargo test -p nomad-app edge     # a single test module by name filter
RUST_LOG=debug cargo run -- --server   # run with verbose logs, forced server role
```

CLI flags: `--server` (force role), `--name`, `--port` (default 47800), `--discovery-secs`, `--clip-poll-ms`, `--config`. Node identity (stable UUID) + prefs persist in `~/.config/nomad/config.toml`.

## Architecture

Cargo workspace, 5 crates ordered pure → OS-dependent. The key design principle: **all OS-sensitive code (`rdev`, `enigo`, `arboard`) is isolated behind portable vocabulary types from `nomad-core`, so orchestration and edge logic stay pure and unit-testable.**

| Crate | Role |
|-------|------|
| `nomad-core` | Pure, no tokio/no OS deps. Wire protocol (`Message`), portable input events (`InputEvent`, `Key`, `Button`), screen `Layout`/`Side`, and the length-prefixed bincode `codec`. |
| `nomad-net` | mDNS discovery (`mdns-sd`), role election, star-topology TCP transport (tokio). Entry point `start()` returns `Endpoint::{Server,Client}`. |
| `nomad-input` | Global capture (`rdev`) and injection (`enigo`) + rdev↔core↔enigo keymap. |
| `nomad-clip` | Clipboard sync (`arboard`), single-threaded, with anti-echo. |
| `nomad-app` | The `nomad` binary: CLI, TOML config, orchestration, edge-switching state machine. |

### Threading model (critical, drives the whole `main.rs` structure)

`rdev` capture is a **blocking callback that must run on the main thread under macOS** (it needs a `CFRunLoop`). Therefore:
- **main thread** = `rdev` capture (server role only);
- the **tokio runtime** (networking + orchestration) runs on its own threads;
- **`enigo` injection** lives on a dedicated `nomad-inject` thread (owns the `Injector`);
- **`arboard` clipboard** lives on a dedicated `nomad-clip` thread.

These are wired together with channels (`std::sync::mpsc` for inject/clip commands, `tokio::sync::mpsc` for captured events and clipboard changes). When touching `main.rs` or the orchestrator, preserve this separation — moving capture or injection off its required thread breaks macOS silently.

### Control flow

`nomad-app/src/orchestrator.rs` is the hub. `run_server` selects over: captured local input, server events (joins/leaves/messages), and local clipboard changes. `run_client` selects over incoming server messages and local clipboard changes, injecting received events.

`nomad-app/src/edge.rs` (`EdgeController`) is the **pure, deterministic** edge-switching state machine — it returns `MoveOutcome` describing transitions; the orchestrator translates those into network messages and cursor warps. In remote-control mode, `nomad-app/src/motion.rs` (`MotionTracker`, also pure) turns captured absolute positions into deltas between **successive** positions; the real cursor is re-centered (`InjectCmd::Warp`) only when it strays from the center, and each warp's landing event is recognized (by target, with tolerance) and swallowed — never compute deltas against the center, warps are asynchronous. On return to local, the cursor is warped a few pixels **inside** the edge (`REENTRY_MARGIN`) so the landing event doesn't immediately re-trigger `local_move`. The bulk of unit tests live in `edge.rs` and `motion.rs`.

Layout defaults to a horizontal left-to-right row in connection order (server leftmost); configurable TOML layout is not yet implemented.

## CI / releases

`.github/workflows/release.yml` builds Linux/Windows/macOS-aarch64 on every push to `main` and on `v*` tags. **A GitHub Release is only published on tag pushes** (`if: startsWith(github.ref, 'refs/tags/')`); pushes to `main` only upload downloadable build artifacts. To cut a release, push a tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.

## Platform permissions

- **macOS**: the executable needs both **Accessibility** and **Input Monitoring** (System Settings → Privacy & Security). Without them capture (server) and injection (client) silently do nothing.
- **Linux/X11**: local input suppression (`rdev` grab via evdev) may need device-access privileges.

## Known gaps (not implemented)

Client auto-reconnect when the server drops, election collision handling, source-cursor hiding during remote control, file drag-and-drop transfer, TOML-configurable layout.
