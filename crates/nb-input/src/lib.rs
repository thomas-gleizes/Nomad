//! `nb-input` — abstraction cross-OS de la capture et de l'injection d'entrées.
//!
//! - [`capture`] : écoute globale via `rdev` (souris/clavier), traduite en
//!   événements [`capture::Captured`] portables. Modèle *callback bloquant*
//!   (contrainte de rdev), à lancer sur le thread principal sous macOS.
//! - [`inject`] : injection via `enigo` ([`inject::Injector`]).
//! - [`keymap`] : tables de correspondance rdev ↔ `nb_core` ↔ enigo.
//!
//! Le découpage isole les deux crates OS sensibles derrière un vocabulaire neutre
//! (`nb_core::{InputEvent, Key, Button}`), ce qui rend l'orchestration testable.

pub mod capture;
pub mod inject;
pub mod keymap;

pub use capture::{listen, grab, Captured};
pub use inject::Injector;
