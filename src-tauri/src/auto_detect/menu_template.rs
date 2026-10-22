//! One-shot diagnostic: render menu anchor strings from the bundled Exocet
//! Blizzard fonts and look for them in a captured menu region.
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

static MEDIUM_BYTES: &[u8] = include_bytes!("../../../src/assets/exocet-blizzard-medium.ttf");
static HEAVY_BYTES: &[u8] = include_bytes!("../../../src/assets/exocet-blizzard-heavy.ttf");

/// Left column buttons on the character selection screen. The strings are
/// unique to that screen (unlike OPTIONS, which the pause menu also shows).
pub const MENU_ANCHORS: [&str; 3] = ["CINÉMATIQUES", "CRÉDITS", "QUITTER DIABLO II"];

const SIZES: [f32; 10] = [16.0, 18.0, 20.0, 22.0, 24.0, 28.0, 32.0, 36.0, 40.0, 44.0];
const LETTER_SPACINGS: [f32; 4] = [0.0, 1.0, 2.0, 3.0];

/// Empty pixels around the rendered text so the crop is visually verifiable.
const RENDER_PADDING: u32 = 3;
/// Intensity pixels at or below this value are ignored by the NCC mask.
const MASK_THRESHOLD: u8 = 24;
/// Template pixels above this value form the binary mask.
const BINARY_TEMPLATE_THRESHOLD: u8 = 128;
/// Search pixels at or above this value are considered "on" by Dice.
const BINARY_SEARCH_THRESHOLD: u8 = 100;
/// Sobel magnitudes above this value form the edge mask.
const EDGE_TEMPLATE_THRESHOLD: u8 = 40;
/// Downsampling factor for the coarse pass. Small templates are only a few
/// pixels tall, so 2 is safer than 4.
const COARSE_FACTOR: u32 = 2;
/// How many coarse combinations are refined at full resolution.
const COARSE_CANDIDATES: usize = 24;
/// Coarse scores below this are not worth a full-resolution refinement.
const COARSE_SKIP_BELOW: f32 = 0.05;
/// Search radius (in full-resolution pixels) around a coarse match.
const REFINE_RADIUS: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontKind {
    Medium,
    Heavy,
}

impl FontKind {
    const ALL: [FontKind; 2] = [FontKind::Medium, FontKind::Heavy];

    fn as_str(self) -> &'static str {
        match self {
            FontKind::Medium => "medium",
            FontKind::Heavy => "heavy",
        }
    }

    fn bytes(self) -> &'static [u8] {
        match self {
            FontKind::Medium => MEDIUM_BYTES,
            FontKind::Heavy => HEAVY_BYTES,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MenuAnchorResult {
    pub text: String,
    pub font: String,
    /// Combined score: the best of the intensity, binary and edge scores.
    pub score: f32,
    pub ncc_score: f32,
    pub binary_score: f32,
    pub edge_score: f32,
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
            font: String::new(),
            score: 0.0,
            ncc_score: 0.0,
            binary_score: 0.0,
            edge_score: 0.0,
            size: 0.0,
            letter_spacing: 0.0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }
}

fn font(kind: FontKind) -> &'static Font {
    static MEDIUM: OnceLock<Font> = OnceLock::new();
    static HEAVY: OnceLock<Font> = OnceLock::new();

    let cell = match kind {
        FontKind::Medium => &MEDIUM,
        FontKind::Heavy => &HEAVY,
    };
    cell.get_or_init(|| {
        Font::from_bytes(kind.bytes(), fontdue::FontSettings::default())
            .expect("the bundled Exocet Blizzard font is valid")
    })
}

/// Render `text` to a tightly cropped grayscale image (white glyphs on black).
///
/// `fontdue` 0.9 has no letter spacing option in its layout API, so glyphs are
/// laid out normally and each subsequent glyph is shifted by `letter_spacing`.
/// Kerning between glyphs is lost when spacing is non-zero, which is fine for
/// this diagnostic.
pub fn render_menu_text(
    text: &str,
    size_px: f32,
    letter_spacing: f32,
    font_kind: FontKind,
) -> GrayImage {
    let font = font(font_kind);
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

/// Precomputed search data shared by every template and position.
struct SearchContext<'a> {
    gray: &'a GrayImage,
    binary: GrayImage,
    /// Summed-area table of `binary`, with one extra row and column.
    binary_integral: Vec<u64>,
    gradient: GrayImage,
}

