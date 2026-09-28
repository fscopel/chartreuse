//! Rendering a document into pixels for export, matching the canvas. Owned by
//! track 2F.
//!
//! [`flatten`] draws a [`Document`]'s annotations over a copy of its base
//! image, bottom to top, at the base image's resolution: one document unit is
//! one output pixel, so the result is what the [canvas](crate::canvas#drawing)
//! shows at a scale of 1, clipped to the image. Last, it cuts the result down
//! to the document's [crop](Document::crop), if it has one.
//!
//! # Rasterizer
//!
//! Shapes are rasterized with [tiny-skia]: anti-aliased, analytic coverage,
//! with real round caps and joins. It is the library iced's software renderer
//! draws canvas geometry with, so the tiny-skia canvas and flatten rasterize
//! the same paths with the same code and agree to within rounding, and iced
//! already builds it, so it adds nothing to the build. Text glyphs come from
//! cosmic-text's [`SwashCache`], the glyph rasterizer behind both of iced's
//! renderers.
//!
//! # Drawing
//!
//! Per annotation, in its [`Style`]'s color (straight alpha), exactly as the
//! canvas docs describe:
//!
//! - Strokes (a line, an arrow's shaft, a rectangle's or ellipse's outline, a
//!   pen's path) are `stroke_width` wide, centered on the geometry, with
//!   round caps and round joins. A zero-length stroke is a disc
//!   `stroke_width` across; a stroke width of zero (or less) draws nothing.
//! - An arrow is its shaft stroked from `start` to [`ArrowHead::base`], then
//!   the head triangle `[tip, left, right]` filled, never stroked.
//! - A rectangle is the closed outline through [`Rect::corners`].
//! - An ellipse is the closed path of the Béziers of [`Ellipse::curves`];
//!   one of zero size is a dot.
//! - A pen stroke is the open path through its points; one whose points all
//!   coincide is a dot.
//! - A highlighter stroke is the same path, [`highlighter::width`] wide,
//!   drawn opaque into a layer of its own (`highlighter_layer`) that is
//!   composited at [`highlighter::alpha`], so it never darkens where it
//!   overlaps itself (see [`highlighter`]).
//! - A step marker is a disc [`StepMarker::radius`] in radius filled in its
//!   color, then its number ([`Document::step_number`]) drawn as text would
//!   be, in [`StepMarker::number_color`], at the layout box position
//!   [`StepMarker::label_origin`] gives for the number's [`font::measure`].
//! - A blur region composites everything drawn so far onto the image
//!   (`Flattener::flush`), then runs [`chartreuse_imaging::pixelate`] (with
//!   [`BlurRegion::PIXELATE_BLOCK`]) or [`chartreuse_imaging::blur`] (with
//!   [`BlurRegion::BLUR_RADIUS`]), as its style's [`BlurMode`] says, over
//!   [`BlurRegion::pixels`] of the result, so it obscures exactly what lies
//!   beneath it. The canvas shows it with `obscured`, which computes the same
//!   pixels.
//! - Text is laid out by [`font::layout`] and each glyph rasterized by swash,
//!   placed as iced places canvas text: the glyph's pixel origin is
//!   [`LayoutGlyph::physical`] with the text's position as the offset, moved
//!   down to the line's baseline (`LayoutRun::line_y`, rounded) and by the
//!   glyph image's placement. Mask glyphs are coverage × the color; color
//!   glyphs (emoji from a system fallback font) keep their own colors, with
//!   the style's alpha as their opacity, as in iced.
//!
//! # Compositing
//!
//! Annotations are drawn with source-over blending into a transparent,
//! premultiplied layer the size of the image, which is then composited
//! source-over onto the straight-alpha base image in floating point. Pixels no
//! annotation touches keep their exact bytes, and translucent base pixels
//! (from a pasted or opened image) are blended correctly rather than
//! round-tripped through premultiplied 8-bit color.
//!
//! # Adding a kind
//!
//! A new [`Shape`] variant needs one arm in the private `Flattener::draw`,
//! built from its helpers. Kinds that act on what is beneath them, as blur
//! regions do, `flush` first and then work on `Flattener::image`. A
//! `Flattener` covers a window of the document (all of it, for [`flatten`]),
//! so draw through `Flattener::transform`.
//!
//! [`Style`]: crate::model::Style
//! [tiny-skia]: https://docs.rs/tiny-skia/0.11
//! [`SwashCache`]: iced::advanced::graphics::text::cosmic_text::SwashCache
//! [`LayoutGlyph::physical`]: iced::advanced::graphics::text::cosmic_text::LayoutGlyph::physical
//! [`ArrowHead::base`]: crate::model::ArrowHead::base
//! [`Rect::corners`]: crate::model::Rect::corners
//! [`Ellipse::curves`]: crate::model::Ellipse::curves
//! [`font::layout`]: crate::font::layout
//! [`font::measure`]: crate::font::measure
//! [`StepMarker::radius`]: crate::model::StepMarker::radius
//! [`StepMarker::number_color`]: crate::model::StepMarker::number_color
//! [`StepMarker::label_origin`]: crate::model::StepMarker::label_origin
//! [`Document::step_number`]: crate::model::Document::step_number

