//! Événements d'entrée portables (indépendants de l'OS et des crates rdev/enigo).
//!
//! `nomad-input` traduit `rdev::*` (capture) vers ces types, et ces types vers
//! `enigo::*` (injection). Garder ce vocabulaire neutre évite de coupler le
//! protocole réseau à une implémentation d'entrée particulière.

use serde::{Deserialize, Serialize};

/// Boutons de souris.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// Touche clavier portable.
///
/// On couvre l'ensemble pratique des touches courantes. Toute touche non listée
/// est transportée via [`Key::Raw`] (scancode brut de la plateforme source), ce
/// qui permet une dégradation gracieuse même si la correspondance symbolique
/// n'est pas connue des deux côtés.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Key {
    // Lettres
    A, B, C, D, E, F, G, H, I, J, K, L, M,
    N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    // Chiffres (rangée principale)
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    // Fonctions
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    // Contrôles / édition
    Escape, Tab, CapsLock, Space, Return, Backspace, Delete, Insert,
    Home, End, PageUp, PageDown,
    UpArrow, DownArrow, LeftArrow, RightArrow,
    // Modificateurs
    ShiftLeft, ShiftRight, ControlLeft, ControlRight,
    AltLeft, AltRight, MetaLeft, MetaRight,
    // Ponctuation usuelle (positions US)
    Minus, Equal, LeftBracket, RightBracket, BackSlash, SemiColon,
    Quote, BackQuote, Comma, Dot, Slash,
    /// Touche non mappée symboliquement : scancode brut de la plateforme source.
    Raw(u32),
}

/// Un événement d'entrée unitaire circulant sur le réseau.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Déplacement relatif de la souris (mode contrôle distant).
    MouseMove { dx: f64, dy: f64 },
    /// Déplacement absolu (ratio 0.0..=1.0 de l'écran cible) — utilisé à l'entrée d'un écran.
    MouseAbs { rx: f64, ry: f64 },
    /// Appui/relâche d'un bouton.
    MouseButton { button: Button, pressed: bool },
    /// Molette (deltas crantés).
    MouseWheel { dx: i64, dy: i64 },
    /// Appui/relâche d'une touche.
    Key { key: Key, pressed: bool },
}