impl<'a> SearchContext<'a> {
    fn new(gray: &'a GrayImage) -> Self {
        let binary = GrayImage::from_fn(gray.width(), gray.height(), |x, y| {
            let value = gray.get_pixel(x, y).0[0];
            Luma([if value >= BINARY_SEARCH_THRESHOLD {
                255
            } else {
                0
            }])
        });
        let binary_integral = build_integral(&binary);
        let gradient = sobel_magnitude(gray);
        Self {
            gray,
            binary,
            binary_integral,
            gradient,
        }
    }

    fn binary_sum(&self, x: u32, y: u32, width: u32, height: u32) -> u64 {
        let stride = self.binary.width() as usize + 1;
        let x0 = x as usize;
        let y0 = y as usize;
        let x1 = (x + width) as usize;
        let y1 = (y + height) as usize;

        self.binary_integral[y1 * stride + x1] + self.binary_integral[y0 * stride + x0]
            - self.binary_integral[y0 * stride + x1]
            - self.binary_integral[y1 * stride + x0]
    }
}

#[derive(Debug, Clone, Copy)]
struct Scores {
    combined: f32,
    ncc: f32,
    binary: f32,
    edge: f32,
}

/// Masks and normalized values for the three matching metrics.
struct PreparedTemplate {
    width: u32,
    height: u32,
    mask_offsets: Vec<(u32, u32)>,
    mask_values: Vec<f64>,
    mask_mean: f64,
    mask_variance: f64,
    binary_offsets: Vec<(u32, u32)>,
    binary_count: u64,
    edge_offsets: Vec<(u32, u32)>,
    edge_values: Vec<f64>,
    edge_mean: f64,
    edge_variance: f64,
}

impl PreparedTemplate {
    fn new(image: &GrayImage) -> Option<Self> {
        let mut mask_offsets = Vec::new();
        let mut mask_values = Vec::new();
        let mut binary_offsets = Vec::new();
        for y in 0..image.height() {
            for x in 0..image.width() {
                let value = image.get_pixel(x, y).0[0];
                if value > MASK_THRESHOLD {
                    mask_offsets.push((x, y));
                    mask_values.push(f64::from(value));
                }
                if value > BINARY_TEMPLATE_THRESHOLD {
                    binary_offsets.push((x, y));
                }
            }
        }
        if mask_values.is_empty() {
            return None;
        }

        let mask_count = mask_values.len() as f64;
        let mask_mean = mask_values.iter().sum::<f64>() / mask_count;
        let mask_variance = mask_values
            .iter()
            .map(|value| (value - mask_mean).powi(2))
            .sum::<f64>();

        let gradient = sobel_magnitude(image);
        let mut edge_offsets = Vec::new();
        let mut edge_values = Vec::new();
        for y in 0..gradient.height() {
            for x in 0..gradient.width() {
                let value = gradient.get_pixel(x, y).0[0];
                if value > EDGE_TEMPLATE_THRESHOLD {
                    edge_offsets.push((x, y));
                    edge_values.push(f64::from(value));
                }
            }
        }
        let edge_count = edge_values.len() as f64;
        let (edge_mean, edge_variance) = if edge_count > 0.0 {
            let mean = edge_values.iter().sum::<f64>() / edge_count;
            let variance = edge_values
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>();
            (mean, variance)
        } else {
            (0.0, 0.0)
        };

        Some(Self {
            width: image.width(),
            height: image.height(),
            mask_offsets,
            mask_values,
            mask_mean,
            mask_variance,
            binary_count: binary_offsets.len() as u64,
            binary_offsets,
            edge_offsets,
            edge_values,
            edge_mean,
            edge_variance,
        })
    }

    /// NCC over the intensity mask. Returns 0 when either window is flat.
    fn intensity_score_at(&self, search: &GrayImage, x: u32, y: u32) -> f32 {
        let count = self.mask_values.len() as f64;
        let stride = search.width() as usize;
        let data = search.as_raw();
        let base = y as usize * stride + x as usize;
        let mut sum = 0.0;
        let mut sum_squared = 0.0;
        let mut cross = 0.0;

        for (index, (offset_x, offset_y)) in self.mask_offsets.iter().enumerate() {
            let pixel = f64::from(data[base + *offset_y as usize * stride + *offset_x as usize]);
            sum += pixel;
            sum_squared += pixel * pixel;
            cross += pixel * self.mask_values[index];
        }

        correlate(
            sum,
            sum_squared,
            cross,
            count,
            self.mask_mean,
            self.mask_variance,
        )
    }

