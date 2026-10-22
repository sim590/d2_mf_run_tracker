//! Combined menu + HUD state machine.
//!
//! The menu template is the fast run-end trigger: returning to the character
//! select screen ends the run within a couple of frames. The HUD confirms that
//! the game is actually in progress, which prevents the menu template from
//! ending a run on an inventory tooltip false positive. When neither signal is
//! visible (zone teleport, loading screen, submenu) the phase is kept as is and
//! nothing is emitted, so no disappearance grace is needed.

use std::time::{Duration, Instant};

use super::detector::{DetectorEvent, Phase};

#[derive(Debug, Clone, Copy)]
pub struct CombinedDetectorConfig {
    /// Consecutive frames the menu must be visible before it counts.
    pub menu_samples: u32,
    /// Consecutive frames the HUD must be visible before it counts.
    pub hud_samples: u32,
    pub min_run_duration: Duration,
    /// HUD-only fallback: when no menu calibration exists, a confirmed missing
    /// HUD ends the run instead of being treated as a transition.
    pub hud_missing_ends_run: bool,
}

impl Default for CombinedDetectorConfig {
    fn default() -> Self {
        Self {
            menu_samples: 2,
            hud_samples: 2,
            min_run_duration: Duration::from_secs(3),
            hud_missing_ends_run: false,
        }
    }
}

pub struct CombinedDetector {
    config: CombinedDetectorConfig,
    phase: Phase,
    menu_streak: u32,
    hud_streak: u32,
    hud_missing_streak: u32,
    run_started_at: Option<Instant>,
}

