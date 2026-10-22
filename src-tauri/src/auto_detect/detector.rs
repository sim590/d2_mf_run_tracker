//! Loading-screen state machine.
//!
//! The detector is independent from Tauri and from the capture backend. It
//! receives a brightness sample and returns zero or more events, which makes
//! the logic unit-testable without a display or a running game.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct DetectorConfig {
    pub dark_threshold: f32,
    pub dark_samples: u32,
    pub bright_samples: u32,
    pub menu_threshold: f32,
    pub menu_samples: u32,
    pub menu_dark_samples: u32,
    pub min_run_duration: Duration,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            dark_threshold: 22.0,
            dark_samples: 2,
            bright_samples: 2,
            menu_threshold: 50.0,
            menu_samples: 2,
            menu_dark_samples: 4,
            min_run_duration: Duration::from_secs(3),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Unknown,
    Menu,
    InGame,
    Loading,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Unknown => "unknown",
            Phase::Menu => "menu",
            Phase::InGame => "in-game",
            Phase::Loading => "loading",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectorEvent {
    RunStart,
    RunEnd { duration: Duration, too_short: bool },
}

pub struct LoadingDetector {
    config: DetectorConfig,
    phase: Phase,
    dark_streak: u32,
    bright_streak: u32,
    menu_streak: u32,
    menu_dark_streak: u32,
    run_started_at: Option<Instant>,
}

