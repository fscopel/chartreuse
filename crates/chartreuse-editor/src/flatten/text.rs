//! Text annotations: laid out by [`font::layout`], rasterized by swash, and
//! placed as iced's renderers place canvas text (see the
//! [module docs](super#drawing)).

use std::sync::PoisonError;

use chartreuse_core::color::Rgba8;
use chartreuse_core::geometry::PhysicalPoint;
use iced::advanced::graphics::text::cosmic_text::{SwashCache, SwashContent, SwashImage};
use iced::advanced::graphics::text::font_system;
use tiny_skia::{ColorU8, Pixmap, PixmapPaint, PixmapRef, Transform};

use crate::font;
use crate::model::{Style, Text};

/// Rasterizes glyphs for one flatten, keeping each glyph image it rasterizes
/// for the text drawn after it.
#[derive(Debug)]
pub(super) struct Rasterizer {
    glyphs: SwashCache,
    /// A glyph image as a premultiplied pixmap, reused between glyphs.
    pixels: Vec<u8>,
}

impl Rasterizer {
    pub(super) fn new() -> Self {
        Self {
            glyphs: SwashCache::new(),
            pixels: Vec::new(),
        }
    }

    /// Draws `text` in `style` into `layer`, source-over, `layer`'s top-left
    /// pixel being document pixel `origin`.
    pub(super) fn draw(
        &mut self,
        layer: &mut Pixmap,
        origin: PhysicalPoint,
        text: &Text,
        style: &Style,
    ) {
        font::load();
        let mut system = font_system()
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let system = system.raw();
        let buffer = font::layout(system, &text.content, style.font_size.max(0.0));
        let paint = PixmapPaint {
            opacity: f32::from(style.color.a) / 255.0,
            ..PixmapPaint::default()
        };
        for run in buffer.layout_runs() {
            // As iced: the baseline is rounded to a whole pixel, and the
            // glyph's own origin is snapped by its subpixel bin.
            let baseline = run.line_y.round() as i32;
            for glyph in run.glyphs {
                let physical = glyph.physical((text.position.x, text.position.y), 1.0);
                let Some(image) = self.glyphs.get_image(system, physical.cache_key) else {
                    continue;
                };
                let Some(pixmap) = premultiply(image, style.color, &mut self.pixels) else {
                    continue;
                };
                layer.draw_pixmap(
                    physical.x + image.placement.left - origin.x,
                    physical.y - image.placement.top + baseline - origin.y,
                    pixmap,
                    &paint,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}

/// `image` as a premultiplied pixmap in `pixels`: a mask glyph's coverage
/// becomes the alpha of `color` (whose own alpha is applied as the draw's
/// opacity), and a color glyph keeps its colors. `None` for an empty glyph
/// (a space) or content swash does not produce for us (subpixel masks).
fn premultiply<'a>(
    image: &SwashImage,
    color: Rgba8,
    pixels: &'a mut Vec<u8>,
) -> Option<PixmapRef<'a>> {
    let placement = image.placement;
    pixels.clear();
    match image.content {
        SwashContent::Mask => {
            for &coverage in &image.data {
                let pixel = ColorU8::from_rgba(color.r, color.g, color.b, coverage).premultiply();
                pixels.extend_from_slice(&[pixel.red(), pixel.green(), pixel.blue(), coverage]);
            }
        }
        SwashContent::Color => {
            for rgba in image.data.chunks_exact(4) {
                let pixel = ColorU8::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]).premultiply();
                pixels.extend_from_slice(&[pixel.red(), pixel.green(), pixel.blue(), rgba[3]]);
            }
        }
        SwashContent::SubpixelMask => return None,
    }
    PixmapRef::from_bytes(pixels, placement.width, placement.height)
}
