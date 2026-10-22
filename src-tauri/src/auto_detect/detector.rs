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
    pub min_run_duration: Duration,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            dark_threshold: 22.0,
            dark_samples: 2,
            bright_samples: 2,
            min_run_duration: Duration::from_secs(3),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Unknown,
    InGame,
    Loading,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Unknown => "unknown",
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
    run_started_at: Option<Instant>,
}

impl LoadingDetector {
    pub fn new(config: DetectorConfig) -> Self {
        Self {
            config,
            phase: Phase::Unknown,
            dark_streak: 0,
            bright_streak: 0,
            run_started_at: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Feed one brightness sample (0.0 to 255.0) and return the resulting
    /// events. The first dark-to-bright cycle starts a run; the next dark
    /// cycle ends it.
    pub fn process(&mut self, brightness: f32, now: Instant) -> Vec<DetectorEvent> {
        let mut events = Vec::new();

        if brightness <= self.config.dark_threshold {
            self.bright_streak = 0;
            self.dark_streak = self.dark_streak.saturating_add(1);

            if self.dark_streak >= self.config.dark_samples.max(1) && self.phase != Phase::Loading {
                if self.phase == Phase::InGame {
                    if let Some(started_at) = self.run_started_at.take() {
                        let duration = now.saturating_duration_since(started_at);
                        events.push(DetectorEvent::RunEnd {
                            duration,
                            too_short: duration < self.config.min_run_duration,
                        });
                    }
                }
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
    fn dark_ends_a_run() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(100.0, at(base, 0));
        detector.process(0.0, at(base, 250));
        detector.process(0.0, at(base, 500));
        detector.process(100.0, at(base, 750));
        detector.process(100.0, at(base, 1000));

        detector.process(0.0, at(base, 4000));
        assert_eq!(
            detector.process(0.0, at(base, 4250)),
            vec![DetectorEvent::RunEnd {
                duration: Duration::from_millis(3250),
                too_short: false,
            }]
        );
        assert_eq!(detector.phase(), Phase::Loading);
    }

    #[test]
    fn short_run_is_flagged() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(100.0, at(base, 0));
        detector.process(0.0, at(base, 250));
        detector.process(0.0, at(base, 500));
        detector.process(100.0, at(base, 750));
        detector.process(100.0, at(base, 1000));

        detector.process(0.0, at(base, 1250));
        assert_eq!(
            detector.process(0.0, at(base, 1500)),
            vec![DetectorEvent::RunEnd {
                duration: Duration::from_millis(500),
                too_short: true,
            }]
        );
    }

    #[test]
    fn single_dark_sample_is_debounced() {
        let mut detector = detector();
        let base = Instant::now();
        detector.process(100.0, at(base, 0));
        detector.process(0.0, at(base, 250));
        detector.process(0.0, at(base, 500));
        detector.process(100.0, at(base, 750));
        detector.process(100.0, at(base, 1000));

        detector.process(0.0, at(base, 4000));
        assert!(detector.process(100.0, at(base, 4250)).is_empty());
        assert_eq!(detector.phase(), Phase::InGame);

        detector.process(0.0, at(base, 5000));
        assert_eq!(
            detector.process(0.0, at(base, 5250)),
            vec![DetectorEvent::RunEnd {
                duration: Duration::from_millis(4250),
                too_short: false,
            }]
        );
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
}
