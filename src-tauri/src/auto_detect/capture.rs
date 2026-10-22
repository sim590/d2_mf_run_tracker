//! Platform-neutral capture types and trait.
//!
//! Backends (X11/XWayland first, later Wayland, Windows and macOS) implement
//! `CaptureBackend` without depending on another backend module.

use std::fmt;
use std::io::Cursor;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Default for Region {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GameWindowInfo {
    pub id: u32,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct NormalizedRegion {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for NormalizedRegion {
    fn default() -> Self {
        Self {
            x: 1175.0 / 2560.0,
            y: 500.0 / 1440.0,
            width: 200.0 / 2560.0,
            height: 300.0 / 1440.0,
        }
    }
}

impl NormalizedRegion {
    /// Convert fractions of the game window to a pixel region.
    pub fn to_pixels(self, window: &GameWindowInfo) -> Region {
        let window_width = window.width.max(1) as f32;
        let window_height = window.height.max(1) as f32;
        let x = (self.x.clamp(0.0, 1.0) * window_width).round() as i32;
        let y = (self.y.clamp(0.0, 1.0) * window_height).round() as i32;
        let width = (self.width.clamp(0.0, 1.0) * window_width).round() as u32;
        let height = (self.height.clamp(0.0, 1.0) * window_height).round() as u32;
        Region {
            x,
            y,
            width: width.max(1),
            height: height.max(1),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl ImageData {
    pub fn mean_brightness(&self) -> f32 {
        if self.rgb.is_empty() {
            return 0.0;
        }
        let total: u64 = self.rgb.iter().map(|value| u64::from(*value)).sum();
        (total as f64 / self.rgb.len() as f64) as f32
    }

    /// Rec. 601 luma, matching the formula used by the diagnostics.
    pub fn to_luma8(&self) -> image::GrayImage {
        image::GrayImage::from_fn(self.width, self.height, |x, y| {
            let offset = (y as usize * self.width as usize + x as usize) * 3;
            let red = f32::from(self.rgb[offset]);
            let green = f32::from(self.rgb[offset + 1]);
            let blue = f32::from(self.rgb[offset + 2]);
            let luma = (0.299 * red + 0.587 * green + 0.114 * blue).round();
            image::Luma([luma.clamp(0.0, 255.0) as u8])
        })
    }

    pub fn to_png_base64(&self) -> Result<String, CaptureError> {
        let image = image::RgbImage::from_raw(self.width, self.height, self.rgb.clone())
            .ok_or_else(|| CaptureError::Encode("invalid RGB buffer".into()))?;
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|error| CaptureError::Encode(error.to_string()))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        Ok(format!("data:image/png;base64,{encoded}"))
    }
}

#[derive(Debug)]
pub enum CaptureError {
    Connect(String),
    WindowNotFound,
    Backend(String),
    RegionOutOfBounds,
    UnsupportedFormat(usize),
    Encode(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::Connect(message) => {
                write!(formatter, "cannot connect to the display server: {message}")
            }
            CaptureError::WindowNotFound => write!(formatter, "Diablo II window not found"),
            CaptureError::Backend(message) => {
                write!(formatter, "capture backend error: {message}")
            }
            CaptureError::RegionOutOfBounds => {
                write!(formatter, "capture region is outside the game window")
            }
            CaptureError::UnsupportedFormat(bytes) => {
                write!(
                    formatter,
                    "unsupported pixel format ({bytes} bytes per pixel)"
                )
            }
            CaptureError::Encode(message) => write!(formatter, "cannot encode preview: {message}"),
        }
    }
}

impl std::error::Error for CaptureError {}

pub trait CaptureBackend {
    fn find_game_window(&self) -> Result<GameWindowInfo, CaptureError>;
    fn capture_region(
        &self,
        window: &GameWindowInfo,
        region: Region,
    ) -> Result<ImageData, CaptureError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_brightness_works() {
        let image = ImageData {
            width: 2,
            height: 1,
            rgb: vec![0, 0, 0, 255, 255, 255],
        };
        assert!((image.mean_brightness() - 127.5).abs() < 0.1);
    }

    #[test]
    fn luma_uses_rec601_weights() {
        let image = ImageData {
            width: 1,
            height: 1,
            rgb: vec![255, 0, 0],
        };
        assert_eq!(image.to_luma8().get_pixel(0, 0).0[0], 76);
    }

    #[test]
    fn png_preview_has_data_url_prefix() {
        let image = ImageData {
            width: 1,
            height: 1,
            rgb: vec![255, 0, 0],
        };
        let png = image.to_png_base64().expect("png encoding");
        assert!(png.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn normalized_region_scales_with_window() {
        let region = NormalizedRegion::default();
        let reference = GameWindowInfo {
            id: 0,
            title: String::new(),
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        };
        assert_eq!(
            region.to_pixels(&reference),
            Region {
                x: 1175,
                y: 500,
                width: 200,
                height: 300,
            }
        );

        let hd = GameWindowInfo {
            width: 1920,
            height: 1080,
            ..reference
        };
        assert_eq!(
            region.to_pixels(&hd),
            Region {
                x: 881,
                y: 375,
                width: 150,
                height: 225,
            }
        );
    }
}
