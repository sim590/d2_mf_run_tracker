//! Font-generated menu templates from the bundled Exocet Blizzard fonts.
//!
//! The diagnostic renders every anchor/size/spacing combination and scores
//! them with three metrics. The live loop uses a single calibrated template
//! and only its binary Dice score, with the existing detector still consuming
//! the synthetic menu signal.

use std::cmp::Ordering;
use std::sync::OnceLock;

use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use fontdue::Font;
use image::{GrayImage, Luma};
use serde::Serialize;

use super::matching::{
    downsample, scan_combined, scan_intensity, PreparedTemplate, SearchContext, COARSE_FACTOR,
    COARSE_SKIP_BELOW, REFINE_RADIUS,
};

static MEDIUM_BYTES: &[u8] = include_bytes!("../../../src/assets/exocet-blizzard-medium.ttf");
static HEAVY_BYTES: &[u8] = include_bytes!("../../../src/assets/exocet-blizzard-heavy.ttf");

/// Menu buttons used for detection. The left column strings are unique to the
/// character selection screen (unlike OPTIONS, which the pause menu shows).
pub const MENU_ANCHORS: [&str; 5] = [
    "CINÉMATIQUES",
    "CRÉDITS",
    "QUITTER DIABLO II",
    "JOUER",
    "CRÉER NOUVEAU",
];

/// The three screen regions that must all show a menu anchor before the menu
/// is considered visible. A tooltip can overlap one region by chance, but not
/// all three at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAnchorGroup {
    Left,
    Center,
    Right,
}

impl MenuAnchorGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            MenuAnchorGroup::Left => "left",
            MenuAnchorGroup::Center => "center",
            MenuAnchorGroup::Right => "right",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "left" => Some(MenuAnchorGroup::Left),
            "center" | "centre" => Some(MenuAnchorGroup::Center),
            "right" => Some(MenuAnchorGroup::Right),
            _ => None,
        }
    }

    pub fn for_anchor(text: &str) -> Option<Self> {
        match text {
            "CINÉMATIQUES" | "CRÉDITS" | "QUITTER DIABLO II" => Some(MenuAnchorGroup::Left),
            "JOUER" => Some(MenuAnchorGroup::Center),
            "CRÉER NOUVEAU" => Some(MenuAnchorGroup::Right),
            _ => None,
        }
    }
}

/// All three groups must have at least one visible anchor.
pub fn menu_groups_visible(left: bool, center: bool, right: bool) -> bool {
    left && center && right
}

const SIZES: [f32; 10] = [16.0, 18.0, 20.0, 22.0, 24.0, 28.0, 32.0, 36.0, 40.0, 44.0];
const LETTER_SPACINGS: [f32; 4] = [0.0, 1.0, 2.0, 3.0];

/// Empty pixels around the rendered text so the crop is visually verifiable.
const RENDER_PADDING: u32 = 3;
/// How many coarse combinations are refined at full resolution.
const COARSE_CANDIDATES: usize = 24;

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
    pub group: String,
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
            group: MenuAnchorGroup::for_anchor(text)
                .map(|group| group.as_str().to_string())
                .unwrap_or_default(),
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
                group: MenuAnchorGroup::for_anchor(text)
                    .map(|group| group.as_str().to_string())
                    .unwrap_or_default(),
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

/// A single calibrated template used by the live auto-detect loop. Only the
/// binary Dice score is used, so no multi-size search happens per frame.
pub struct MenuTemplateMatcher {
    template: PreparedTemplate,
}

impl MenuTemplateMatcher {
    pub fn new(text: &str, font: &str, size: f32, letter_spacing: f32) -> Option<Self> {
        let font_kind = match font.trim().to_ascii_lowercase().as_str() {
            "heavy" => FontKind::Heavy,
            "medium" | "" => FontKind::Medium,
            _ => return None,
        };
        if !size.is_finite() || size <= 0.0 || !letter_spacing.is_finite() {
            return None;
        }

        let image = render_menu_text(text, size, letter_spacing, font_kind);
        let template = PreparedTemplate::new(&image)?;
        if template.binary_count == 0 {
            return None;
        }

        Some(Self { template })
    }

    /// Best binary Dice score over the whole search image.
    pub fn score(&self, search: &GrayImage) -> f32 {
        if search.width() < self.template.width || search.height() < self.template.height {
            return 0.0;
        }

        let context = SearchContext::new_binary(search);
        let max_x = search.width() - self.template.width;
        let max_y = search.height() - self.template.height;
        let template_count = self.template.binary_count as f64;
        let mut best = 0.0_f32;

        for y in 0..=max_y {
            for x in 0..=max_x {
                // Upper bound on Dice: intersection can never exceed the
                // smaller of the two on-pixel counts. This skips most dark or
                // weakly lit positions before walking the template mask.
                let search_count =
                    context.binary_sum(x, y, self.template.width, self.template.height);
                if search_count == 0 {
                    continue;
                }
                let max_dice = 2.0 * search_count.min(self.template.binary_count) as f64
                    / (template_count + search_count as f64);
                if max_dice <= f64::from(best) {
                    continue;
                }

                let score = self.template.binary_score_at(&context, x, y);
                if score > best {
                    best = score;
                }
            }
        }

        best
    }
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
    fn matcher_scores_pasted_template() {
        let template = render_menu_text("CINÉMATIQUES", 20.0, 1.0, FontKind::Medium);
        let mut search = GrayImage::from_pixel(384, 58, Luma([0]));
        paste(&mut search, &template, 40, 15);

        let matcher =
            MenuTemplateMatcher::new("CINÉMATIQUES", "MEDIUM", 20.0, 1.0).expect("valid matcher");
        assert!(matcher.score(&search) > 0.9);
    }