impl CombinedDetector {
    pub fn new(config: CombinedDetectorConfig) -> Self {
        Self {
            config,
            phase: Phase::Unknown,
            menu_streak: 0,
            hud_streak: 0,
            hud_missing_streak: 0,
            run_started_at: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Feed one frame of raw per-frame signals.
    ///
    /// `menu_visible` is the multi-anchor menu rule (one visible anchor in each
    /// group) and `hud_visible` is the 2-of-3 HUD rule. Both are confirmed over
    /// their own sample count before they are acted on.
    pub fn process(
        &mut self,
        menu_visible: bool,
        hud_visible: bool,
        now: Instant,
    ) -> Vec<DetectorEvent> {
        let mut events = Vec::new();

        if hud_visible {
            self.hud_streak = self.hud_streak.saturating_add(1);
            self.hud_missing_streak = 0;
        } else {
            self.hud_streak = 0;
            self.hud_missing_streak = self.hud_missing_streak.saturating_add(1);
        }
        if menu_visible {
            self.menu_streak = self.menu_streak.saturating_add(1);
        } else {
            self.menu_streak = 0;
        }

        let hud_confirmed = self.hud_streak >= self.config.hud_samples.max(1);
        let menu_confirmed = self.menu_streak >= self.config.menu_samples.max(1);
        let hud_missing_confirmed = self.hud_missing_streak >= self.config.hud_samples.max(1);

        if hud_confirmed {
            // The HUD has priority: stay in game even when the menu template
            // false-positives on an inventory tooltip.
            self.menu_streak = 0;
            if self.phase == Phase::Menu {
                self.run_started_at = Some(now);
                events.push(DetectorEvent::RunStart);
            }
            self.phase = Phase::InGame;
        } else if menu_confirmed && !hud_visible {
            // The menu ends the run quickly, but only when the HUD is not
            // visible (otherwise this is a tooltip false positive).
            if self.phase == Phase::InGame {
                if let Some(started_at) = self.run_started_at.take() {
                    let duration = now.saturating_duration_since(started_at);
                    events.push(DetectorEvent::RunEnd {
                        duration,
                        too_short: duration < self.config.min_run_duration,
                    });
                }
            }
            self.phase = Phase::Menu;
        } else if self.config.hud_missing_ends_run
            && hud_missing_confirmed
            && !menu_visible
            && self.phase == Phase::InGame
        {
            // HUD-only fallback: no menu calibration exists, so a confirmed
            // missing HUD is the run-end trigger.
            if let Some(started_at) = self.run_started_at.take() {
                let duration = now.saturating_duration_since(started_at);
                events.push(DetectorEvent::RunEnd {
                    duration,
                    too_short: duration < self.config.min_run_duration,
                });
            }
            self.phase = Phase::Menu;
        }

        // Otherwise neither signal is confirmed: keep the current phase and
        // emit nothing (teleport, loading screen, submenu).
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detector() -> CombinedDetector {
        CombinedDetector::new(CombinedDetectorConfig {
            menu_samples: 2,
            hud_samples: 2,
            min_run_duration: Duration::from_secs(3),
            hud_missing_ends_run: false,
        })
    }

    fn at(base: Instant, milliseconds: u64) -> Instant {
        base + Duration::from_millis(milliseconds)
    }

    #[test]
    fn menu_then_game_starts_a_run() {
        let mut detector = detector();
        let base = Instant::now();

        // Character select: menu visible, no HUD.
        assert!(detector.process(true, false, at(base, 0)).is_empty());
        assert_eq!(detector.phase(), Phase::Unknown);
        assert!(detector.process(true, false, at(base, 250)).is_empty());
        assert_eq!(detector.phase(), Phase::Menu);

        // Entering the game: HUD appears, menu gone.
        assert!(detector.process(false, true, at(base, 500)).is_empty());
        assert_eq!(
            detector.process(false, true, at(base, 750)),
            vec![DetectorEvent::RunStart]
        );
        assert_eq!(detector.phase(), Phase::InGame);
    }

    #[test]
    fn game_then_menu_ends_a_run() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(true, false, at(base, 0));
        detector.process(true, false, at(base, 250));
        detector.process(false, true, at(base, 500));
        assert_eq!(
            detector.process(false, true, at(base, 750)),
            vec![DetectorEvent::RunStart]
        );

        // Return to the character select: menu visible, no HUD.
        assert!(detector.process(true, false, at(base, 4000)).is_empty());
        assert_eq!(
            detector.process(true, false, at(base, 4250)),
            vec![DetectorEvent::RunEnd {
                duration: Duration::from_millis(3500),
                too_short: false,
            }]
        );
        assert_eq!(detector.phase(), Phase::Menu);
    }

    #[test]
    fn teleport_with_neither_signal_keeps_in_game() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(true, false, at(base, 0));
        detector.process(true, false, at(base, 250));
        detector.process(false, true, at(base, 500));
        assert_eq!(
            detector.process(false, true, at(base, 750)),
            vec![DetectorEvent::RunStart]
        );

        // Zone teleport: neither the menu nor the HUD is visible for a while.
        for step in 0..8u64 {
            let offset = 1000 + step * 250;
            assert!(detector.process(false, false, at(base, offset)).is_empty());
        }
        assert_eq!(detector.phase(), Phase::InGame);

        // The HUD comes back: still in the same run, no new start.
        assert!(detector.process(false, true, at(base, 3000)).is_empty());
        assert!(detector.process(false, true, at(base, 3250)).is_empty());
        assert_eq!(detector.phase(), Phase::InGame);
    }

    #[test]
    fn brief_hud_missing_without_menu_keeps_in_game() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(true, false, at(base, 0));
        detector.process(true, false, at(base, 250));
        detector.process(false, true, at(base, 500));
        assert_eq!(
            detector.process(false, true, at(base, 750)),
            vec![DetectorEvent::RunStart]
        );

        // A couple of HUD-less frames with no menu must not end the run.
        assert!(detector.process(false, false, at(base, 1000)).is_empty());
        assert!(detector.process(false, false, at(base, 1250)).is_empty());
        assert!(detector.process(false, false, at(base, 1500)).is_empty());
        assert_eq!(detector.phase(), Phase::InGame);
    }

    #[test]
    fn menu_false_positive_with_hud_visible_keeps_in_game() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(true, false, at(base, 0));
        detector.process(true, false, at(base, 250));
        detector.process(false, true, at(base, 500));
        assert_eq!(
            detector.process(false, true, at(base, 750)),
            vec![DetectorEvent::RunStart]
        );

