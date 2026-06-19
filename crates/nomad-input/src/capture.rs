//! Capture globale des entrées via `rdev`.
//!
//! ⚠️ `rdev::listen`/`grab` sont **bloquants** et, sur macOS, doivent tourner sur
//! le **thread principal** (ils y installent une `CFRunLoop`). L'appelant
//! (`nomad-app`) réserve donc le thread principal à la capture et exécute le reste
//! (réseau, orchestration) sur d'autres threads.

use nomad_core::{Button, Key};
use rdev::{Event, EventType};

use crate::keymap;

/// Un événement d'entrée capturé localement (la souris est en position absolue,
/// telle que rapportée par l'OS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Captured {
    /// Position absolue du curseur (pixels écran).
    MouseMoveAbs { x: f64, y: f64 },
    MouseButton { button: Button, pressed: bool },
    MouseWheel { dx: i64, dy: i64 },
    Key { key: Key, pressed: bool },
}

/// Traduit un événement rdev en [`Captured`] portable.
pub fn translate(event_type: &EventType) -> Option<Captured> {
    match *event_type {
        EventType::MouseMove { x, y } => Some(Captured::MouseMoveAbs { x, y }),
        EventType::ButtonPress(b) => keymap::from_rdev_button(b)
            .map(|button| Captured::MouseButton { button, pressed: true }),
        EventType::ButtonRelease(b) => keymap::from_rdev_button(b)
            .map(|button| Captured::MouseButton { button, pressed: false }),
        EventType::Wheel { delta_x, delta_y } => Some(Captured::MouseWheel {
            dx: delta_x,
            dy: delta_y,
        }),
        EventType::KeyPress(k) => Some(Captured::Key {
            key: keymap::from_rdev_key(k),
            pressed: true,
        }),
        EventType::KeyRelease(k) => Some(Captured::Key {
            key: keymap::from_rdev_key(k),
            pressed: false,
        }),
    }
}

/// Démarre l'écoute globale (sans suppression locale). **Bloquant.**
///
/// `on_event` est appelé pour chaque événement traduit. À exécuter sur le thread
/// principal (macOS).
pub fn listen<F>(mut on_event: F) -> anyhow::Result<()>
where
    F: FnMut(Captured) + 'static,
{
    rdev::listen(move |event: Event| {
        if let Some(c) = translate(&event.event_type) {
            on_event(c);
        }
    })
    .map_err(|e| anyhow::anyhow!("rdev listen: {e:?}"))?;
    Ok(())
}

/// Démarre la capture **avec suppression conditionnelle** (P4).
///
/// Pour chaque événement traduit, `decide(captured)` est appelé : s'il renvoie
/// `true`, l'événement est consommé localement (non délivré aux applications) ;
/// sinon il est laissé passer. Les événements non traduisibles passent toujours.
///
/// Nécessite la feature `unstable_grab` de rdev et, sur Linux, des privilèges
/// d'accès à evdev.
pub fn grab<F>(decide: F) -> anyhow::Result<()>
where
    F: Fn(Captured) -> bool + 'static,
{
    rdev::grab(move |event: Event| match translate(&event.event_type) {
        Some(c) => {
            if decide(c) {
                None // consomme localement
            } else {
                Some(event) // laisse passer
            }
        }
        None => Some(event),
    })
    .map_err(|e| anyhow::anyhow!("rdev grab: {e:?}"))?;
    Ok(())
}
