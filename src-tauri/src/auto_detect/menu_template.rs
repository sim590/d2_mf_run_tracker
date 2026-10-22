//! One-shot diagnostic: render menu anchor strings from the bundled Exocet
//! Blizzard font and look for them in a captured menu region.
//!
//! This module is intentionally independent from the brightness/menu detector.
//! It only answers "do font-generated templates match the real menu?" so the
//! result can guide a future implementation, if any.

use std::cmp::Ordering;
use std::sync::OnceLock;

use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use fontdue::Font;
use image::{GrayImage, Luma};
use serde::Serialize;

static FONT_BYTES: &[u8] = include_bytes!("../../../src/assets/exocet-blizzard-medium.ttf");

/// Left column buttons on the character selection screen. The strings are
/// unique to that screen (unlike OPTIONS, which the pause menu also shows).
pub const MENU_ANCHORS: [&str; 3] = ["CINÉMATIQUES", "CRÉDITS", "QUITTER DIABLO II"];

const SIZES: [f32; 6] = [24.0, 28.0, 32.0, 36.0, 40.0, 44.0];
const LETTER_SPACINGS: [f32; 4] = [0.0, 1.0, 2.0, 3.0];

/// Empty pixels around the rendered text so the crop is visually verifiable.
const RENDER_PADDING: u32 = 3;
/// Template pixels at or below this value are ignored by the mask.
const MASK_THRESHOLD: u8 = 24;
/// Downsampling factor for the coarse pass.
const COARSE_FACTOR: u32 = 4;
/// How many coarse size/spacing candidates are refined at full resolution.
/// Generous on purpose: a 4x downsample can blur the true match behind a
/// spurious one, and refining is much cheaper than the coarse scan.
const COARSE_CANDIDATES: usize = 12;
/// Coarse scores below this are not worth a full-resolution refinement.
const COARSE_SKIP_BELOW: f32 = 0.05;
/// Search radius (in full-resolution pixels) around a coarse match.
const REFINE_RADIUS: u32 = 12;

#[derive(Debug, Clone, Serialize)]
pub struct MenuAnchorResult {
    pub text: String,
    pub score: f32,
    pub size: f32,
    pub letter_spacing: f32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl MenuAnchorResult {
    fn empty(text: &str) -> Self {
        Self {
            text: text.to_string(),
            score: 0.0,
            size: 0.0,
            letter_spacing: 0.0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }
}

fn font() -> &'static Font {
    static FONT: OnceLock<Font> = OnceLock::new();
    FONT.get_or_init(|| {
        Font::from_bytes(FONT_BYTES, fontdue::FontSettings::default())
            .expect("the bundled Exocet Blizzard font is valid")
    })
}

/// Render `text` to a tightly cropped grayscale image (white glyphs on black).
///
/// `fontdue` 0.9 has no letter spacing option in its layout API, so glyphs are
/// laid out normally and each subsequent glyph is shifted by `letter_spacing`.
/// Kerning between glyphs is lost when spacing is non-zero, which is fine for
/// this diagnostic.
pub fn render_menu_text(text: &str, size_px: f32, letter_spacing: f32) -> GrayImage {
    let font = font();
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings::default());
    layout.append(&[font], &TextStyle::new(text, size_px, 0));

    let glyphs = layout.glyphs();
    if glyphs.is_empty() {
        return GrayImage::from_pixel(1, 1, Luma([0]));
    }

    // PositiveYDown: glyph.x is the left edge and glyph.y the top edge of the
    // rasterized glyph, so positioning is a straight paste.
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (index, glyph) in glyphs.iter().enumerate() {
        let left = glyph.x + index as f32 * letter_spacing;
        min_x = min_x.min(left);
        min_y = min_y.min(glyph.y);
        max_x = max_x.max(left + glyph.width as f32);
        max_y = max_y.max(glyph.y + glyph.height as f32);
    }