mod text;

use chartreuse_core::color::Rgba8;
use chartreuse_core::error::{Error, Result};
use chartreuse_core::geometry::{PhysicalPoint, PhysicalRect};
use chartreuse_core::image::Image;
use chartreuse_imaging::region::clip;
use tiny_skia::{
    FillRule, FilterQuality, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapPaint,
    Stroke, Transform,
};

use crate::font;
use crate::model::{
    highlighter, Annotation, BlurMode, BlurRegion, Document, Point, Polyline, Rect, Shape,
    StepMarker, Style, Text,
};

/// The document's base image with every annotation drawn over it, bottom to
/// top, at the base image's size, then cut down to the document's
/// [crop](Document::crop) (see the [module docs](self)). The document is
/// unchanged.
///
/// Draws text with iced's shared font system (after [`font::load`]), so it
/// uses the same fonts, including fallbacks, as the canvas; it holds that
/// system's lock while it draws each text annotation.
///
/// # Errors
///
/// [`Error::InvalidImage`] if the image is too large for the rasterizer
/// (2²⁹ pixels wide or more).
///
/// [`font::load`]: crate::font::load
pub fn flatten(document: &Document) -> Result<Image> {
    let base = document.base();
    let annotations = document.annotations();
    let image = if annotations.is_empty() || base.width() == 0 || base.height() == 0 {
        base.clone()
    } else {
        let mut flattener = Flattener::new(base.clone(), PhysicalPoint::new(0, 0))?;
        for annotation in annotations {
            flattener.draw(&Drawn::of(document, annotation));
        }
        flattener.flush();
        flattener.image
    };
    match document.crop_pixels() {
        Some(crop) => chartreuse_imaging::crop(&image, crop),
        None => Ok(image),
    }
}

/// An annotation as flatten draws it: its shape and style, and a step
/// marker's number.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drawn<'a> {
    pub(crate) shape: &'a Shape,
    pub(crate) style: &'a Style,
    pub(crate) number: Option<usize>,
}

impl<'a> Drawn<'a> {
    /// `annotation` as it is in `document`.
    pub(crate) fn of(document: &Document, annotation: &'a Annotation) -> Self {
        Self {
            shape: &annotation.shape,
            style: &annotation.style,
            number: document.step_number(annotation.id()),
        }
    }

    /// A rectangle outside which the annotation draws nothing: its bounds,
    /// grown for anti-aliasing and for glyph ink overhanging text's layout
    /// box.
    fn reach(&self) -> Rect {
        let slack = match self.shape {
            Shape::Text(_) | Shape::Step(_) => self.style.font_size.max(0.0),
            _ => 2.0,
        };
        self.shape.bounds(self.style).expand(slack)
    }
}