    /// Dice coefficient between the template's glyph mask and the thresholded
    /// search patch. The search's on-pixel count uses a summed-area table, so
    /// only the template mask is walked per position.
    fn binary_score_at(&self, context: &SearchContext<'_>, x: u32, y: u32) -> f32 {
        if self.binary_count == 0 {
            return 0.0;
        }

        let stride = context.binary.width() as usize;
        let data = context.binary.as_raw();
        let base = y as usize * stride + x as usize;
        let mut intersection = 0u64;
        for (offset_x, offset_y) in &self.binary_offsets {
            if data[base + *offset_y as usize * stride + *offset_x as usize] > 0 {
                intersection += 1;
            }
        }

        let search_count = context.binary_sum(x, y, self.width, self.height);
        let denominator = self.binary_count + search_count;
        if denominator == 0 {
            return 0.0;
        }

        (2.0 * intersection as f64 / denominator as f64).min(1.0) as f32
    }

    /// NCC over the Sobel edge mask.
    fn edge_score_at(&self, context: &SearchContext<'_>, x: u32, y: u32) -> f32 {
        if self.edge_offsets.is_empty() || self.edge_variance <= 1e-9 {
            return 0.0;
        }

        let count = self.edge_values.len() as f64;
        let stride = context.gradient.width() as usize;
        let data = context.gradient.as_raw();
        let base = y as usize * stride + x as usize;
        let mut sum = 0.0;
        let mut sum_squared = 0.0;
        let mut cross = 0.0;

        for (index, (offset_x, offset_y)) in self.edge_offsets.iter().enumerate() {
            let pixel = f64::from(data[base + *offset_y as usize * stride + *offset_x as usize]);
            sum += pixel;
            sum_squared += pixel * pixel;
            cross += pixel * self.edge_values[index];
        }

        correlate(
            sum,
            sum_squared,
            cross,
            count,
            self.edge_mean,
            self.edge_variance,
        )
    }

    fn scores_at(&self, context: &SearchContext<'_>, x: u32, y: u32) -> Scores {
        let ncc = self.intensity_score_at(context.gray, x, y);
        let binary = self.binary_score_at(context, x, y);
        let edge = self.edge_score_at(context, x, y);
        Scores {
            combined: ncc.max(binary).max(edge),
            ncc,
            binary,
            edge,
        }
    }
}

fn correlate(
    sum: f64,
    sum_squared: f64,
    cross: f64,
    count: f64,
    template_mean: f64,
    template_variance: f64,
) -> f32 {
    let mean = sum / count;
    let variance = sum_squared - count * mean * mean;
    if variance <= 1e-9 || template_variance <= 1e-9 {
        return 0.0;
    }

    let covariance = cross - count * mean * template_mean;
    let score = covariance / (variance * template_variance).sqrt();
    score.clamp(0.0, 1.0) as f32
}

fn build_integral(binary: &GrayImage) -> Vec<u64> {
    let width = binary.width() as usize;
    let height = binary.height() as usize;
    let stride = width + 1;
    let mut integral = vec![0u64; stride * (height + 1)];
    let data = binary.as_raw();

    for y in 0..height {
        let mut row_sum = 0u64;
        for x in 0..width {
            row_sum += u64::from(data[y * width + x] / 255);
            integral[(y + 1) * stride + (x + 1)] = integral[y * stride + (x + 1)] + row_sum;
        }
    }

    integral
}

fn sobel_magnitude(image: &GrayImage) -> GrayImage {
    let width = image.width();
    let height = image.height();
    let mut output = GrayImage::from_pixel(width, height, Luma([0]));
    if width < 3 || height < 3 {
        return output;
    }

    let stride = width as usize;
    let data = image.as_raw();
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let index = y as usize * stride + x as usize;
            let top_left = data[index - stride - 1] as i32;
            let top = data[index - stride] as i32;
            let top_right = data[index - stride + 1] as i32;
            let left = data[index - 1] as i32;
            let right = data[index + 1] as i32;
            let bottom_left = data[index + stride - 1] as i32;
            let bottom = data[index + stride] as i32;
            let bottom_right = data[index + stride + 1] as i32;

            let gradient_x =
                -top_left + top_right - 2 * left + 2 * right - bottom_left + bottom_right;
            let gradient_y =
                -top_left - 2 * top - top_right + bottom_left + 2 * bottom + bottom_right;
            let magnitude = ((gradient_x * gradient_x + gradient_y * gradient_y) as f32).sqrt();
            output.put_pixel(x, y, Luma([magnitude.min(255.0) as u8]));
        }
    }

    output
}