    let width = (max_x - min_x).ceil().max(1.0) as u32 + RENDER_PADDING * 2;
    let height = (max_y - min_y).ceil().max(1.0) as u32 + RENDER_PADDING * 2;
    let mut image = GrayImage::from_pixel(width, height, Luma([0]));

    for (index, glyph) in glyphs.iter().enumerate() {
        let (metrics, bitmap) = font.rasterize_config(glyph.key);
        if metrics.width == 0 || metrics.height == 0 {
            continue;
        }
        let origin_x = (glyph.x + index as f32 * letter_spacing - min_x).round() as i32
            + RENDER_PADDING as i32;
        let origin_y = (glyph.y - min_y).round() as i32 + RENDER_PADDING as i32;

        for glyph_y in 0..metrics.height {
            let y = origin_y + glyph_y as i32;
            if y < 0 || y >= height as i32 {
                continue;
            }
            for glyph_x in 0..metrics.width {
                let x = origin_x + glyph_x as i32;
                if x < 0 || x >= width as i32 {
                    continue;
                }
                let coverage = bitmap[glyph_y * metrics.width + glyph_x];
                if coverage == 0 {
                    continue;
                }
                let pixel = image.get_pixel_mut(x as u32, y as u32);
                pixel.0[0] = pixel.0[0].max(coverage);
            }
        }
    }

    image
}

/// Masked normalized cross-correlation template.
struct PreparedTemplate {
    width: u32,
    height: u32,
    offsets: Vec<(u32, u32)>,
    values: Vec<f64>,
    mean: f64,
    variance: f64,
}

impl PreparedTemplate {
    fn new(image: &GrayImage, threshold: u8) -> Option<Self> {
        let mut offsets = Vec::new();
        let mut values = Vec::new();
        for y in 0..image.height() {
            for x in 0..image.width() {
                let value = image.get_pixel(x, y).0[0];
                if value > threshold {
                    offsets.push((x, y));
                    values.push(f64::from(value));
                }
            }
        }
        if values.is_empty() {
            return None;
        }

        let count = values.len() as f64;
        let mean = values.iter().sum::<f64>() / count;
        let variance = values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>();

        Some(Self {
            width: image.width(),
            height: image.height(),
            offsets,
            values,
            mean,
            variance,
        })
    }

    /// Score one top-left position. Returns 0 when the window is flat.
    fn score_at(&self, search: &GrayImage, x: u32, y: u32) -> f32 {
        let count = self.values.len() as f64;
        let stride = search.width() as usize;
        let data = search.as_raw();
        let base = y as usize * stride + x as usize;
        let mut sum = 0.0;
        let mut sum_squared = 0.0;
        let mut cross = 0.0;

        for (index, (offset_x, offset_y)) in self.offsets.iter().enumerate() {
            let pixel = f64::from(data[base + *offset_y as usize * stride + *offset_x as usize]);
            sum += pixel;
            sum_squared += pixel * pixel;
            cross += pixel * self.values[index];
        }

        let mean = sum / count;
        let variance = sum_squared - count * mean * mean;
        if variance <= 1e-9 || self.variance <= 1e-9 {
            return 0.0;
        }

        let covariance = cross - count * mean * self.mean;
        let score = covariance / (variance * self.variance).sqrt();
        score.clamp(0.0, 1.0) as f32
    }
}

/// Best top-left position inside an inclusive position range.
fn scan(
    template: &PreparedTemplate,
    search: &GrayImage,
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
) -> Option<(f32, u32, u32)> {
    if template.width == 0
        || template.height == 0
        || template.width > search.width()
        || template.height > search.height()
    {
        return None;
    }

    let limit_x = search.width() - template.width;
    let limit_y = search.height() - template.height;
    let start_x = min_x.min(limit_x);
    let end_x = max_x.min(limit_x);
    let start_y = min_y.min(limit_y);
    let end_y = max_y.min(limit_y);
    if start_x > end_x || start_y > end_y {
        return None;
    }

    let mut best = (-1.0_f32, start_x, start_y);
    for y in start_y..=end_y {
        for x in start_x..=end_x {
            let score = template.score_at(search, x, y);
            if score > best.0 {
                best = (score, x, y);
            }
        }
    }

    (best.0 >= 0.0).then_some(best)
}

