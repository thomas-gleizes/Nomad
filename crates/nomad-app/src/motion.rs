//! Transformation *pure* des positions absolues capturées en deltas de
//! mouvement, en mode contrôle distant.
//!
//! Le problème : le recentrage du curseur réel (`InjectCmd::Warp`) est
//! asynchrone. Entre l'envoi du warp et son atterrissage physique, les
//! événements capturés sont encore mesurés depuis l'ancienne position ;
//! calculer les deltas « par rapport au centre » sur-compte alors le mouvement
//! (jusqu'à un demi-écran par événement lors d'un franchissement de bord).
//!
//! Solution : les deltas sont calculés entre positions capturées *successives*,
//! et chaque warp émis est mémorisé pour que son événement d'atterrissage soit
//! avalé (il ne correspond à aucun mouvement de l'utilisateur).

use std::collections::VecDeque;

/// Nombre maximal de warps en attente d'atterrissage (au-delà, les plus
/// anciens sont considérés perdus — coalescés par l'OS).
const MAX_PENDING_WARPS: usize = 8;

/// Suit la position réelle du curseur et neutralise les warps de recentrage.
pub struct MotionTracker {
    /// Dernière position absolue capturée.
    last: Option<(f64, f64)>,
    /// Cibles des warps émis dont l'atterrissage n'a pas encore été observé.
    pending_warps: VecDeque<(f64, f64)>,
    /// Tolérance (px) pour reconnaître l'atterrissage d'un warp.
    tolerance: f64,
}

impl MotionTracker {
    pub fn new(tolerance: f64) -> Self {
        Self {
            last: None,
            pending_warps: VecDeque::new(),
            tolerance,
        }
    }

    /// À appeler juste avant d'émettre un `InjectCmd::Warp(x, y)`.
    pub fn expect_warp(&mut self, x: f64, y: f64) {
        if self.pending_warps.len() >= MAX_PENDING_WARPS {
            self.pending_warps.pop_front();
        }
        self.pending_warps.push_back((x, y));
    }

    /// Réinitialise l'état (à chaque transition local ↔ distant) : la première
    /// position capturée ensuite servira de référence, sans produire de delta.
    pub fn reset(&mut self) {
        self.last = None;
        self.pending_warps.clear();
    }

    /// Traite une position absolue capturée. Retourne le delta de mouvement
    /// réel, ou `None` si l'événement doit être ignoré (atterrissage d'un warp,
    /// première position après un `reset`, ou delta nul).
    pub fn delta(&mut self, x: f64, y: f64) -> Option<(f64, f64)> {
        // Atterrissage d'un warp ? On accepte un atterrissage dans le désordre
        // (l'OS peut coalescer) : tout ce qui précède la cible reconnue est
        // considéré perdu.
        if let Some(idx) = self
            .pending_warps
            .iter()
            .position(|&(wx, wy)| (x - wx).abs() <= self.tolerance && (y - wy).abs() <= self.tolerance)
        {
            self.pending_warps.drain(..=idx);
            self.last = Some((x, y));
            return None;
        }

        let delta = self.last.map(|(lx, ly)| (x - lx, y - ly));
        self.last = Some((x, y));
        delta.filter(|&(dx, dy)| dx != 0.0 || dy != 0.0)
    }
}

/// Position de ré-entrée sur l'écran local, en pixels, **en retrait du bord**.
///
/// Sans marge, le warp de retour atterrit dans la zone de déclenchement du
/// bord opposé (`local_move` déclenche dès `x >= w - 1`), et l'événement
/// d'atterrissage renvoie aussitôt le contrôle à l'écran distant.
pub fn entry_px(ratio: f64, extent: u32, margin: f64) -> i32 {
    let max = (extent.saturating_sub(1)) as f64;
    (ratio * max).clamp(margin.min(max / 2.0), (max - margin).max(max / 2.0)).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_position_after_reset_yields_no_delta() {
        let mut t = MotionTracker::new(2.0);
        assert_eq!(t.delta(500.0, 300.0), None);
        assert_eq!(t.delta(510.0, 305.0), Some((10.0, 5.0)));
    }

    #[test]
    fn warp_landing_is_swallowed_and_rebases() {
        let mut t = MotionTracker::new(2.0);
        t.delta(950.0, 500.0);
        t.expect_warp(640.0, 400.0);
        // L'atterrissage du warp ne produit aucun delta…
        assert_eq!(t.delta(640.0, 400.0), None);
        // …et les deltas suivants repartent du centre.
        assert_eq!(t.delta(645.0, 398.0), Some((5.0, -2.0)));
    }

    #[test]
    fn events_in_flight_before_landing_use_previous_position() {
        let mut t = MotionTracker::new(2.0);
        t.delta(950.0, 500.0);
        t.expect_warp(640.0, 400.0);
        // Événement réel arrivé avant l'atterrissage : delta depuis 950, pas depuis 640.
        assert_eq!(t.delta(955.0, 500.0), Some((5.0, 0.0)));
        assert_eq!(t.delta(641.0, 399.0), None); // atterrissage (tolérance)
        assert_eq!(t.delta(650.0, 399.0), Some((9.0, 0.0)));
    }

    #[test]
    fn edge_crossing_scenario_does_not_overshoot() {
        // Reproduit le bug historique : à l'entrée en mode distant, les
        // événements encore collés au bord (x ≈ w-1) ne doivent PAS devenir
        // des deltas d'un demi-écran.
        let mut t = MotionTracker::new(2.0);
        t.reset();
        t.expect_warp(640.0, 400.0);
        assert_eq!(t.delta(1279.0, 500.0), None); // première position : référence
        assert_eq!(t.delta(1279.0, 500.0), None); // répétition clampée au bord : delta nul
        assert_eq!(t.delta(640.0, 400.0), None); // atterrissage du recentrage
        assert_eq!(t.delta(630.0, 400.0), Some((-10.0, 0.0)));
    }

    #[test]
    fn lost_warps_are_dropped_when_a_later_one_lands() {
        let mut t = MotionTracker::new(2.0);
        t.expect_warp(100.0, 100.0);
        t.expect_warp(640.0, 400.0);
        assert_eq!(t.delta(640.0, 400.0), None); // le warp perdu (100,100) est purgé
        assert_eq!(t.delta(645.0, 400.0), Some((5.0, 0.0)));
    }

    #[test]
    fn entry_px_stays_out_of_trigger_zones() {
        // Ré-entrée par le bord droit (ratio 1.0) d'un écran de 1280 px :
        // en retrait, jamais à x >= 1279 (zone de déclenchement).
        assert_eq!(entry_px(1.0, 1280, 8.0), 1271);
        assert_eq!(entry_px(0.0, 1280, 8.0), 8);
        assert_eq!(entry_px(0.5, 1280, 8.0), 640);
        // Ratio hors bornes : clampé.
        assert_eq!(entry_px(1.5, 1280, 8.0), 1271);
    }
}
