//! Tables de correspondance clavier/souris entre les trois représentations :
//! `rdev::*` (capturé) ↔ [`nb_core`] (sur le réseau) ↔ `enigo::*` (injecté).
//!
//! Toute touche rdev non reconnue est transportée via [`nb_core::Key::Raw`] avec
//! son scancode, ce qui garantit une dégradation gracieuse.

use nb_core::{Button, Key};

/// Convertit un bouton rdev en bouton portable.
pub fn from_rdev_button(b: rdev::Button) -> Option<Button> {
    match b {
        rdev::Button::Left => Some(Button::Left),
        rdev::Button::Right => Some(Button::Right),
        rdev::Button::Middle => Some(Button::Middle),
        rdev::Button::Unknown(_) => None,
    }
}

/// Convertit un bouton portable en bouton enigo.
pub fn to_enigo_button(b: Button) -> enigo::Button {
    match b {
        Button::Left => enigo::Button::Left,
        Button::Right => enigo::Button::Right,
        Button::Middle => enigo::Button::Middle,
    }
}

/// Convertit une touche rdev en touche portable.
pub fn from_rdev_key(k: rdev::Key) -> Key {
    use rdev::Key as R;
    match k {
        R::KeyA => Key::A, R::KeyB => Key::B, R::KeyC => Key::C, R::KeyD => Key::D,
        R::KeyE => Key::E, R::KeyF => Key::F, R::KeyG => Key::G, R::KeyH => Key::H,
        R::KeyI => Key::I, R::KeyJ => Key::J, R::KeyK => Key::K, R::KeyL => Key::L,
        R::KeyM => Key::M, R::KeyN => Key::N, R::KeyO => Key::O, R::KeyP => Key::P,
        R::KeyQ => Key::Q, R::KeyR => Key::R, R::KeyS => Key::S, R::KeyT => Key::T,
        R::KeyU => Key::U, R::KeyV => Key::V, R::KeyW => Key::W, R::KeyX => Key::X,
        R::KeyY => Key::Y, R::KeyZ => Key::Z,

        R::Num0 => Key::Num0, R::Num1 => Key::Num1, R::Num2 => Key::Num2,
        R::Num3 => Key::Num3, R::Num4 => Key::Num4, R::Num5 => Key::Num5,
        R::Num6 => Key::Num6, R::Num7 => Key::Num7, R::Num8 => Key::Num8,
        R::Num9 => Key::Num9,

        R::F1 => Key::F1, R::F2 => Key::F2, R::F3 => Key::F3, R::F4 => Key::F4,
        R::F5 => Key::F5, R::F6 => Key::F6, R::F7 => Key::F7, R::F8 => Key::F8,
        R::F9 => Key::F9, R::F10 => Key::F10, R::F11 => Key::F11, R::F12 => Key::F12,

        R::Escape => Key::Escape, R::Tab => Key::Tab, R::CapsLock => Key::CapsLock,
        R::Space => Key::Space, R::Return => Key::Return, R::Backspace => Key::Backspace,
        R::Delete => Key::Delete, R::Insert => Key::Insert,
        R::Home => Key::Home, R::End => Key::End,
        R::PageUp => Key::PageUp, R::PageDown => Key::PageDown,
        R::UpArrow => Key::UpArrow, R::DownArrow => Key::DownArrow,
        R::LeftArrow => Key::LeftArrow, R::RightArrow => Key::RightArrow,

        R::ShiftLeft => Key::ShiftLeft, R::ShiftRight => Key::ShiftRight,
        R::ControlLeft => Key::ControlLeft, R::ControlRight => Key::ControlRight,
        R::Alt => Key::AltLeft, R::AltGr => Key::AltRight,
        R::MetaLeft => Key::MetaLeft, R::MetaRight => Key::MetaRight,

        R::Minus => Key::Minus, R::Equal => Key::Equal,
        R::LeftBracket => Key::LeftBracket, R::RightBracket => Key::RightBracket,
        R::BackSlash => Key::BackSlash, R::SemiColon => Key::SemiColon,
        R::Quote => Key::Quote, R::BackQuote => Key::BackQuote,
        R::Comma => Key::Comma, R::Dot => Key::Dot, R::Slash => Key::Slash,

        R::Unknown(code) => Key::Raw(code),
        // Toute autre touche rdev (pavé numérique, touches média…) : fallback brut.
        other => Key::Raw(rdev_key_fallback_code(other)),
    }
}

/// Code de repli stable pour les touches rdev non mappées symboliquement.
/// (rdev n'expose pas de scancode pour ces variantes ; on hache le nom Debug.)
fn rdev_key_fallback_code(k: rdev::Key) -> u32 {
    // Identifiant déterministe basé sur le nom de variante, décalé hors de la
    // plage des scancodes réels pour éviter toute collision avec `Unknown`.
    let name = format!("{k:?}");
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    0x8000_0000 | (h & 0x7FFF_FFFF)
}

