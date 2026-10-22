//! HUD image templates.
//!
//! Each marker is a PNG captured from the live game (the static orb/skill-bar
//! frame, not the animated liquid). The template is stored as a data URL in the
//! frontend calibration and scored against the matching region of the game
//! window. The scoring itself is shared with the menu diagnostic through
//! `matching::ImageTemplateMatcher`.

use base64::Engine as _;
use image::GrayImage;

use super::matching::ImageTemplateMatcher;
use super::{HudTemplateConfig, HudTemplateResult, NormalizedRegion};

/// How many markers must be visible for the HUD to count as in game.
pub const HUD_REQUIRED_VISIBLE: usize = 2;

/// Fixed default threshold for the HUD decision.
pub const HUD_DEFAULT_THRESHOLD: f32 = 0.3;

/// Required number of visible markers for a given number of configured
/// templates: 2 of 3, or all of them when fewer than three are configured.
pub fn required_visible(template_count: usize) -> usize {
    HUD_REQUIRED_VISIBLE.min(template_count)
}

/// Whether enough markers are visible to consider the game in progress.
pub fn hud_visible(results: &[HudTemplateResult], required: usize) -> bool {
    results.iter().filter(|result| result.visible).count() >= required
}

/// Decode a PNG data URL or a bare base64 PNG into a grayscale image.
pub fn decode_png_data_url(template: &str) -> Result<GrayImage, String> {
    let trimmed = template.trim();
    if trimmed.is_empty() {
        return Err("HUD template image is empty".to_string());
    }

    let encoded = if let Some(rest) = trimmed.strip_prefix("data:") {
        rest.split_once(',').map_or(rest, |(_, data)| data)
    } else {
        trimmed
    };

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|error| format!("HUD template is not valid base64: {error}"))?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|error| format!("HUD template is not a valid PNG: {error}"))?;
    Ok(image.to_luma8())
}

/// A calibrated HUD marker: a decoded template, its region name and its
/// visibility threshold.
pub struct HudTemplateMatcher {
    name: String,
    region: NormalizedRegion,
    threshold: f32,
    matcher: ImageTemplateMatcher,
}

impl HudTemplateMatcher {
    pub fn new(config: &HudTemplateConfig) -> Result<Self, String> {
        if config.name.trim().is_empty() {
            return Err("HUD template has an empty name".to_string());
        }

        let image = decode_png_data_url(&config.template)?;
        let matcher = ImageTemplateMatcher::new(&image).ok_or_else(|| {
            format!(
                "HUD template '{}' is blank or too flat to match",
                config.name
            )
        })?;
        let threshold = if config.threshold.is_finite() && config.threshold > 0.0 {
            config.threshold
        } else {
            HUD_DEFAULT_THRESHOLD
        };

        Ok(Self {
            name: config.name.clone(),
            region: config.region,
            threshold,
            matcher,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn region(&self) -> NormalizedRegion {
        self.region
    }

    /// Score the marker against a captured region and decide visibility.
    pub fn score(&self, search: &GrayImage) -> HudTemplateResult {
        match self.matcher.best(search) {
            Some(matched) => HudTemplateResult {
                name: self.name.clone(),
                score: matched.score,
                ncc_score: matched.ncc,
                binary_score: matched.binary,
                edge_score: matched.edge,
                visible: matched.score >= self.threshold,
                x: matched.x,
                y: matched.y,
            },
            None => HudTemplateResult {
                name: self.name.clone(),
                score: 0.0,
                ncc_score: 0.0,
                binary_score: 0.0,
                edge_score: 0.0,
                visible: false,
                x: 0,
                y: 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn png_data_url(image: &GrayImage) -> String {
        let rgb = image::DynamicImage::ImageLuma8(image.clone()).to_rgb8();
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(rgb)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode png");
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        format!("data:image/png;base64,{encoded}")
    }

    fn frame_template(width: u32, height: u32) -> GrayImage {
        let mut image = GrayImage::from_pixel(width, height, Luma([0]));
        for y in 0..height {
            for x in 0..width {
                let border = x < 2 || y < 2 || x + 2 >= width || y + 2 >= height;
                if border {
                    let value = 80 + ((x * 17 + y * 29) % 160) as u8;
                    image.put_pixel(x, y, Luma([value]));
                } else if (x * 7 + y * 11) % 23 == 0 {
                    image.put_pixel(x, y, Luma([60]));
                }
            }
        }
        image
    }

    fn config(name: &str, template: GrayImage) -> HudTemplateConfig {
        HudTemplateConfig {
            name: name.to_string(),
            region: crate::auto_detect::NormalizedRegion::default(),
            template: png_data_url(&template),
            threshold: HUD_DEFAULT_THRESHOLD,
        }
    }

    #[test]
    fn decodes_png_data_url_round_trip() {
        let template = frame_template(20, 12);
        let decoded = decode_png_data_url(&png_data_url(&template)).expect("decode");
        assert_eq!(decoded.width(), 20);
        assert_eq!(decoded.height(), 12);
    }

    #[test]
    fn rejects_invalid_templates() {
        assert!(decode_png_data_url("").is_err());
        assert!(decode_png_data_url("not base64!").is_err());
        assert!(decode_png_data_url("data:image/png;base64,bm90IGEgcG5n").is_err());
    }

    #[test]
    fn matcher_marks_pasted_template_visible() {
        let template = frame_template(40, 40);
        let matcher = HudTemplateMatcher::new(&config("life orb", template.clone())).expect("new");

        let mut search = GrayImage::from_pixel(200, 120, Luma([10]));
        for y in 0..template.height() {
            for x in 0..template.width() {
                search.put_pixel(70 + x, 40 + y, *template.get_pixel(x, y));
            }
        }

        let result = matcher.score(&search);
        assert!(result.visible, "score was {}", result.score);
        assert!(result.score > 0.8);
    }

    #[test]
    fn matcher_marks_blank_region_invisible() {
        let matcher =
            HudTemplateMatcher::new(&config("mana orb", frame_template(40, 40))).expect("new");
        let search = GrayImage::from_pixel(200, 120, Luma([0]));
        assert!(!matcher.score(&search).visible);
    }

    #[test]
    fn requires_two_of_three() {
        let make = |name: &str, visible: bool| HudTemplateResult {
            name: name.to_string(),
            score: if visible { 0.9 } else { 0.0 },
            ncc_score: 0.0,
            binary_score: 0.0,
            edge_score: 0.0,
            visible,
            x: 0,
            y: 0,
        };

        let results = vec![
            make("life orb", true),
            make("mana orb", true),
            make("skill bar", false),
        ];
        assert!(hud_visible(&results, required_visible(results.len())));

        let results = vec![
            make("life orb", true),
            make("mana orb", false),
            make("skill bar", false),
        ];
        assert!(!hud_visible(&results, required_visible(results.len())));

        // Two configured markers require both.
        let results = vec![make("life orb", true), make("mana orb", false)];
        assert!(!hud_visible(&results, required_visible(results.len())));
        assert_eq!(required_visible(2), 2);
        assert_eq!(required_visible(3), 2);
    }
}