fn downsample(image: &GrayImage, factor: u32) -> GrayImage {
    if factor <= 1 {
        return image.clone();
    }

    let width = (image.width() / factor).max(1);
    let height = (image.height() / factor).max(1);
    let mut output = GrayImage::from_pixel(width, height, Luma([0]));

    for y in 0..height {
        for x in 0..width {
            let mut sum = 0u32;
            let mut count = 0u32;
            for offset_y in 0..factor {
                let source_y = y * factor + offset_y;
                if source_y >= image.height() {
                    break;
                }
                for offset_x in 0..factor {
                    let source_x = x * factor + offset_x;
                    if source_x >= image.width() {
                        break;
                    }
                    sum += u32::from(image.get_pixel(source_x, source_y).0[0]);
                    count += 1;
                }
            }
            let value = sum
                .checked_div(count)
                .map_or(0, |average| average.min(255) as u8);
            output.put_pixel(x, y, Luma([value]));
        }
    }

    output
}

#[derive(Debug, Clone, Copy)]
struct CoarseCandidate {
    score: f32,
    size: f32,
    letter_spacing: f32,
    x: u32,
    y: u32,
}

/// Render every size/spacing combination for one anchor, run a coarse pass on
/// a downsampled search image, then refine the best candidates at full
/// resolution.
fn best_for_anchor(text: &str, search: &GrayImage, coarse_search: &GrayImage) -> MenuAnchorResult {
    let mut candidates = Vec::new();

    for size in SIZES {
        for letter_spacing in LETTER_SPACINGS {
            let template = render_menu_text(text, size, letter_spacing);
            if template.width() > search.width() || template.height() > search.height() {
                continue;
            }

            let coarse_template = downsample(&template, COARSE_FACTOR);
            let Some(prepared) = PreparedTemplate::new(&coarse_template, MASK_THRESHOLD) else {
                continue;
            };
            let max_x = coarse_search.width().saturating_sub(1);
            let max_y = coarse_search.height().saturating_sub(1);
            let Some((score, x, y)) = scan(&prepared, coarse_search, 0, 0, max_x, max_y) else {
                continue;
            };

            candidates.push(CoarseCandidate {
                score,
                size,
                letter_spacing,
                x,
                y,
            });
        }
    }

    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
    });

    let mut best = MenuAnchorResult::empty(text);
    if candidates
        .first()
        .is_none_or(|candidate| candidate.score < COARSE_SKIP_BELOW)
    {
        return best;
    }

    for candidate in candidates.into_iter().take(COARSE_CANDIDATES) {
        let template = render_menu_text(text, candidate.size, candidate.letter_spacing);
        if template.width() > search.width() || template.height() > search.height() {
            continue;
        }
        let Some(prepared) = PreparedTemplate::new(&template, MASK_THRESHOLD) else {
            continue;
        };

        let center_x = candidate.x.saturating_mul(COARSE_FACTOR);
        let center_y = candidate.y.saturating_mul(COARSE_FACTOR);
        let min_x = center_x.saturating_sub(REFINE_RADIUS);
        let min_y = center_y.saturating_sub(REFINE_RADIUS);
        let max_x = center_x.saturating_add(REFINE_RADIUS);
        let max_y = center_y.saturating_add(REFINE_RADIUS);

        let Some((score, x, y)) = scan(&prepared, search, min_x, min_y, max_x, max_y) else {
            continue;
        };
        if score > best.score {
            best = MenuAnchorResult {
                text: text.to_string(),
                score,
                size: candidate.size,
                letter_spacing: candidate.letter_spacing,
                x,
                y,
                width: template.width(),
                height: template.height(),
            };
        }
    }

    best
}