/// Convertit une touche portable en touche enigo injectable.
///
/// Les lettres/chiffres passent par `Unicode` (combinés aux modificateurs
/// transmis séparément). `None` si la touche n'est pas injectable (`Raw`).
pub fn to_enigo_key(k: Key) -> Option<enigo::Key> {
    use enigo::Key as E;
    let key = match k {
        Key::A => E::Unicode('a'), Key::B => E::Unicode('b'), Key::C => E::Unicode('c'),
        Key::D => E::Unicode('d'), Key::E => E::Unicode('e'), Key::F => E::Unicode('f'),
        Key::G => E::Unicode('g'), Key::H => E::Unicode('h'), Key::I => E::Unicode('i'),
        Key::J => E::Unicode('j'), Key::K => E::Unicode('k'), Key::L => E::Unicode('l'),
        Key::M => E::Unicode('m'), Key::N => E::Unicode('n'), Key::O => E::Unicode('o'),
        Key::P => E::Unicode('p'), Key::Q => E::Unicode('q'), Key::R => E::Unicode('r'),
        Key::S => E::Unicode('s'), Key::T => E::Unicode('t'), Key::U => E::Unicode('u'),
        Key::V => E::Unicode('v'), Key::W => E::Unicode('w'), Key::X => E::Unicode('x'),
        Key::Y => E::Unicode('y'), Key::Z => E::Unicode('z'),

        Key::Num0 => E::Unicode('0'), Key::Num1 => E::Unicode('1'), Key::Num2 => E::Unicode('2'),
        Key::Num3 => E::Unicode('3'), Key::Num4 => E::Unicode('4'), Key::Num5 => E::Unicode('5'),
        Key::Num6 => E::Unicode('6'), Key::Num7 => E::Unicode('7'), Key::Num8 => E::Unicode('8'),
        Key::Num9 => E::Unicode('9'),

        Key::F1 => E::F1, Key::F2 => E::F2, Key::F3 => E::F3, Key::F4 => E::F4,
        Key::F5 => E::F5, Key::F6 => E::F6, Key::F7 => E::F7, Key::F8 => E::F8,
        Key::F9 => E::F9, Key::F10 => E::F10, Key::F11 => E::F11, Key::F12 => E::F12,

        Key::Escape => E::Escape, Key::Tab => E::Tab, Key::CapsLock => E::CapsLock,
        Key::Space => E::Space, Key::Return => E::Return, Key::Backspace => E::Backspace,
        Key::Delete => E::Delete,
        Key::Home => E::Home, Key::End => E::End,
        Key::PageUp => E::PageUp, Key::PageDown => E::PageDown,
        Key::UpArrow => E::UpArrow, Key::DownArrow => E::DownArrow,
        Key::LeftArrow => E::LeftArrow, Key::RightArrow => E::RightArrow,

        Key::ShiftLeft | Key::ShiftRight => E::Shift,
        Key::ControlLeft | Key::ControlRight => E::Control,
        Key::AltLeft | Key::AltRight => E::Alt,
        Key::MetaLeft | Key::MetaRight => E::Meta,

        Key::Minus => E::Unicode('-'), Key::Equal => E::Unicode('='),
        Key::LeftBracket => E::Unicode('['), Key::RightBracket => E::Unicode(']'),
        Key::BackSlash => E::Unicode('\\'), Key::SemiColon => E::Unicode(';'),
        Key::Quote => E::Unicode('\''), Key::BackQuote => E::Unicode('`'),
        Key::Comma => E::Unicode(','), Key::Dot => E::Unicode('.'),
        Key::Slash => E::Unicode('/'),

        // Touches sans correspondance symbolique : non injectables.
        Key::Insert | Key::Raw(_) => return None,
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdev_letters_roundtrip_through_core_to_enigo() {
        // Chaque lettre capturée doit produire une touche enigo injectable.
        let pairs = [
            (rdev::Key::KeyA, Key::A),
            (rdev::Key::KeyZ, Key::Z),
            (rdev::Key::Num5, Key::Num5),
            (rdev::Key::Return, Key::Return),
            (rdev::Key::ShiftLeft, Key::ShiftLeft),
        ];
        for (rk, expected) in pairs {
            let core = from_rdev_key(rk);
            assert_eq!(core, expected);
            assert!(to_enigo_key(core).is_some(), "{core:?} devrait être injectable");
        }
    }

    #[test]
    fn unknown_rdev_key_falls_back_to_raw() {
        assert_eq!(from_rdev_key(rdev::Key::Unknown(999)), Key::Raw(999));
        assert!(to_enigo_key(Key::Raw(999)).is_none());
    }

    #[test]
    fn buttons_roundtrip() {
        for b in [Button::Left, Button::Right, Button::Middle] {
            let _ = to_enigo_button(b);
        }
        assert_eq!(from_rdev_button(rdev::Button::Left), Some(Button::Left));
        assert_eq!(from_rdev_button(rdev::Button::Unknown(7)), None);
    }
}
