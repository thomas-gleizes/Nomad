//! Injection d'entrées sur le nœud cible, via `enigo`.

use enigo::{Axis, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use nomad_core::layout::Screen;
use nomad_core::InputEvent;

use crate::keymap;

/// Injecteur d'entrées portable.
pub struct Injector {
    enigo: Enigo,
    /// Résolution de l'écran local (pour convertir les positions absolues).
    screen: Screen,
}

impl Injector {
    pub fn new(screen: Screen) -> anyhow::Result<Self> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("initialisation enigo: {e}"))?;
        Ok(Self { enigo, screen })
    }

    /// Applique un événement reçu du réseau.
    pub fn inject(&mut self, ev: &InputEvent) -> anyhow::Result<()> {
        match *ev {
            InputEvent::MouseMove { dx, dy } => {
                self.enigo
                    .move_mouse(dx.round() as i32, dy.round() as i32, Coordinate::Rel)?;
            }
            InputEvent::MouseAbs { rx, ry } => {
                // Coordonnées valides : 0..=width-1.
                let x = (rx.clamp(0.0, 1.0) * self.screen.width.saturating_sub(1) as f64).round() as i32;
                let y = (ry.clamp(0.0, 1.0) * self.screen.height.saturating_sub(1) as f64).round() as i32;
                self.enigo.move_mouse(x, y, Coordinate::Abs)?;
            }
            InputEvent::MouseButton { button, pressed } => {
                self.enigo
                    .button(keymap::to_enigo_button(button), dir(pressed))?;
            }
            InputEvent::MouseWheel { dx, dy } => {
                if dx != 0 {
                    self.enigo.scroll(dx as i32, Axis::Horizontal)?;
                }
                if dy != 0 {
                    self.enigo.scroll(dy as i32, Axis::Vertical)?;
                }
            }
            InputEvent::Key { key, pressed } => {
                if let Some(k) = keymap::to_enigo_key(key) {
                    self.enigo.key(k, dir(pressed))?;
                } else {
                    tracing::debug!(?key, "touche non injectable, ignorée");
                }
            }
        }
        Ok(())
    }

    /// Déplace le curseur à une position absolue en pixels (warp).
    pub fn warp_cursor(&mut self, x: i32, y: i32) {
        let _ = self.enigo.move_mouse(x, y, Coordinate::Abs);
    }
}

fn dir(pressed: bool) -> Direction {
    if pressed {
        Direction::Press
    } else {
        Direction::Release
    }
}