/// Best match per anchor. Coordinates are relative to `search`.
pub fn test_menu_templates(search: &GrayImage) -> Vec<MenuAnchorResult> {
    if search.width() == 0 || search.height() == 0 {
        return MENU_ANCHORS
            .iter()
            .map(|text| MenuAnchorResult::empty(text))
            .collect();
    }

    let coarse_search = downsample(search, COARSE_FACTOR);
    MENU_ANCHORS
        .iter()
        .map(|text| best_for_anchor(text, search, &coarse_search))
        .collect()
}

/// Draw one rectangle per result on an RGB buffer. Matches at 0.5 or more are
/// green, weaker candidates amber. This is only for the diagnostic preview.
pub fn draw_debug_rectangles(
    rgb: &mut [u8],
    width: u32,
    height: u32,
    results: &[MenuAnchorResult],
) {
    if width == 0 || height == 0 || rgb.len() < (width as usize * height as usize * 3) {
        return;
    }

    for result in results {
        if result.width == 0 || result.height == 0 || result.score < 0.2 {
            continue;
        }
        let x0 = result.x.min(width - 1);
        let y0 = result.y.min(height - 1);
        let x1 = (result.x + result.width - 1).min(width - 1);
        let y1 = (result.y + result.height - 1).min(height - 1);
        let color = if result.score >= 0.5 {
            [0, 255, 0]
        } else {
            [255, 170, 0]
        };

        for x in x0..=x1 {
            set_pixel(rgb, width, height, x, y0, color);
            set_pixel(rgb, width, height, x, y1, color);
        }
        for y in y0..=y1 {
            set_pixel(rgb, width, height, x0, y, color);
            set_pixel(rgb, width, height, x1, y, color);
        }
    }
}

fn set_pixel(rgb: &mut [u8], width: u32, height: u32, x: u32, y: u32, color: [u8; 3]) {
    if x >= width || y >= height {
        return;
    }
    let offset = (y as usize * width as usize + x as usize) * 3;
    rgb[offset..offset + 3].copy_from_slice(&color);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paste(destination: &mut GrayImage, source: &GrayImage, x0: u32, y0: u32) {
        for y in 0..source.height() {
            for x in 0..source.width() {
                destination.put_pixel(x0 + x, y0 + y, *source.get_pixel(x, y));
            }
        }
    }

    #[test]
    fn renders_non_empty_menu_text() {
        let image = render_menu_text("CINÉMATIQUES", 32.0, 0.0);
        assert!(image.width() > 40);
        assert!(image.height() > 10);
        assert!(image.pixels().any(|pixel| pixel.0[0] > 128));
    }

    #[test]
    fn finds_known_template_position() {
        let template = render_menu_text("CINÉMATIQUES", 32.0, 0.0);
        let mut search = GrayImage::from_pixel(420, 160, Luma([0]));
        let (x0, y0) = (30, 40);
        paste(&mut search, &template, x0, y0);

        let results = test_menu_templates(&search);
        let result = results
            .iter()
            .find(|result| result.text == "CINÉMATIQUES")
            .expect("cinematiques result");

        assert!(result.score > 0.9, "score was {}", result.score);
        assert_eq!(result.size, 32.0);
        assert_eq!(result.letter_spacing, 0.0);
        assert!(
            result.x.abs_diff(x0) <= 2,
            "x was {} instead of {x0}",
            result.x
        );
        assert!(
            result.y.abs_diff(y0) <= 2,
            "y was {} instead of {y0}",
            result.y
        );
    }

    #[test]
    fn blank_search_scores_low() {
        let search = GrayImage::from_pixel(420, 160, Luma([0]));
        let results = test_menu_templates(&search);
        assert_eq!(results.len(), MENU_ANCHORS.len());
        for result in results {
            assert!(
                result.score < 0.05,
                "{} scored {} on a blank image",
                result.text,
                result.score
            );
        }
    }
}