    #[test]
    fn menu_anchor_groups_map_correctly() {
        assert_eq!(
            MenuAnchorGroup::for_anchor("CINÉMATIQUES"),
            Some(MenuAnchorGroup::Left)
        );
        assert_eq!(
            MenuAnchorGroup::for_anchor("CRÉDITS"),
            Some(MenuAnchorGroup::Left)
        );
        assert_eq!(
            MenuAnchorGroup::for_anchor("QUITTER DIABLO II"),
            Some(MenuAnchorGroup::Left)
        );
        assert_eq!(
            MenuAnchorGroup::for_anchor("JOUER"),
            Some(MenuAnchorGroup::Center)
        );
        assert_eq!(
            MenuAnchorGroup::for_anchor("CRÉER NOUVEAU"),
            Some(MenuAnchorGroup::Right)
        );
        assert_eq!(MenuAnchorGroup::for_anchor("OPTIONS"), None);

        assert_eq!(MenuAnchorGroup::parse("LEFT"), Some(MenuAnchorGroup::Left));
        assert_eq!(
            MenuAnchorGroup::parse("centre"),
            Some(MenuAnchorGroup::Center)
        );
        assert_eq!(
            MenuAnchorGroup::parse("right"),
            Some(MenuAnchorGroup::Right)
        );
        assert_eq!(MenuAnchorGroup::parse("somewhere"), None);
    }

    #[test]
    fn menu_visibility_requires_all_three_groups() {
        assert!(!menu_groups_visible(false, false, false));
        assert!(!menu_groups_visible(true, false, false));
        assert!(!menu_groups_visible(true, false, true));
        assert!(!menu_groups_visible(false, true, true));
        assert!(menu_groups_visible(true, true, true));
    }

    #[test]
    fn matcher_scores_every_menu_anchor() {
        for text in MENU_ANCHORS {
            let template = render_menu_text(text, 20.0, 1.0, FontKind::Medium);
            let mut search = GrayImage::from_pixel(384, 58, Luma([0]));
            paste(&mut search, &template, 40, 15);

            let matcher =
                MenuTemplateMatcher::new(text, "medium", 20.0, 1.0).expect("valid matcher");
            let score = matcher.score(&search);
            assert!(score > 0.9, "{text} scored {score}");
        }
    }

    #[test]
    fn matcher_blank_search_scores_low() {
        let search = GrayImage::from_pixel(384, 58, Luma([0]));
        let matcher =
            MenuTemplateMatcher::new("CINÉMATIQUES", "medium", 20.0, 1.0).expect("valid matcher");
        assert!(matcher.score(&search) < 0.05);
    }

    #[test]
    fn matcher_scores_different_region_low_and_validates_input() {
        // An unrelated small bright blob is not the calibrated text.
        let mut search = GrayImage::from_pixel(384, 58, Luma([0]));
        for y in 20..32 {
            for x in 60..72 {
                search.put_pixel(x, y, Luma([255]));
            }
        }

        let matcher =
            MenuTemplateMatcher::new("CINÉMATIQUES", "medium", 20.0, 1.0).expect("valid matcher");
        // Below the 0.3 live decision threshold, unlike an exact match (>0.9).
        assert!(
            matcher.score(&search) < 0.25,
            "unrelated region scored {}",
            matcher.score(&search)
        );

        assert!(MenuTemplateMatcher::new("CINÉMATIQUES", "nope", 20.0, 0.0).is_none());
        assert!(MenuTemplateMatcher::new("CINÉMATIQUES", "medium", 0.0, 0.0).is_none());
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
        assert_eq!(one[0].group, "left");

        let center = test_menu_templates(&search, Some("JOUER"));
        assert_eq!(center.len(), 1);
        assert_eq!(center[0].group, "center");

        let right = test_menu_templates(&search, Some("CRÉER NOUVEAU"));
        assert_eq!(right.len(), 1);
        assert_eq!(right[0].group, "right");

        let all = test_menu_templates(&search, None);
        assert_eq!(all.len(), MENU_ANCHORS.len());

        let empty = test_menu_templates(&search, Some(""));
        assert_eq!(empty.len(), MENU_ANCHORS.len());

        let unknown = test_menu_templates(&search, Some("OPTIONS"));
        assert!(unknown.is_empty());
    }
}
