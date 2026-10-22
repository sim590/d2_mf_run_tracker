//! Shared grayscale template matching primitives.
//!
//! `PreparedTemplate`, `SearchContext` and `Scores` implement the three
//! matching metrics (intensity NCC, binary Dice and Sobel edge NCC) used by
//! both the font-generated menu diagnostic and the HUD image matcher. The
//! module is backend- and Tauri-independent, so the matching code is shared
//! instead of duplicated.

use image::{GrayImage, Luma};

/// Intensity pixels at or below this value are ignored by the NCC mask.
pub(crate) const MASK_THRESHOLD: u8 = 24;
/// Template pixels above this value form the binary mask.
pub(crate) const BINARY_TEMPLATE_THRESHOLD: u8 = 128;
/// Search pixels at or above this value are considered "on" by Dice.
pub(crate) const BINARY_SEARCH_THRESHOLD: u8 = 100;
/// Sobel magnitudes above this value form the edge mask.
pub(crate) const EDGE_TEMPLATE_THRESHOLD: u8 = 40;
/// Downsampling factor for the coarse pass. Templates are only a few pixels
/// tall in the worst case, so 2 is safer than 4.
pub(crate) const COARSE_FACTOR: u32 = 2;
/// Coarse scores below this are not worth a full-resolution refinement.
pub(crate) const COARSE_SKIP_BELOW: f32 = 0.05;
/// Search radius (in full-resolution pixels) around a coarse match.
pub(crate) const REFINE_RADIUS: u32 = 16;

/// Precomputed search data shared by every template and position.
pub(crate) struct SearchContext<'a> {
    pub(crate) gray: &'a GrayImage,
    pub(crate) binary: GrayImage,
    /// Summed-area table of `binary`, with one extra row and column.
    pub(crate) binary_integral: Vec<u64>,
    pub(crate) gradient: GrayImage,
}

impl<'a> SearchContext<'a> {
    pub(crate) fn new(gray: &'a GrayImage) -> Self {
        let (binary, binary_integral) = binary_view(gray);
        Self {
            gray,
            binary,
            binary_integral,
            gradient: sobel_magnitude(gray),
        }
    }

    /// Lightweight constructor for binary-only matching: skips the Sobel pass
    /// used by the diagnostic edge score.
    pub(crate) fn new_binary(gray: &'a GrayImage) -> Self {
        let (binary, binary_integral) = binary_view(gray);
        Self {
            gray,
            binary,
            binary_integral,
            gradient: GrayImage::new(0, 0),
        }
    }

