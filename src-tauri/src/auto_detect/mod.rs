//! Backend for automatic run detection from screen capture.
//!
//! The first capture backend targets X11/XWayland, which covers D2R running
//! through Proton on Linux even when the desktop session is Wayland. The
//! module is deliberately independent from the UI: it emits run-start and
//! run-end events and lets the frontend decide how to persist them.

pub mod capture;
pub mod detector;
pub mod menu_template;
pub mod x11;

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use capture::{CaptureBackend, GameWindowInfo, NormalizedRegion};
pub use detector::{DetectorConfig, DetectorEvent, LoadingDetector, Phase};
pub use menu_template::{
    menu_groups_visible, MenuAnchorGroup, MenuAnchorResult, MenuTemplateMatcher,
};
pub use x11::X11Backend;

/// One calibrated font-generated menu template. The live loop runs one
/// template per anchor; the diagnostic search is not used per frame.
#[derive(Debug, Clone, Deserialize)]
pub struct MenuTemplateConfig {
    pub region: NormalizedRegion,
    pub text: String,
    pub group: String,
    pub font: String,
    pub size: f32,
    pub letter_spacing: f32,
    pub threshold: f32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AutoDetectConfig {
    pub loading_region: NormalizedRegion,
    pub menu_region: Option<NormalizedRegion>,
    pub menu_templates: Vec<MenuTemplateConfig>,
    pub dark_threshold: f32,
    pub dark_samples: u32,
    pub bright_samples: u32,
    pub menu_threshold: f32,
    pub menu_samples: u32,
    pub menu_dark_samples: u32,
    pub poll_interval_ms: u64,
    pub min_run_seconds: f64,
}

impl Default for AutoDetectConfig {
    fn default() -> Self {
        Self {
            loading_region: NormalizedRegion::default(),
            menu_region: None,
            menu_templates: Vec::new(),
            dark_threshold: 22.0,
            dark_samples: 2,
            bright_samples: 2,
            menu_threshold: 50.0,
            menu_samples: 2,
            menu_dark_samples: 4,
            poll_interval_ms: 250,
            min_run_seconds: 3.0,
        }
    }
}

impl AutoDetectConfig {
    pub fn detector_config(&self) -> DetectorConfig {
        DetectorConfig {
            dark_threshold: self.dark_threshold,
            dark_samples: self.dark_samples.max(1),
            bright_samples: self.bright_samples.max(1),
            menu_threshold: self.menu_threshold,
            menu_samples: self.menu_samples.max(1),
            menu_dark_samples: self.menu_dark_samples.max(1),
            min_run_duration: Duration::from_secs_f64(self.min_run_seconds.max(0.0)),
        }
    }
}

/// One anchor sampled by the live multi-anchor menu detector.
#[derive(Debug, Clone, Serialize)]
pub struct LiveMenuAnchor {
    pub text: String,
    pub group: String,
    pub score: f32,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoDetectStatus {
    pub running: bool,
    pub window: Option<GameWindowInfo>,
    pub last_brightness: Option<f32>,
    pub last_menu_brightness: Option<f32>,
    pub last_menu_score: Option<f32>,
    pub last_menu_anchors: Vec<LiveMenuAnchor>,
    pub phase: String,
    pub last_error: Option<String>,
    pub last_event: Option<String>,
}

impl Default for AutoDetectStatus {
    fn default() -> Self {
        Self {
            running: false,
            window: None,
            last_brightness: None,
            last_menu_brightness: None,
            last_menu_score: None,
            last_menu_anchors: Vec::new(),
            phase: Phase::Unknown.as_str().to_string(),
            last_error: None,
            last_event: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub image: String,
    pub brightness: f32,
    pub window: GameWindowInfo,
}

/// Diagnostic result for the font-generated menu template test.
#[derive(Debug, Clone, Serialize)]
pub struct MenuTemplateTest {
    pub window: GameWindowInfo,
    pub preview: String,
    pub results: Vec<MenuAnchorResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunStartPayload {
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunEndPayload {
    pub duration_ms: u64,
    pub too_short: bool,
    pub source: String,
}