impl LoadingDetector {
    pub fn new(config: DetectorConfig) -> Self {
        Self {
            config,
            phase: Phase::Unknown,
            dark_streak: 0,
            bright_streak: 0,
            menu_streak: 0,
            menu_dark_streak: 0,
            run_started_at: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Feed one loading-probe brightness sample. Menu detection is disabled.
    #[cfg(test)]
    pub fn process(&mut self, brightness: f32, now: Instant) -> Vec<DetectorEvent> {
        self.process_with_menu(brightness, None, now)
    }

    /// Feed the loading probe and, when configured, the menu probe.
    ///
    /// A bright menu probe has priority: it ends an active run and keeps the
    /// detector in the Menu phase until a loading screen starts a new game.
    pub fn process_with_menu(
        &mut self,
        brightness: f32,
        menu_brightness: Option<f32>,
        now: Instant,
    ) -> Vec<DetectorEvent> {
        let mut events = Vec::new();

        if let Some(menu_brightness) = menu_brightness {
            if menu_brightness >= self.config.menu_threshold {
                self.menu_streak = self.menu_streak.saturating_add(1);
                if self.menu_streak >= self.config.menu_samples.max(1) && self.phase != Phase::Menu
                {
                    if let Some(started_at) = self.run_started_at.take() {
                        let duration = now.saturating_duration_since(started_at);
                        events.push(DetectorEvent::RunEnd {
                            duration,
                            too_short: duration < self.config.min_run_duration,
                        });
                    }
                    self.phase = Phase::Menu;
                    self.dark_streak = 0;
                    self.bright_streak = 0;
                }
                self.menu_dark_streak = 0;
                // Wait for menu confirmation. A bright menu frame must not
                // start a run through the loading probe.
                return events;
            }
            self.menu_streak = 0;
            self.menu_dark_streak = self.menu_dark_streak.saturating_add(1);
        } else {
            self.menu_streak = 0;
            self.menu_dark_streak = 0;
        }

        if brightness <= self.config.dark_threshold {
            self.bright_streak = 0;
            self.dark_streak = self.dark_streak.saturating_add(1);

            if self.dark_streak >= self.config.dark_samples.max(1) && self.phase != Phase::Loading {
                self.phase = Phase::Loading;
            }
        } else {
            self.dark_streak = 0;
            self.bright_streak = self.bright_streak.saturating_add(1);

            match self.phase {
                Phase::Unknown => {
                    if self.bright_streak >= self.config.bright_samples.max(1) {
                        self.phase = Phase::InGame;
                    }
                }
                Phase::Loading => {
                    if self.bright_streak >= self.config.bright_samples.max(1) {
                        self.phase = Phase::InGame;
                        if self.run_started_at.is_none() {
                            self.run_started_at = Some(now);
                            events.push(DetectorEvent::RunStart);
                        }
                    }
                }
                Phase::InGame => {}
                Phase::Menu => {
                    if self.menu_dark_streak >= self.config.menu_dark_samples.max(1)
                        && self.bright_streak >= self.config.bright_samples.max(1)
                    {
                        self.phase = Phase::InGame;
                        if self.run_started_at.is_none() {
                            self.run_started_at = Some(now);
                            events.push(DetectorEvent::RunStart);
                        }
                    }
                }
            }
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn detector() -> LoadingDetector {
        LoadingDetector::new(DetectorConfig {
            dark_threshold: 22.0,
            dark_samples: 2,
            bright_samples: 2,
            menu_threshold: 50.0,
            menu_samples: 2,
            menu_dark_samples: 4,
            min_run_duration: Duration::from_secs(3),
        })
    }

    fn at(base: Instant, milliseconds: u64) -> Instant {
        base + Duration::from_millis(milliseconds)
    }

    #[test]
    fn initial_bright_does_not_start_a_run() {
        let mut detector = detector();
        let base = Instant::now();
        assert!(detector.process(100.0, at(base, 0)).is_empty());
        assert!(detector.process(100.0, at(base, 250)).is_empty());
        assert_eq!(detector.phase(), Phase::InGame);
    }

    #[test]
    fn loading_then_bright_starts_a_run() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(100.0, at(base, 0));
        assert!(detector.process(0.0, at(base, 250)).is_empty());
        assert!(detector.process(0.0, at(base, 500)).is_empty());
        assert!(detector.process(100.0, at(base, 750)).is_empty());
        assert_eq!(
            detector.process(100.0, at(base, 1000)),
            vec![DetectorEvent::RunStart]
        );
        assert_eq!(detector.phase(), Phase::InGame);
    }

    #[test]
    fn initial_dark_then_bright_starts_a_run() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(0.0, at(base, 0));
        detector.process(0.0, at(base, 250));
        assert_eq!(detector.phase(), Phase::Loading);
        detector.process(100.0, at(base, 500));
        assert_eq!(
            detector.process(100.0, at(base, 750)),
            vec![DetectorEvent::RunStart]
        );
    }

    #[test]
    fn menu_ends_a_run_after_loading() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process_with_menu(100.0, Some(0.0), at(base, 0));
        detector.process_with_menu(100.0, Some(0.0), at(base, 250));
        detector.process_with_menu(0.0, Some(0.0), at(base, 500));
        detector.process_with_menu(0.0, Some(0.0), at(base, 750));
        detector.process_with_menu(100.0, Some(0.0), at(base, 1000));
        assert_eq!(
            detector.process_with_menu(100.0, Some(0.0), at(base, 1250)),
            vec![DetectorEvent::RunStart]
        );

        // A loading screen inside the game does not end the run.
        detector.process_with_menu(0.0, Some(0.0), at(base, 4000));
        assert!(detector
            .process_with_menu(0.0, Some(0.0), at(base, 4250))
            .is_empty());
        assert_eq!(detector.phase(), Phase::Loading);

        // Returning to the menu ends it.
        assert!(detector
            .process_with_menu(100.0, Some(88.0), at(base, 4500))
            .is_empty());
        assert_eq!(
            detector.process_with_menu(100.0, Some(88.0), at(base, 4750)),
            vec![DetectorEvent::RunEnd {
                duration: Duration::from_millis(3500),
                too_short: false,
            }]
        );
        assert_eq!(detector.phase(), Phase::Menu);
    }

    #[test]
    fn short_run_is_flagged_when_returning_to_menu() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process_with_menu(100.0, Some(0.0), at(base, 0));
        detector.process_with_menu(100.0, Some(0.0), at(base, 250));
        detector.process_with_menu(0.0, Some(0.0), at(base, 500));
        detector.process_with_menu(0.0, Some(0.0), at(base, 750));
        detector.process_with_menu(100.0, Some(0.0), at(base, 1000));
        assert_eq!(
            detector.process_with_menu(100.0, Some(0.0), at(base, 1250)),
            vec![DetectorEvent::RunStart]
        );

        // Return to the menu quickly: 500 ms run.
        assert!(detector
            .process_with_menu(100.0, Some(88.0), at(base, 1500))
            .is_empty());
        assert_eq!(
            detector.process_with_menu(100.0, Some(88.0), at(base, 1750)),
            vec![DetectorEvent::RunEnd {
                duration: Duration::from_millis(500),
                too_short: true,
            }]
        );
    }