/// What blur region `region`, drawn in `mode` over `below` (the annotations
/// beneath it, bottom to top, on `base`), leaves in the pixels it acts on:
/// the pixels [`flatten`] gives them, before anything above the region is
/// drawn. Returns those pixels' rectangle within the image, and their
/// contents; `None` if the region covers none of the image.
///
/// Only the part of the document the result depends on is flattened: the
/// region, plus any blur regions below that overlap it (whose effect reads
/// pixels around it), and so on. Rasterizing shapes into that smaller window
/// can shade an anti-aliased pixel one unit differently than rasterizing
/// them into the whole image does, so the result matches flatten's to within
/// one per channel. The canvas shows blur regions with this.
pub(crate) fn obscured(
    base: &Image,
    below: &[Drawn<'_>],
    region: &BlurRegion,
    mode: BlurMode,
) -> Option<(PhysicalRect, Image)> {
    let target = clip(base, region.pixels()?)?;
    let window = window(base, below, target);
    let pixels = chartreuse_imaging::crop(base, window).ok()?;
    let mut flattener = Flattener::new(pixels, window.origin).ok()?;
    for drawn in below {
        flattener.draw(drawn);
    }
    flattener.obscure(region, mode);
    let within = PhysicalRect {
        origin: PhysicalPoint::new(
            target.origin.x - window.origin.x,
            target.origin.y - window.origin.y,
        ),
        size: target.size,
    };
    let image = chartreuse_imaging::crop(&flattener.image, within).ok()?;
    Some((target, image))
}

/// The annotations among `below` that can change what [`obscured`] gives
/// `region`: those that draw somewhere in the pixels it depends on. Leaving
/// the others out gives the same result.
pub(crate) fn beneath<'a>(
    base: &Image,
    below: &[Drawn<'a>],
    region: &BlurRegion,
) -> Vec<Drawn<'a>> {
    let Some(target) = region.pixels().and_then(|pixels| clip(base, pixels)) else {
        return Vec::new();
    };
    let window = window(base, below, target);
    let window = Rect::from_pixels(window);
    below
        .iter()
        .filter(|drawn| drawn.reach().intersects(&window))
        .copied()
        .collect()
}

