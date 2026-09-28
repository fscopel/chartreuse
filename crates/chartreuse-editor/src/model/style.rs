//! Annotation styling.

use chartreuse_core::color::Rgba8;

/// How an annotation is drawn. Every annotation carries a full `Style`; each
/// kind reads the fields that apply to it
/// ([`Shape::style_fields`](super::Shape::style_fields): text ignores
/// `stroke_width`, strokes ignore `font_size`), and restyling changes only
/// those.
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
            stroke_width: 8.0,
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

impl StylePatch {
    /// The patch with only the fields in `fields` kept.
    #[must_use]
    pub fn only(self, fields: StyleFields) -> Self {
        Self {
            color: self.color.filter(|_| fields.color),
            stroke_width: self.stroke_width.filter(|_| fields.stroke_width),
            font_size: self.font_size.filter(|_| fields.font_size),
            blur: self.blur.filter(|_| fields.blur),
        }
    }
}

/// A set of [`Style`] fields: those a kind of annotation draws with
/// ([`Shape::style_fields`](super::Shape::style_fields)), for example.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct StyleFields {
    pub color: bool,
    pub stroke_width: bool,
    pub font_size: bool,
    pub blur: bool,
}

impl StyleFields {
    /// No fields.
    pub const NONE: Self = Self {
        color: false,
        stroke_width: false,
        font_size: false,
        blur: false,
    };

    /// Every field.
    pub const ALL: Self = Self {
        color: true,
        stroke_width: true,
        font_size: true,
        blur: true,
    };

    /// What strokes draw with: color and stroke width.
    pub const STROKE: Self = Self {
        color: true,
        stroke_width: true,
        ..Self::NONE
    };

    /// What text and step markers draw with: color and font size.
    pub const TEXT: Self = Self {
        color: true,
        font_size: true,
        ..Self::NONE
    };

    /// What blur regions draw with: the blur mode.
    pub const BLUR: Self = Self {
        blur: true,
        ..Self::NONE
    };
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

    #[test]
    fn only_keeps_the_fields_asked_for() {
        let patch = StylePatch {
            color: Some(Rgba8::rgb(1, 2, 3)),
            stroke_width: Some(9.0),
            font_size: Some(30.0),
            blur: Some(BlurMode::Gaussian),
        };
        assert_eq!(
            patch.only(StyleFields::STROKE),
            StylePatch {
                color: patch.color,
                stroke_width: patch.stroke_width,
                ..StylePatch::default()
            }
        );
        assert_eq!(patch.only(StyleFields::ALL), patch);
        assert_eq!(patch.only(StyleFields::NONE), StylePatch::default());
    }
}