        // Inventory tooltip: the menu template false-positives while the HUD
        // stays visible.
        for step in 0..6u64 {
            let offset = 1000 + step * 250;
            assert!(detector.process(true, true, at(base, offset)).is_empty());
        }
        assert_eq!(detector.phase(), Phase::InGame);
    }

    #[test]
    fn manual_stop_does_not_repeat_start_while_hud_stays_visible() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(true, false, at(base, 0));
        detector.process(true, false, at(base, 250));
        detector.process(false, true, at(base, 500));
        assert_eq!(
            detector.process(false, true, at(base, 750)),
            vec![DetectorEvent::RunStart]
        );

        // HUD stays visible for a long time: no repeated start, no end.
        for step in 0..20u64 {
            let offset = 1000 + step * 250;
            assert!(detector.process(false, true, at(base, offset)).is_empty());
        }
        assert_eq!(detector.phase(), Phase::InGame);

        // The next return to the menu ends the run once, then the next entry
        // starts exactly one new run.
        assert!(detector.process(true, false, at(base, 6000)).is_empty());
        assert_eq!(
            detector.process(true, false, at(base, 6250)),
            vec![DetectorEvent::RunEnd {
                duration: Duration::from_millis(5500),
                too_short: false,
            }]
        );

        assert!(detector.process(false, true, at(base, 6500)).is_empty());
        assert_eq!(
            detector.process(false, true, at(base, 6750)),
            vec![DetectorEvent::RunStart]
        );
    }

    #[test]
    fn short_run_is_flagged() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(true, false, at(base, 0));
        detector.process(true, false, at(base, 250));
        detector.process(false, true, at(base, 500));
        assert_eq!(
            detector.process(false, true, at(base, 750)),
            vec![DetectorEvent::RunStart]
        );

        assert!(detector.process(true, false, at(base, 1000)).is_empty());
        assert_eq!(
            detector.process(true, false, at(base, 1250)),
            vec![DetectorEvent::RunEnd {
                duration: Duration::from_millis(500),
                too_short: true,
            }]
        );
    }

    #[test]
    fn unknown_hud_only_starts_without_run_start() {
        let mut detector = detector();
        let base = Instant::now();

        // Detection starts while already in game: no run is started.
        assert!(detector.process(false, true, at(base, 0)).is_empty());
        assert!(detector.process(false, true, at(base, 250)).is_empty());
        assert_eq!(detector.phase(), Phase::InGame);

        // Leaving to the menu does not end a run that was never started.
        assert!(detector.process(true, false, at(base, 500)).is_empty());
        assert!(detector.process(true, false, at(base, 750)).is_empty());
        assert_eq!(detector.phase(), Phase::Menu);

        // The next game entry starts a run.
        assert!(detector.process(false, true, at(base, 1000)).is_empty());
        assert_eq!(
            detector.process(false, true, at(base, 1250)),
            vec![DetectorEvent::RunStart]
        );
    }

    #[test]
    fn hud_only_fallback_ends_on_missing_hud() {
        let mut detector = CombinedDetector::new(CombinedDetectorConfig {
            menu_samples: 2,
            hud_samples: 2,
            min_run_duration: Duration::from_secs(3),
            hud_missing_ends_run: true,
        });
        let base = Instant::now();

        assert!(detector.process(false, true, at(base, 0)).is_empty());
        assert!(detector.process(false, true, at(base, 250)).is_empty());
        assert_eq!(detector.phase(), Phase::InGame);

        // No run was started (Unknown -> InGame), so no end is emitted.
        assert!(detector.process(false, false, at(base, 500)).is_empty());
        assert!(detector.process(false, false, at(base, 750)).is_empty());
        assert_eq!(detector.phase(), Phase::Menu);

        assert!(detector.process(false, true, at(base, 1000)).is_empty());
        assert_eq!(
            detector.process(false, true, at(base, 1250)),
            vec![DetectorEvent::RunStart]
        );

        // The HUD disappears: the run ends (HUD-only fallback).
        assert!(detector.process(false, false, at(base, 4000)).is_empty());
        assert_eq!(
            detector.process(false, false, at(base, 4250)),
            vec![DetectorEvent::RunEnd {
                duration: Duration::from_millis(3000),
                too_short: false,
            }]
        );
    }
}