/// Best top-left position inside an inclusive position range, ranked by the
/// intensity NCC only (used by the coarse pass).
fn scan_intensity(
    template: &PreparedTemplate,
    search: &GrayImage,
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
) -> Option<(f32, u32, u32)> {
    let (start_x, end_x, start_y, end_y) =
        position_range(template, search, min_x, min_y, max_x, max_y)?;
    let mut best = (-1.0_f32, start_x, start_y);
    for y in start_y..=end_y {
        for x in start_x..=end_x {
            let score = template.intensity_score_at(search, x, y);
            if score > best.0 {
                best = (score, x, y);
            }
        }
    }

    (best.0 >= 0.0).then_some(best)
}

/// Best top-left position inside an inclusive position range, ranked by the
/// combined score (used by the full-resolution pass).
fn scan_combined(
    template: &PreparedTemplate,
    context: &SearchContext<'_>,
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
) -> Option<(Scores, u32, u32)> {
    let (start_x, end_x, start_y, end_y) =
        position_range(template, context.gray, min_x, min_y, max_x, max_y)?;
    let mut best = Scores {
        combined: -1.0,
        ncc: 0.0,
        binary: 0.0,
        edge: 0.0,
    };
    let mut best_position = (start_x, start_y);

    for y in start_y..=end_y {
        for x in start_x..=end_x {
            let scores = template.scores_at(context, x, y);
            if scores.combined > best.combined {
                best = scores;
                best_position = (x, y);
            }
        }
    }

    (best.combined >= 0.0).then_some((best, best_position.0, best_position.1))
}

#[allow(clippy::too_many_arguments)]
fn position_range(
    template: &PreparedTemplate,
    search: &GrayImage,
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
) -> Option<(u32, u32, u32, u32)> {
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

    Some((start_x, end_x, start_y, end_y))
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
    font: FontKind,
    size: f32,
    letter_spacing: f32,
    x: u32,
    y: u32,
}

