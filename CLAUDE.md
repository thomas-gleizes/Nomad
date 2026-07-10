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

CLI flags: `--server` (force role), `--name`, `--port` (default 47800), `--discovery-secs`, `--clip-poll-ms`, `--config`, `--headless` (no tray UI), `--ipc-socket <path>` (control-API socket, default: next to the config), `--no-ipc` (disable it). Node identity (stable UUID) + prefs persist in `~/.config/nomad/config.toml`. A second instance exits immediately (code 3) when a daemon already answers on the same IPC socket (single-instance guard).

On macOS/Windows a native **tray / menu-bar icon** (crate `nomad-ui`) shows role, name, screen, connected peers and the active screen, with actions (rename, force server, reconnect, quit). It is disabled with `--headless` and is a no-op on Linux (headless).

On macOS a fuller **native app** lives in [`apps/macos/`](apps/macos/) (SwiftUI Swift Package): menu-bar icon + a console window with a sidebar (Machines page live, others placeholders). It is a **thin client of `nomad-ipc`** (zero business logic) that spawns/supervises the daemon in `--headless`; it will supersede the `nomad-ui` tray on macOS. Build with `swift build` / open the folder in Xcode.

## Architecture

Cargo workspace, 7 crates ordered pure → OS-dependent. The key design principle: **all OS-sensitive code (`rdev`, `enigo`, `arboard`, tray UI) is isolated behind portable vocabulary types from `nomad-core`, so orchestration and edge logic stay pure and unit-testable.**

| Crate | Role |
|-------|------|
| `nomad-core` | Pure, no tokio/no OS deps. Wire protocol (`Message`), portable input events (`InputEvent`, `Key`, `Button`), screen `Layout`/`Side`, the length-prefixed bincode `codec`, and the shared UI state model (`status`: `AppStatus`, `SharedStatus`; `AppStatus`/`Role`/`PeerInfo` are `serde`-serializable — they double as the IPC `status` payload). |
| `nomad-net` | mDNS discovery (`mdns-sd`), role election, star-topology TCP transport (tokio). Entry point `start()` returns `Endpoint::{Server,Client}`. |
| `nomad-input` | Global capture (`rdev`) and injection (`enigo`) + rdev↔core↔enigo keymap. |
| `nomad-clip` | Clipboard sync (`arboard`), single-threaded, with anti-echo. |
| `nomad-ui` | Native tray / menu-bar UI (`tao` event loop + `tray-icon` + `muda`), **cfg-gated macOS + Windows** (no-op elsewhere). Read-only: polls `SharedStatus` (via a generation counter) and rebuilds the menu; user clicks return a `UiAction`. |
| `nomad-ipc` | Local **control API** over a Unix socket (JSON-lines, versioned). Read-only over `SharedStatus`: exposes `status`, streams changes on `subscribe`, relays commands to a `DaemonAction` callback. Most commands relaunch (rename / force-server / reconnect / quit); `forget` is a **hot** command (no relaunch) routed to the orchestrator via a control channel. Single-instance guard via connect-probe in `bind`. cfg-gated `unix` (no-op stub elsewhere). This is the foundation for native shells (macOS app, later Windows). Dev tool: `cargo run -p nomad-ipc --example ipcctl -- status`. |
| `nomad-app` | The `nomad` binary: CLI, TOML config, orchestration, edge-switching state machine, UI + IPC wiring. |

### Threading model (critical, drives the whole `main.rs` structure)

Both `rdev` capture and the native UI event loop are **blocking and want the main thread under macOS** (each needs a `CFRunLoop` / `NSApplication`). Only one can own it, so **the UI owns the main thread** and capture moves to a dedicated thread:
- **main thread** = the `nomad-ui` tray event loop (`tao`), when the UI is enabled;
- **`rdev` capture** (server role) runs on a dedicated `nomad-capture` thread — but **only when the UI is enabled**. In **headless** mode (`--headless`, or any non-macOS/Windows platform where the UI is a no-op) capture keeps the main thread, as before;
- the **tokio runtime** (networking + orchestration) runs on its own threads;
- **`enigo` injection** lives on a dedicated `nomad-inject` thread (owns the `Injector`);
- **`arboard` clipboard** lives on a dedicated `nomad-clip` thread.