    #[test]
    fn single_dark_sample_does_not_enter_loading() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process_with_menu(100.0, Some(0.0), at(base, 0));
        detector.process_with_menu(100.0, Some(0.0), at(base, 250));
        assert_eq!(detector.phase(), Phase::InGame);

        assert!(detector
            .process_with_menu(0.0, Some(0.0), at(base, 500))
            .is_empty());
        assert_eq!(detector.phase(), Phase::InGame);

        assert!(detector
            .process_with_menu(0.0, Some(0.0), at(base, 750))
            .is_empty());
        assert_eq!(detector.phase(), Phase::Loading);
    }

    #[test]
    fn short_bright_blip_does_not_start_a_run() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(0.0, at(base, 0));
        detector.process(0.0, at(base, 250));
        detector.process(100.0, at(base, 500));
        detector.process(0.0, at(base, 750));
        detector.process(0.0, at(base, 1000));
        assert_eq!(detector.phase(), Phase::Loading);
        assert!(detector.process(100.0, at(base, 1250)).is_empty());
        assert_eq!(
            detector.process(100.0, at(base, 1500)),
            vec![DetectorEvent::RunStart]
        );
    }

    #[test]
    fn menu_probe_enters_menu_without_starting_a_run() {
        let mut detector = detector();
        let base = Instant::now();
        assert!(detector
            .process_with_menu(100.0, Some(88.0), at(base, 0))
            .is_empty());
        assert!(detector
            .process_with_menu(100.0, Some(88.0), at(base, 250))
            .is_empty());
        assert_eq!(detector.phase(), Phase::Menu);
        assert!(detector
            .process_with_menu(100.0, Some(88.0), at(base, 500))
            .is_empty());
    }

    #[test]
    fn menu_probe_ends_active_run() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process_with_menu(100.0, Some(0.0), at(base, 0));
        detector.process_with_menu(100.0, Some(0.0), at(base, 250));
        detector.process_with_menu(0.0, Some(0.0), at(base, 500));
        detector.process_with_menu(0.0, Some(0.0), at(base, 750));
        detector.process_with_menu(100.0, Some(0.0), at(base, 1000));
        assert_eq!(
            detector.process_with_menu(100.0, Some(0.0), at(base, 1250)),
            vec![DetectorEvent::RunStart]
        );

        assert!(detector
            .process_with_menu(100.0, Some(88.0), at(base, 4000))
            .is_empty());
        assert_eq!(
            detector.process_with_menu(100.0, Some(88.0), at(base, 4250)),
            vec![DetectorEvent::RunEnd {
                duration: Duration::from_millis(3000),
                too_short: false,
            }]
        );
        assert_eq!(detector.phase(), Phase::Menu);
    }

    #[test]
    fn menu_then_loading_starts_a_run() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process_with_menu(100.0, Some(88.0), at(base, 0));
        detector.process_with_menu(100.0, Some(88.0), at(base, 250));
        assert_eq!(detector.phase(), Phase::Menu);

        detector.process_with_menu(0.0, Some(0.0), at(base, 500));
        detector.process_with_menu(0.0, Some(0.0), at(base, 750));
        assert_eq!(detector.phase(), Phase::Loading);

        detector.process_with_menu(100.0, Some(0.0), at(base, 1000));
        assert_eq!(
            detector.process_with_menu(100.0, Some(0.0), at(base, 1250)),
            vec![DetectorEvent::RunStart]
        );
    }

    #[test]
    fn menu_dark_then_game_bright_leaves_menu_and_starts_run() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process_with_menu(100.0, Some(88.0), at(base, 0));
        detector.process_with_menu(100.0, Some(88.0), at(base, 250));
        assert_eq!(detector.phase(), Phase::Menu);

        for step in 0..4u64 {
            let offset = 500 + step * 250;
            let events = detector.process_with_menu(100.0, Some(22.0), at(base, offset));
            if step < 3 {
                assert!(events.is_empty());
            } else {
                assert_eq!(events, vec![DetectorEvent::RunStart]);
            }
        }
        assert_eq!(detector.phase(), Phase::InGame);
    }
}