/// Render every font/size/spacing combination for one anchor, run a coarse
/// pass on a downsampled search image, then refine the best combinations at
/// full resolution with all three metrics.
fn best_for_anchor(
    text: &str,
    search: &GrayImage,
    coarse_search: &GrayImage,
    context: &SearchContext<'_>,
) -> MenuAnchorResult {
    let mut candidates = Vec::new();

    for font_kind in FontKind::ALL {
        for size in SIZES {
            for letter_spacing in LETTER_SPACINGS {
                let template = render_menu_text(text, size, letter_spacing, font_kind);
                if template.width() > search.width() || template.height() > search.height() {
                    continue;
                }

                let coarse_template = downsample(&template, COARSE_FACTOR);
                let Some(prepared) = PreparedTemplate::new(&coarse_template) else {
                    continue;
                };
                let max_x = coarse_search.width().saturating_sub(1);
                let max_y = coarse_search.height().saturating_sub(1);
                let Some((score, x, y)) =
                    scan_intensity(&prepared, coarse_search, 0, 0, max_x, max_y)
                else {
                    continue;
                };

                candidates.push(CoarseCandidate {
                    score,
                    font: font_kind,
                    size,
                    letter_spacing,
                    x,
                    y,
                });
            }
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
        let template = render_menu_text(
            text,
            candidate.size,
            candidate.letter_spacing,
            candidate.font,
        );
        if template.width() > search.width() || template.height() > search.height() {
            continue;
        }
        let Some(prepared) = PreparedTemplate::new(&template) else {
            continue;
        };

        let center_x = candidate.x.saturating_mul(COARSE_FACTOR);
        let center_y = candidate.y.saturating_mul(COARSE_FACTOR);
        let min_x = center_x.saturating_sub(REFINE_RADIUS);
        let min_y = center_y.saturating_sub(REFINE_RADIUS);
        let max_x = center_x.saturating_add(REFINE_RADIUS);
        let max_y = center_y.saturating_add(REFINE_RADIUS);

        let Some((scores, x, y)) = scan_combined(&prepared, context, min_x, min_y, max_x, max_y)
        else {
            continue;
        };
        if scores.combined > best.score {
            best = MenuAnchorResult {
                text: text.to_string(),
                font: candidate.font.as_str().to_string(),
                score: scores.combined,
                ncc_score: scores.ncc,
                binary_score: scores.binary,
                edge_score: scores.edge,
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

/// Best match per selected anchor. `anchor` filters by exact text; an empty
/// string behaves like `None` (all anchors). Coordinates are relative to
/// `search`.
pub fn test_menu_templates(search: &GrayImage, anchor: Option<&str>) -> Vec<MenuAnchorResult> {
    let selected: Vec<&str> = match anchor.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => MENU_ANCHORS
            .iter()
            .copied()
            .filter(|text| *text == value)
            .collect(),
        None => MENU_ANCHORS.to_vec(),
    };

    if search.width() == 0 || search.height() == 0 || selected.is_empty() {
        return selected
            .iter()
            .map(|text| MenuAnchorResult::empty(text))
            .collect();
    }

    let context = SearchContext::new(search);
    let coarse_search = downsample(search, COARSE_FACTOR);
    selected
        .iter()
        .map(|text| best_for_anchor(text, search, &coarse_search, &context))
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
        let image = render_menu_text("CINÉMATIQUES", 32.0, 0.0, FontKind::Medium);
        assert!(image.width() > 40);
        assert!(image.height() > 10);
        assert!(image.pixels().any(|pixel| pixel.0[0] > 128));

        let heavy = render_menu_text("CINÉMATIQUES", 32.0, 0.0, FontKind::Heavy);
        assert!(heavy.width() > 40);
        assert!(heavy.height() > 10);
    }

    #[test]
    fn finds_known_template_position() {
        let template = render_menu_text("CINÉMATIQUES", 32.0, 0.0, FontKind::Medium);
        let mut search = GrayImage::from_pixel(420, 160, Luma([0]));
        let (x0, y0) = (30, 40);
        paste(&mut search, &template, x0, y0);

        let results = test_menu_templates(&search, Some("CINÉMATIQUES"));
        assert_eq!(results.len(), 1);
        let result = &results[0];

        assert!(result.score > 0.9, "score was {}", result.score);
        assert!(result.ncc_score > 0.9, "ncc was {}", result.ncc_score);
        assert!(
            result.binary_score > 0.85,
            "binary was {}",
            result.binary_score
        );
        assert!(result.edge_score > 0.9, "edge was {}", result.edge_score);
        assert_eq!(result.font, "medium");
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
    fn finds_small_template_in_menu_crop() {
        // Mirrors the real character-selection crop: ~207x18 px of text in a
        // 384x58 region, matching the ~20 px size the user measured.
        let template = render_menu_text("CINÉMATIQUES", 20.0, 0.0, FontKind::Medium);
        let mut search = GrayImage::from_pixel(384, 58, Luma([0]));
        let (x0, y0) = (40, 15);
        paste(&mut search, &template, x0, y0);

        let results = test_menu_templates(&search, Some("CINÉMATIQUES"));
        assert_eq!(results.len(), 1);
        let result = &results[0];

        assert!(result.score > 0.9, "score was {}", result.score);
        assert_eq!(result.font, "medium");
        assert_eq!(result.size, 20.0);
        assert!(result.x.abs_diff(x0) <= 2);
        assert!(result.y.abs_diff(y0) <= 2);
    }

    #[test]
    fn blank_search_scores_low() {
        let search = GrayImage::from_pixel(420, 160, Luma([0]));
        let results = test_menu_templates(&search, None);
        assert_eq!(results.len(), MENU_ANCHORS.len());
        for result in results {
            assert!(
                result.score < 0.05,
                "{} scored {} on a blank image",
                result.text,
                result.score
            );
            assert!(result.ncc_score < 0.05);
            assert!(result.binary_score < 0.05);
            assert!(result.edge_score < 0.05);
        }
    }

    #[test]
    fn filters_to_one_anchor() {
        // Tiny image: every template is skipped, so this only exercises the
        // anchor filter without paying for a full match.
        let search = GrayImage::from_pixel(16, 16, Luma([0]));

        let one = test_menu_templates(&search, Some("CRÉDITS"));
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].text, "CRÉDITS");

        let all = test_menu_templates(&search, None);
        assert_eq!(all.len(), MENU_ANCHORS.len());

        let empty = test_menu_templates(&search, Some(""));
        assert_eq!(empty.len(), MENU_ANCHORS.len());

        let unknown = test_menu_templates(&search, Some("OPTIONS"));
        assert!(unknown.is_empty());
    }
}