These are wired together with channels (`std::sync::mpsc` for inject/clip commands, `tokio::sync::mpsc` for captured events and clipboard changes) plus the lock-based `SharedStatus` for UI state. When touching `main.rs`, keep the invariant: **exactly one blocking main-thread owner** (UI when enabled, else capture). UI menu actions (rename / force-server / reconnect) are handled by a **clean process relaunch** (`relaunch()` in `main.rs`), not live reconfiguration.

The **control API** (`nomad-ipc`) runs entirely inside the tokio runtime, so it never contends for the main thread. Both the tray and the IPC server drive the same cloneable `ActionHandler` (in `main.rs`) → same relaunch/exit paths. Two ordering points matter: the IPC socket is `bind`-ed **before** the blocking mDNS discovery (fail fast on a second instance), and `serve` is **spawned before** discovery too — so the API answers immediately. `SharedStatus` is therefore created up front with a *provisional* role (`--server` ⇒ Server, else Client) and corrected right after election.

### Control flow

`nomad-app/src/orchestrator.rs` is the hub. `run_server` selects over: captured local input, server events (joins/leaves/messages), local clipboard changes, a 5 s ping timer (latency measurement), and a hot-control channel (`ControlCmd::Forget`). It tracks each peer's network address and round-trip latency (server-local, never on the wire), and persists **known machines** (`KnownPeer`) to config so the offline list survives restarts. `run_client` selects over incoming server messages, local clipboard changes, and its own 5 s ping timer; it records the server address and its latency to the server. Both directions reuse the existing `Ping`/`Pong` (no `seq`): a side responds to `Ping` and measures on `Pong`, so at most one ping is in flight per link. `nomad-app/src/known.rs` holds the **pure** known-machine logic (upsert / forget / offline-derivation), unit-tested like `edge.rs`.

`nomad-app/src/edge.rs` (`EdgeController`) is the **pure, deterministic** edge-switching state machine — it returns `MoveOutcome` describing transitions; the orchestrator translates those into network messages and cursor warps. It also tracks `exit_side` (the local edge the control left through). In remote-control mode, `nomad-app/src/motion.rs` (`MotionTracker`, also pure) turns captured absolute positions into deltas between **successive** positions; each warp's landing event is recognized (by target, with tolerance) and swallowed — never compute deltas against the anchor, warps are asynchronous. Instead of recentering, the real server cursor is **pinned to the exit edge** (`edge_anchor`, in retreat of `EDGE_INSET`) and **slides along it** to mirror the remote cursor's perpendicular position — it acts as a position indicator; it is only re-warped when it drifts past `ANCHOR_SLACK`. On return to local, the cursor is warped a few pixels **inside** the edge (`REENTRY_MARGIN`) so the landing event doesn't immediately re-trigger `local_move`. The bulk of unit tests live in `edge.rs` and `motion.rs`.

Layout defaults to a horizontal left-to-right row in connection order (server leftmost); configurable TOML layout is not yet implemented.

## CI / releases

`.github/workflows/release.yml` builds Linux/Windows/macOS-aarch64 on every push to `main` and on `v*` tags. **A GitHub Release is only published on tag pushes** (`if: startsWith(github.ref, 'refs/tags/')`); pushes to `main` only upload downloadable build artifacts. To cut a release, push a tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.

## Platform permissions

- **macOS**: the executable needs both **Accessibility** and **Input Monitoring** (System Settings → Privacy & Security). Without them capture (server) and injection (client) silently do nothing.
- **Linux/X11**: local input suppression (`rdev` grab via evdev) may need device-access privileges.

## Known gaps (not implemented)

Client auto-reconnect when the server drops, election collision handling, source-cursor hiding during remote control, file drag-and-drop transfer, TOML-configurable layout. **UI**: no Linux tray yet (headless there); menu actions relaunch the process rather than reconfiguring role/name live.