    pub(crate) fn binary_sum(&self, x: u32, y: u32, width: u32, height: u32) -> u64 {
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
pub(crate) struct Scores {
    pub(crate) combined: f32,
    pub(crate) ncc: f32,
    pub(crate) binary: f32,
    pub(crate) edge: f32,
}

impl Scores {
    /// Metric used by the HUD image matcher: a graphic frame is best described
    /// by its edges, but a flat-but-unique patch is caught by the intensity
    /// NCC. The binary Dice is exposed for diagnostics only.
    pub(crate) fn image_metric(&self) -> f32 {
        self.ncc.max(self.edge)
    }
}

/// Masks and normalized values for the three matching metrics.
pub(crate) struct PreparedTemplate {
    pub(crate) width: u32,
    pub(crate) height: u32,
    mask_offsets: Vec<(u32, u32)>,
    mask_values: Vec<f64>,
    mask_mean: f64,
    mask_variance: f64,
    pub(crate) binary_offsets: Vec<(u32, u32)>,
    pub(crate) binary_count: u64,
    edge_offsets: Vec<(u32, u32)>,
    edge_values: Vec<f64>,
    edge_mean: f64,
    edge_variance: f64,
}

impl PreparedTemplate {
    pub(crate) fn new(image: &GrayImage) -> Option<Self> {
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
    pub(crate) fn intensity_score_at(&self, search: &GrayImage, x: u32, y: u32) -> f32 {
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
    pub(crate) fn binary_score_at(&self, context: &SearchContext<'_>, x: u32, y: u32) -> f32 {
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
    pub(crate) fn edge_score_at(&self, context: &SearchContext<'_>, x: u32, y: u32) -> f32 {
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

    pub(crate) fn scores_at(&self, context: &SearchContext<'_>, x: u32, y: u32) -> Scores {
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

pub(crate) fn correlate(
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

fn binary_view(gray: &GrayImage) -> (GrayImage, Vec<u64>) {
    let binary = GrayImage::from_fn(gray.width(), gray.height(), |x, y| {
        let value = gray.get_pixel(x, y).0[0];
        Luma([if value >= BINARY_SEARCH_THRESHOLD {
            255
        } else {
            0
        }])
    });
    let integral = build_integral(&binary);
    (binary, integral)
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

pub(crate) fn sobel_magnitude(image: &GrayImage) -> GrayImage {
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
pub(crate) fn scan_intensity(
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
pub(crate) fn scan_combined(
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

/// Best top-left position inside an inclusive position range, ranked by the
/// HUD image metric (the best of the intensity NCC and the edge NCC).
pub(crate) fn scan_image(
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
    let mut best_metric = -1.0_f32;
    let mut best_position = (start_x, start_y);

    for y in start_y..=end_y {
        for x in start_x..=end_x {
            let scores = template.scores_at(context, x, y);
            let metric = scores.image_metric();
            if metric > best_metric {
                best_metric = metric;
                best = scores;
                best_position = (x, y);
            }
        }
    }

    (best_metric >= 0.0).then_some((best, best_position.0, best_position.1))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn position_range(
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

pub(crate) fn downsample(image: &GrayImage, factor: u32) -> GrayImage {
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

/// Best match of a template against a search image.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ImageMatch {
    /// Combined HUD score: the best of the intensity and edge NCCs.
    pub(crate) score: f32,
    pub(crate) ncc: f32,
    pub(crate) binary: f32,
    pub(crate) edge: f32,
    pub(crate) x: u32,
    pub(crate) y: u32,
}

/// Matcher for an arbitrary grayscale image template (a HUD frame captured
/// from the live game). It reuses the same prepared masks and scoring metrics
/// as the menu diagnostic, but ranks positions by `Scores::image_metric`.
pub(crate) struct ImageTemplateMatcher {
    image: GrayImage,
    template: PreparedTemplate,
}

impl ImageTemplateMatcher {
    pub(crate) fn new(image: &GrayImage) -> Option<Self> {
        let template = PreparedTemplate::new(image)?;
        Some(Self {
            image: image.clone(),
            template,
        })
    }

    /// Best match over the whole search image, or `None` when the template is
    /// larger than the search or nothing is worth refining.
    pub(crate) fn best(&self, search: &GrayImage) -> Option<ImageMatch> {
        if search.width() < self.template.width || search.height() < self.template.height {
            return None;
        }

        // Coarse pass: locate the best intensity NCC on a downsampled image.
        let coarse_search = downsample(search, COARSE_FACTOR);
        let coarse_template = downsample(&self.image, COARSE_FACTOR);
        let coarse_prepared = PreparedTemplate::new(&coarse_template)?;
        let max_x = coarse_search.width().saturating_sub(1);
        let max_y = coarse_search.height().saturating_sub(1);
        let (coarse_score, coarse_x, coarse_y) =
            scan_intensity(&coarse_prepared, &coarse_search, 0, 0, max_x, max_y)?;
        if coarse_score < COARSE_SKIP_BELOW {
            return None;
        }

        // Full-resolution refinement around the coarse candidate.
        let context = SearchContext::new(search);
        let center_x = coarse_x.saturating_mul(COARSE_FACTOR);
        let center_y = coarse_y.saturating_mul(COARSE_FACTOR);
        let min_x = center_x.saturating_sub(REFINE_RADIUS);
        let min_y = center_y.saturating_sub(REFINE_RADIUS);
        let max_x = center_x.saturating_add(REFINE_RADIUS);
        let max_y = center_y.saturating_add(REFINE_RADIUS);
        let (scores, x, y) = scan_image(&self.template, &context, min_x, min_y, max_x, max_y)?;

        Some(ImageMatch {
            score: scores.image_metric(),
            ncc: scores.ncc,
            binary: scores.binary,
            edge: scores.edge,
            x,
            y,
        })
    }
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

    /// A textured frame-like template. The border values vary so the intensity
    /// and edge NCCs have variance, like a real HUD frame.
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

    #[test]
    fn image_matcher_scores_pasted_template_high() {
        let template = frame_template(40, 40);
        let mut search = GrayImage::from_pixel(200, 120, Luma([10]));
        paste(&mut search, &template, 70, 40);

        let matcher = ImageTemplateMatcher::new(&template).expect("valid template");
        let matched = matcher.best(&search).expect("a match");
        assert!(matched.score > 0.8, "score was {}", matched.score);
        assert!(matched.x.abs_diff(70) <= 2, "x was {}", matched.x);
        assert!(matched.y.abs_diff(40) <= 2, "y was {}", matched.y);
    }

    #[test]
    fn image_matcher_scores_blank_low() {
        let template = frame_template(40, 40);
        let search = GrayImage::from_pixel(200, 120, Luma([0]));

        let matcher = ImageTemplateMatcher::new(&template).expect("valid template");
        assert!(matcher.best(&search).map_or(0.0, |matched| matched.score) < 0.3);
    }

    #[test]
    fn image_matcher_scores_different_region_low() {
        let template = frame_template(40, 40);
        // A diagonal stripe field: no rectangular frame edges to correlate.
        let mut search = GrayImage::from_pixel(200, 120, Luma([0]));
        for y in 0..120 {
            for x in 0..200 {
                if (x + y) % 9 == 0 {
                    search.put_pixel(x, y, Luma([200]));
                }
            }
        }

        let matcher = ImageTemplateMatcher::new(&template).expect("valid template");
        assert!(
            matcher.best(&search).map_or(0.0, |matched| matched.score) < 0.3,
            "stripe field scored {}",
            matcher.best(&search).map_or(0.0, |matched| matched.score)
        );
    }

    #[test]
    fn image_matcher_rejects_empty_template() {
        let blank = GrayImage::from_pixel(40, 40, Luma([0]));
        assert!(ImageTemplateMatcher::new(&blank).is_none());
    }

    #[test]
    fn image_matcher_rejects_oversized_template() {
        let template = frame_template(40, 40);
        let search = GrayImage::from_pixel(20, 20, Luma([0]));
        let matcher = ImageTemplateMatcher::new(&template).expect("valid template");
        assert!(matcher.best(&search).is_none());
    }
}
