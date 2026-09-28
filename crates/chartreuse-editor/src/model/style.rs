//! Annotation styling.

use chartreuse_core::color::Rgba8;

/// How an annotation is drawn. Every annotation carries a full `Style`; each
/// kind reads the fields that apply to it (text ignores `stroke_width`, strokes
/// ignore `font_size`), so restyling a mixed selection is uniform.
///
/// Lengths are in base-image pixels, like all document coordinates. They should
/// be finite and positive; geometry treats non-positive values as zero.
///
/// # Strokes
///
/// Every stroke (a line, an arrow's shaft, a rectangle's or ellipse's outline) is
/// `stroke_width` wide, centered on the annotation's geometry, with **round
/// caps and round joins**: it covers exactly the points within
/// `stroke_width / 2` of the stroked path. Filled parts (an arrowhead) are
/// filled only, never stroked, so their corners stay sharp.
///
/// This is the editor's one stroke geometry. The model's hit areas and bounds
/// are derived from it, and the canvas and flatten must draw it, so what the
/// user sees, clicks, selects, and exports agree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Stroke color for lines and outlines, glyph color for text.
    pub color: Rgba8,
    /// Stroke width; see *Strokes* above.
    pub stroke_width: f32,
    /// Text size: the em size of the font (cosmic-text `Metrics::font_size`).
    pub font_size: f32,
    /// How a blur region obscures what is beneath it.
    pub blur: BlurMode,
}

/// How a [blur region](super::BlurRegion) obscures what is beneath it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlurMode {
    /// A mosaic of [`BlurRegion::PIXELATE_BLOCK`](super::BlurRegion::PIXELATE_BLOCK)-pixel
    /// squares, each the average of what it covers.
    #[default]
    Pixelate,
    /// A Gaussian-like blur of radius
    /// [`BlurRegion::BLUR_RADIUS`](super::BlurRegion::BLUR_RADIUS).
    Gaussian,
}

impl BlurMode {
    /// Both modes, in the order the toolbar offers them.
    pub const ALL: [Self; 2] = [Self::Pixelate, Self::Gaussian];

    /// The name the toolbar shows.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pixelate => "Pixelate",
            Self::Gaussian => "Blur",
        }
    }
}

impl Style {
    /// The default annotation color: a saturated red that reads on most
    /// screenshots.
    pub const DEFAULT_COLOR: Rgba8 = Rgba8::from_rgb_hex(0xff_3b_30);

    /// A copy with every field that `patch` sets replaced.
    #[must_use]
    pub fn patched(self, patch: &StylePatch) -> Self {
        Self {
            color: patch.color.unwrap_or(self.color),
            stroke_width: patch.stroke_width.unwrap_or(self.stroke_width),
            font_size: patch.font_size.unwrap_or(self.font_size),
            blur: patch.blur.unwrap_or(self.blur),
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color: Self::DEFAULT_COLOR,
            stroke_width: 4.0,
            font_size: 24.0,
            blur: BlurMode::default(),
        }
    }
}

/// A partial style change: `None` fields are left alone, so a restyle panel can
/// change one attribute across a selection without flattening the others.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StylePatch {
    pub color: Option<Rgba8>,
    pub stroke_width: Option<f32>,
    pub font_size: Option<f32>,
    pub blur: Option<BlurMode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_replaces_only_the_fields_it_sets() {
        let style = Style::default();
        let patch = StylePatch {
            stroke_width: Some(9.0),
            ..StylePatch::default()
        };
        let patched = style.patched(&patch);
        assert_eq!(patched.stroke_width, 9.0);
        assert_eq!(patched.color, style.color);
        assert_eq!(patched.font_size, style.font_size);
        assert_eq!(style.patched(&StylePatch::default()), style);
    }
}