/// The part of the image that what a blur region leaves in `target` (its
/// pixels within the image) depends on: `target`, grown to take in every
/// blur region in `below` that overlaps it, until none that overlaps is left
/// out (each reads all of its own pixels).
fn window(base: &Image, below: &[Drawn<'_>], target: PhysicalRect) -> PhysicalRect {
    let regions: Vec<_> = below
        .iter()
        .filter_map(|drawn| match drawn.shape {
            Shape::Blur(region) => clip(base, region.pixels()?),
            _ => None,
        })
        .collect();
    let mut window = target;
    loop {
        let grown = regions
            .iter()
            .filter(|region| region.intersection(&window).is_some())
            .fold(window, |window, region| union(window, *region));
        if grown == window {
            return window;
        }
        window = grown;
    }
}

/// The smallest rectangle containing both (which lie within one image).
fn union(a: PhysicalRect, b: PhysicalRect) -> PhysicalRect {
    let (x0, y0) = (a.min_x().min(b.min_x()), a.min_y().min(b.min_y()));
    let (x1, y1) = (a.max_x().max(b.max_x()), a.max_y().max(b.max_y()));
    // Within the image, so the casts are exact.
    PhysicalRect::new(
        x0,
        y0,
        (x1 - i64::from(x0)) as u32,
        (y1 - i64::from(y0)) as u32,
    )
}

/// A window of a document being flattened: its pixels so far, and the
/// annotations drawn since the last [`flush`](Self::flush), not yet
/// composited onto them.
struct Flattener {
    /// Straight alpha.
    image: Image,
    /// Where `image`'s top-left pixel is in the document.
    origin: PhysicalPoint,
    /// Premultiplied, the image's size.
    layer: Pixmap,
    text: text::Rasterizer,
}

impl Flattener {
    /// Flattens onto `image`, the document's pixels from `origin` on.
    fn new(image: Image, origin: PhysicalPoint) -> Result<Self> {
        let layer = Pixmap::new(image.width(), image.height()).ok_or_else(|| {
            Error::InvalidImage(format!(
                "{}×{} is too large to flatten",
                image.width(),
                image.height()
            ))
        })?;
        Ok(Self {
            image,
            origin,
            layer,
            text: text::Rasterizer::new(),
        })
    }

    /// Maps document coordinates onto the window's pixels.
    fn transform(&self) -> Transform {
        Transform::from_translate(-self.origin.x as f32, -self.origin.y as f32)
    }

    /// Draws an annotation above everything drawn so far.
    fn draw(&mut self, drawn: &Drawn<'_>) {
        let style = drawn.style;
        let paint = paint(style.color);
        let width = style.stroke_width.max(0.0);
        let transform = self.transform();
        let layer = &mut self.layer;
        match drawn.shape {
            Shape::Line(line) => polyline(layer, &[line.start, line.end], width, &paint, transform),
            Shape::Arrow(arrow) => match arrow.head(style.stroke_width) {
                Some(head) => {
                    polyline(layer, &[arrow.start, head.base], width, &paint, transform);
                    let [tip, left, right] = head.corners();
                    let mut path = PathBuilder::new();
                    path.move_to(tip.x, tip.y);
                    path.line_to(left.x, left.y);
                    path.line_to(right.x, right.y);
                    path.close();
                    fill(layer, path.finish(), &paint, transform);
                }
                None => dot(layer, arrow.start, width, &paint, transform),
            },
            Shape::Rectangle(rectangle) => {
                let [a, b, c, d] = rectangle.rect.corners();
                if a == c {
                    dot(layer, a, width, &paint, transform);
                } else {
                    let mut path = PathBuilder::new();
                    path.move_to(a.x, a.y);
                    for corner in [b, c, d] {
                        path.line_to(corner.x, corner.y);
                    }
                    path.close();
                    stroke(layer, path.finish(), width, &paint, transform);
                }
            }
            Shape::Ellipse(ellipse) => {
                let (start, curves) = ellipse.curves();
                if ellipse.rect.width() == 0.0 && ellipse.rect.height() == 0.0 {
                    dot(layer, start, width, &paint, transform);
                } else {
                    let mut path = PathBuilder::new();
                    path.move_to(start.x, start.y);
                    for [a, b, to] in curves {
                        path.cubic_to(a.x, a.y, b.x, b.y, to.x, to.y);
                    }
                    path.close();
                    stroke(layer, path.finish(), width, &paint, transform);
                }
            }
            Shape::Pen(pen) => polyline(layer, &pen.points, width, &paint, transform),
            Shape::Highlighter(stroke) => self.highlighter(stroke, style),
            Shape::Step(step) => {
                let diameter = 2.0 * StepMarker::radius(style.font_size);
                dot(layer, step.center, diameter, &paint, transform);
                if let Some(number) = drawn.number {
                    let label = number.to_string();
                    let size = font::measure(&label, style.font_size);
                    let text = Text::new(step.label_origin(size), label);
                    let style = Style {
                        color: StepMarker::number_color(style.color),
                        ..*style
                    };
                    self.text.draw(layer, self.origin, &text, &style);
                }
            }
            Shape::Blur(region) => self.obscure(region, style.blur),
            Shape::Text(text) => self.text.draw(layer, self.origin, text, style),
        }
    }

    /// A blur region: composites what is drawn so far, then obscures the
    /// region's pixels of the result.
    fn obscure(&mut self, region: &BlurRegion, mode: BlurMode) {
        let Some(pixels) = region.pixels() else {
            return;
        };
        self.flush();
        // Both well within i32 (see `BlurRegion::pixels`), so this cannot
        // overflow.
        let within = PhysicalRect {
            origin: PhysicalPoint::new(
                pixels.origin.x - self.origin.x,
                pixels.origin.y - self.origin.y,
            ),
            size: pixels.size,
        };
        match mode {
            BlurMode::Pixelate => {
                chartreuse_imaging::pixelate(&mut self.image, within, BlurRegion::PIXELATE_BLOCK);
            }
            BlurMode::Gaussian => {
                chartreuse_imaging::blur(&mut self.image, within, BlurRegion::BLUR_RADIUS);
            }
        }
    }

    /// A highlighter stroke: its own layer (see [`highlighter_layer`]),
    /// covering the whole pixels the stroke touches, composited onto the
    /// annotation layer at [`highlighter::alpha`].
    fn highlighter(&mut self, stroke: &Polyline, style: &Style) {
        let reach = highlighter::width(style) / 2.0;
        let bounds = stroke.path_bounds().expand(reach);
        let (ox, oy) = (self.origin.x as f32, self.origin.y as f32);
        let clamp = |value: f32, size: u32| {
            // Clamped to the window first, so the casts are exact.
            value.clamp(0.0, size as f32) as i32
        };
        let (x0, y0) = (
            clamp(bounds.min().x.floor() - ox, self.image.width()),
            clamp(bounds.min().y.floor() - oy, self.image.height()),
        );
        let (x1, y1) = (
            clamp(bounds.max().x.ceil() - ox, self.image.width()),
            clamp(bounds.max().y.ceil() - oy, self.image.height()),
        );
        let (Ok(width), Ok(height)) = (u32::try_from(x1 - x0), u32::try_from(y1 - y0)) else {
            return;
        };
        let transform = Transform::from_translate(-x0 as f32 - ox, -y0 as f32 - oy);
        if let Some(layer) = highlighter_layer(stroke, style, transform, width, height) {
            let paint = PixmapPaint {
                opacity: highlighter::alpha(style),
                quality: FilterQuality::Nearest,
                ..PixmapPaint::default()
            };
            self.layer
                .draw_pixmap(x0, y0, layer.as_ref(), &paint, Transform::identity(), None);
        }
    }

    /// Composites the layer onto the image and clears it.
    fn flush(&mut self) {
        let layer = self.layer.data_mut();
        for (dst, src) in self
            .image
            .pixels_mut()
            .chunks_exact_mut(4)
            .zip(layer.chunks_exact_mut(4))
        {
            if src[3] != 0 {
                source_over(dst, src);
                src.fill(0);
            }
        }
    }
}

/// A highlighter stroke's own layer (see [`highlighter`]): the stroke drawn
/// at full opacity, in its color with the alpha left out, into a transparent
/// premultiplied pixmap `width` × `height` pixels big, with `transform`
/// mapping document coordinates onto the pixmap. Compositing it at
/// [`highlighter::alpha`] finishes the job. The canvas draws highlighters
/// with this too, so they look the same there.
///
/// `None` if the pixmap would be empty.
pub(crate) fn highlighter_layer(
    stroke: &Polyline,
    style: &Style,
    transform: Transform,
    width: u32,
    height: u32,
) -> Option<Pixmap> {
    let mut layer = Pixmap::new(width, height)?;
    let opaque = Rgba8 {
        a: u8::MAX,
        ..style.color
    };
    polyline(
        &mut layer,
        &stroke.points,
        highlighter::width(style),
        &paint(opaque),
        transform,
    );
    Some(layer)
}

/// A stroke through `points`, or a disc if they all coincide.
fn polyline(
    target: &mut Pixmap,
    points: &[Point],
    width: f32,
    paint: &Paint<'_>,
    transform: Transform,
) {
    let [first, rest @ ..] = points else {
        return;
    };
    if rest.iter().all(|p| p == first) {
        dot(target, *first, width, paint, transform);
    } else {
        let mut path = PathBuilder::new();
        path.move_to(first.x, first.y);
        for p in rest {
            path.line_to(p.x, p.y);
        }
        stroke(target, path.finish(), width, paint, transform);
    }
}

/// A zero-length stroke: a disc `width` across.
fn dot(target: &mut Pixmap, center: Point, width: f32, paint: &Paint<'_>, transform: Transform) {
    if width > 0.0 {
        fill(
            target,
            PathBuilder::from_circle(center.x, center.y, width / 2.0),
            paint,
            transform,
        );
    }
}

/// Strokes `path` (if it was valid) `width` wide with round caps and joins.
/// A width of zero draws nothing; tiny-skia would draw a hairline.
fn stroke(
    target: &mut Pixmap,
    path: Option<Path>,
    width: f32,
    paint: &Paint<'_>,
    transform: Transform,
) {
    if let Some(path) = path
        && width > 0.0
    {
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        target.stroke_path(&path, paint, &stroke, transform, None);
    }
}

/// Fills `path` (if it was valid) with the nonzero rule, as iced's canvas
/// does by default.
fn fill(target: &mut Pixmap, path: Option<Path>, paint: &Paint<'_>, transform: Transform) {
    if let Some(path) = path {
        target.fill_path(&path, paint, FillRule::Winding, transform, None);
    }
}

/// An anti-aliased solid paint in `color` (straight alpha).
fn paint(color: Rgba8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    paint
}

/// Blends the premultiplied pixel `src` over the straight-alpha pixel `dst`.
fn source_over(dst: &mut [u8], src: &[u8]) {
    let unit = |value: u8| f32::from(value) / 255.0;
    let src_alpha = unit(src[3]);
    let dst_alpha = unit(dst[3]);
    // How much of the destination shows through, premultiplied by its alpha.
    let under = dst_alpha * (1.0 - src_alpha);
    let alpha = src_alpha + under;
    for channel in 0..3 {
        let premultiplied = unit(src[channel]) + unit(dst[channel]) * under;
        dst[channel] = to_byte(premultiplied / alpha);
    }
    dst[3] = to_byte(alpha);
}

/// `value` (0 to 1) as the nearest byte.
fn to_byte(value: f32) -> u8 {
    // In range after the clamp, so the cast is exact.
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests;
