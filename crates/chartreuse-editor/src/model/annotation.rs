//! Annotation kinds, their geometry, and hit-testing.
//!
//! # Adding a kind
//!
//! Each kind is a struct plus a [`Shape`] variant. Adding one means a new
//! struct, a new variant, and one arm in each `match` in [`Shape`]'s methods;
//! the compiler lists every other place (canvas, flatten) that must learn to
//! draw it. Nothing in the document or undo machinery is kind-specific
//! except text measurement and step numbering.
//!
//! Step markers do not store their number: it is derived from the document
//! ([`Document::step_number`](super::Document::step_number): the 1-based rank
//! of the marker's [`AnnotationId`] among the step markers still present),
//! so deleting or restoring a marker renumbers the rest for free. Ids
//! increase in creation order and are never reused, and undo restores the
//! original id, so that rank is stable under reordering and undo.

use std::num::NonZeroU32;

use chartreuse_core::color::Rgba8;
use chartreuse_core::geometry::PhysicalRect;

use super::geometry::{
    distance_to_ellipse, distance_to_polyline, distance_to_segment, distance_to_triangle, Point,
    Rect, Size, Vector,
};
use super::style::{Style, StyleFields};

/// A document-unique, stable annotation identifier.
///
/// Ids are handed out by [`Document`](super::Document) in increasing order of
/// creation and are never reused, even after the annotation is deleted or its
/// creation is undone. They are not z-order; use the annotation's index for
/// that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnnotationId(pub(super) u64);

impl AnnotationId {
    /// The raw value, for logging and debugging.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One annotation: what it is ([`Shape`]), how it looks ([`Style`]), and its
/// stable id. Only a [`Document`](super::Document) creates annotations, and it
/// changes them only through commands.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    id: AnnotationId,
    pub shape: Shape,
    pub style: Style,
}

impl Annotation {
    pub(super) const fn new(id: AnnotationId, shape: Shape, style: Style) -> Self {
        Self { id, shape, style }
    }

    #[must_use]
    pub const fn id(&self) -> AnnotationId {
        self.id
    }

    /// True if `point` hits this annotation. See [`Shape::hit`].
    #[must_use]
    pub fn hit(&self, point: Point, tolerance: f32) -> bool {
        self.shape.hit(&self.style, point, tolerance)
    }

    /// The visual bounds. See [`Shape::bounds`].
    #[must_use]
    pub fn bounds(&self) -> Rect {
        self.shape.bounds(&self.style)
    }
}

/// The kind-specific part of an annotation.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Line(Line),
    Arrow(Arrow),
    Rectangle(Rectangle),
    Ellipse(Ellipse),
    /// A freehand pen stroke.
    Pen(Polyline),
    /// A freehand highlighter stroke: wide and translucent; see
    /// [`highlighter`].
    Highlighter(Polyline),
    /// A numbered step marker.
    Step(StepMarker),
    /// A region that pixelates or blurs what is beneath it.
    Blur(BlurRegion),
    Text(Text),
}

impl Shape {
    /// True if `point` hits the shape drawn with `style`.
    ///
    /// `tolerance` (document units, negative treated as zero) is extra slack
    /// around the drawn area so thin strokes are easy to click; the canvas
    /// divides its screen-space slop by the zoom factor. Per kind:
    ///
    /// - Line: within `stroke_width / 2 + tolerance` of the segment (so the hit
    ///   area has round caps, like the stroke; see [`Style`]).
    /// - Arrow: as a line along the shaft (`start` to [`ArrowHead::base`]), or
    ///   within `tolerance` of the filled [`ArrowHead`] triangle.
    /// - Rectangle: within `stroke_width / 2 + tolerance` of the outline (so
    ///   the outer corners are rounded, like the stroke's round joins). The
    ///   interior does not hit, so annotations and image content inside an
    ///   outline stay clickable. (A future filled rectangle would also hit
    ///   inside.)
    /// - Ellipse: within `stroke_width / 2 + tolerance` of the outline
    ///   ([`distance_to_ellipse`]); like a rectangle, not inside.
    /// - Pen: within `stroke_width / 2 + tolerance` of the path.
    /// - Highlighter: within [`highlighter::width`]` / 2 + tolerance` of the
    ///   path.
    /// - Step marker: within `tolerance` of its disc ([`StepMarker::radius`]).
    /// - Blur region: inside its rectangle grown by `tolerance`; it covers
    ///   what is beneath it, so all of it hits.
    /// - Text: inside [`Text::bounds`] grown by `tolerance`.
    #[must_use]
    pub fn hit(&self, style: &Style, point: Point, tolerance: f32) -> bool {
        let tolerance = tolerance.max(0.0);
        let reach = half_stroke(style) + tolerance;
        match self {
            Self::Line(line) => distance_to_segment(point, line.start, line.end) <= reach,
            Self::Arrow(arrow) => match arrow.head(style.stroke_width) {
                Some(head) => {
                    distance_to_segment(point, arrow.start, head.base) <= reach
                        || distance_to_triangle(point, head.corners()) <= tolerance
                }
                None => distance_to_segment(point, arrow.start, arrow.end) <= reach,
            },
            Self::Rectangle(rectangle) => rectangle.rect.distance_to_outline(point) <= reach,
            Self::Ellipse(ellipse) => distance_to_ellipse(point, ellipse.rect) <= reach,
            Self::Pen(pen) => distance_to_polyline(point, &pen.points) <= reach,
            Self::Highlighter(stroke) => {
                distance_to_polyline(point, &stroke.points)
                    <= highlighter::width(style) / 2.0 + tolerance
            }
            Self::Step(step) => {
                point.distance(step.center) <= StepMarker::radius(style.font_size) + tolerance
            }
            Self::Blur(region) => region.rect.expand(tolerance).contains(point),
            Self::Text(text) => text
                .bounds(style.font_size)
                .expand(tolerance)
                .contains(point),
        }
    }

    /// The smallest rectangle containing everything the shape draws with
    /// `style`: strokes reach `stroke_width / 2` past their path in every
    /// direction (round caps and joins; see [`Style`]), arrows include their
    /// head, text its layout box. Used for selection outlines and marquee
    /// selection.
    #[must_use]
    pub fn bounds(&self, style: &Style) -> Rect {
        let half = half_stroke(style);
        match self {
            Self::Line(line) => Rect::from_corners(line.start, line.end).expand(half),
            Self::Arrow(arrow) => match arrow.head(style.stroke_width) {
                Some(head) => Rect::from_corners(arrow.start, head.base)
                    .expand(half)
                    .union(&Rect::from_corners(head.left, head.right))
                    .union(&Rect::from_corners(head.tip, head.tip)),
                None => Rect::from_corners(arrow.start, arrow.end).expand(half),
            },
            Self::Rectangle(rectangle) => rectangle.rect.expand(half),
            Self::Ellipse(ellipse) => ellipse.rect.expand(half),
            Self::Pen(pen) => pen.path_bounds().expand(half),
            Self::Highlighter(stroke) => {
                stroke.path_bounds().expand(highlighter::width(style) / 2.0)
            }
            Self::Step(step) => {
                let center = Rect::from_corners(step.center, step.center);
                center.expand(StepMarker::radius(style.font_size))
            }
            Self::Blur(region) => region.rect,
            Self::Text(text) => text.bounds(style.font_size),
        }
    }

    /// The style fields the shape draws with, the only ones restyling
    /// changes ([`Command::Restyle`](super::Command::Restyle)): color and
    /// stroke width for strokes (a line, arrow, rectangle, ellipse, pen, or
    /// highlighter), color and font size for text and step markers, and only
    /// the blur mode for a blur region.
    #[must_use]
    pub const fn style_fields(&self) -> StyleFields {
        match self {
            Self::Line(_)
            | Self::Arrow(_)
            | Self::Rectangle(_)
            | Self::Ellipse(_)
            | Self::Pen(_)
            | Self::Highlighter(_) => StyleFields::STROKE,
            Self::Step(_) | Self::Text(_) => StyleFields::TEXT,
            Self::Blur(_) => StyleFields::BLUR,
        }
    }

    /// Moves the shape by `delta`.
    pub fn translate(&mut self, delta: Vector) {
        match self {
            Self::Line(line) => {
                line.start += delta;
                line.end += delta;
            }
            Self::Arrow(arrow) => {
                arrow.start += delta;
                arrow.end += delta;
            }
            Self::Rectangle(rectangle) => rectangle.rect = rectangle.rect.translate(delta),
            Self::Ellipse(ellipse) => ellipse.rect = ellipse.rect.translate(delta),
            Self::Pen(stroke) | Self::Highlighter(stroke) => stroke.translate(delta),
            Self::Step(step) => step.center += delta,
            Self::Blur(region) => region.rect = region.rect.translate(delta),
            Self::Text(text) => text.position += delta,
        }
    }
}

fn half_stroke(style: &Style) -> f32 {
    style.stroke_width.max(0.0) / 2.0
}

pub mod highlighter {
    //! How a [`Shape::Highlighter`](super::Shape::Highlighter) draws its
    //! [`Polyline`](super::Polyline): like a pen stroke (round caps and
    //! joins), but [`WIDTH_PER_STROKE`] times as wide as the style's
    //! `stroke_width`, and translucent.
    //!
    //! # Translucency
    //!
    //! The stroke is drawn as a whole at full opacity in the style's color
    //! into a layer of its own, and that layer is composited over what lies
    //! beneath it at [`alpha`]: the color's own alpha times [`OPACITY`]. So a
    //! stroke that crosses itself, or whose joins overlap, is one even tint,
    //! never darker where it overlaps; two separate highlighter strokes do
    //! darken where they cross, like real highlighter ink. The canvas and
    //! flatten both render it this way, with the same rasterizer, so they
    //! agree.

    use super::Style;

    /// The stroke's width per unit of the style's `stroke_width`.
    pub const WIDTH_PER_STROKE: f32 = 4.0;

    /// The opacity the stroke's layer is composited at, times the color's
    /// own alpha.
    pub const OPACITY: f32 = 0.4;

    /// The stroke's width in document units (never negative).
    #[must_use]
    pub fn width(style: &Style) -> f32 {
        style.stroke_width.max(0.0) * WIDTH_PER_STROKE
    }

    /// The opacity (0 to 1) the stroke's layer is composited at.
    #[must_use]
    pub fn alpha(style: &Style) -> f32 {
        f32::from(style.color.a) / 255.0 * OPACITY
    }
}

/// A straight line segment, stroked with round caps (see [`Style`]). A
/// zero-length line draws a dot `stroke_width` across.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub start: Point,
    pub end: Point,
}

/// A line with an arrowhead at `end`.
///
/// The head's geometry comes from [`Arrow::head`] so the canvas, flatten, and
/// hit-testing agree on it. Renderers stroke the shaft from `start` to
/// [`ArrowHead::base`] like any stroke (round caps; see [`Style`]), then fill
/// the head triangle. The head covers the shaft's cap at the base, and ending
/// the shaft there keeps a thick stroke from poking out past the head's point.
/// An arrow shorter than its head's full length is all head (`base` is
/// `start`, so the shaft is a dot there); a zero-length arrow has no head and
/// draws a dot, like a zero-length line.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrow {
    pub start: Point,
    /// The tip.
    pub end: Point,
}

impl Arrow {
    /// Head length per unit of stroke width.
    pub const HEAD_LENGTH_PER_STROKE: f32 = 3.0;
    /// The shortest head, so thin arrows still read as arrows.
    pub const MIN_HEAD_LENGTH: f32 = 10.0;
    /// Head half-width (tip to either back corner, across) per unit of head
    /// length.
    pub const HEAD_HALF_WIDTH_RATIO: f32 = 0.5;

    /// The arrowhead for a given stroke width: length
    /// `max(stroke_width × HEAD_LENGTH_PER_STROKE, MIN_HEAD_LENGTH)`, capped at the
    /// arrow's length, and half as wide on each side as it is long. `None` for a
    /// zero-length arrow, which has no direction.
    #[must_use]
    pub fn head(&self, stroke_width: f32) -> Option<ArrowHead> {
        let along = self.end - self.start;
        let length = along.length();
        if length == 0.0 || !length.is_finite() {
            return None;
        }
        let head_length = (stroke_width.max(0.0) * Self::HEAD_LENGTH_PER_STROKE)
            .max(Self::MIN_HEAD_LENGTH)
            .min(length);
        let direction = along * (1.0 / length);
        let base = self.end - direction * head_length;
        let across = direction.perpendicular() * (head_length * Self::HEAD_HALF_WIDTH_RATIO);
        Some(ArrowHead {
            tip: self.end,
            base,
            left: base - across,
            right: base + across,
        })
    }
}

/// The filled triangle at an arrow's tip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowHead {
    pub tip: Point,
    /// The midpoint of the back edge, where the shaft ends.
    pub base: Point,
    /// The back corners, on either side of `base`.
    pub left: Point,
    pub right: Point,
}

impl ArrowHead {
    /// `[tip, left, right]`.
    #[must_use]
    pub const fn corners(&self) -> [Point; 3] {
        [self.tip, self.left, self.right]
    }
}

/// An unfilled rectangle outline, stroke centered on `rect`'s edges with round
/// joins, so its outer corners are rounded and its inner ones sharp (see
/// [`Style`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Rectangle {
    pub rect: Rect,
}

/// An unfilled ellipse outline: the ellipse inscribed in `rect` (axis-aligned,
/// touching the middle of each side), stroked like any stroke (see
/// [`Style`]). One of zero width or height is the segment it collapses to,
/// with round ends, and one of zero size is a dot.
///
/// Renderers draw the outline as the four cubic Béziers of
/// [`Ellipse::curves`], so the canvas and flatten stroke the same path. They
/// stray from the true ellipse by under 0.03% of the radius, far below a
/// pixel for any screenshot; hit-testing uses the true ellipse.
#[derive(Debug, Clone, PartialEq)]
pub struct Ellipse {
    pub rect: Rect,
}

impl Ellipse {
    /// How far along the tangent a quarter-circle Bézier's control points sit,
    /// per unit of radius: `4/3 × (√2 − 1)`.
    const KAPPA: f32 = 0.552_284_8;

    /// The outline as a closed path: a start point (the rightmost point) and
    /// four cubic Béziers `[control, control, end]`, clockwise on screen, each
    /// a quarter of the ellipse.
    #[must_use]
    pub fn curves(&self) -> (Point, [[Point; 3]; 4]) {
        let c = self.rect.center();
        let (rx, ry) = (self.rect.width() / 2.0, self.rect.height() / 2.0);
        let (kx, ky) = (rx * Self::KAPPA, ry * Self::KAPPA);
        let p = |x: f32, y: f32| Point::new(c.x + x, c.y + y);
        (
            p(rx, 0.0),
            [
                [p(rx, ky), p(kx, ry), p(0.0, ry)],
                [p(-kx, ry), p(-rx, ky), p(-rx, 0.0)],
                [p(-rx, -ky), p(-kx, -ry), p(0.0, -ry)],
                [p(kx, -ry), p(rx, -ky), p(rx, 0.0)],
            ],
        )
    }
}

/// A freehand path: straight segments through `points` in order, stroked
/// like any stroke (round caps and joins; see [`Style`]). A single point is
/// a dot. The freehand tools smooth and simplify the pointer's path before
/// storing it, so the points are the drawn geometry exactly.
///
/// Always has at least one point when made by a tool; one with none draws
/// and hits nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub points: Vec<Point>,
}

impl Polyline {
    /// The smallest rectangle containing every point (a zero-size one at the
    /// origin if there are none).
    #[must_use]
    pub fn path_bounds(&self) -> Rect {
        let mut points = self.points.iter();
        let Some(&first) = points.next() else {
            return Rect::default();
        };
        points.fold(Rect::from_corners(first, first), |bounds, &p| {
            bounds.union(&Rect::from_corners(p, p))
        })
    }

    fn translate(&mut self, delta: Vector) {
        for point in &mut self.points {
            *point += delta;
        }
    }
}

/// A numbered step marker: a disc filled in the style's color, centered on
/// `center`, [`StepMarker::radius`] in radius (sized by the style's
/// `font_size`; `stroke_width` does not apply), with its number on it in
/// [`StepMarker::number_color`].
///
/// The number is not stored: it is the marker's rank among the document's
/// step markers (see the [module docs](self) and
/// [`Document::step_number`](super::Document::step_number)). Renderers lay
/// the number out as annotation text at the style's `font_size` and center
/// its layout box on `center` ([`StepMarker::label_origin`]).
#[derive(Debug, Clone, PartialEq)]
pub struct StepMarker {
    pub center: Point,
}

impl StepMarker {
    /// The disc's radius per unit of font size: room for two digits.
    pub const RADIUS_PER_FONT_SIZE: f32 = 0.8;

    /// The disc's radius for a font size (never negative).
    #[must_use]
    pub fn radius(font_size: f32) -> f32 {
        font_size.max(0.0) * Self::RADIUS_PER_FONT_SIZE
    }

    /// The top-left corner of the number's layout box, given the box's size:
    /// the box centered on the marker.
    #[must_use]
    pub fn label_origin(&self, label: Size) -> Point {
        self.center - Vector::new(label.width / 2.0, label.height / 2.0)
    }

    /// The number's color on a disc of `color`: black on light colors, white
    /// on dark ones (by sRGB luma), with the disc's alpha.
    #[must_use]
    pub fn number_color(color: Rgba8) -> Rgba8 {
        let luma =
            0.2126 * f32::from(color.r) + 0.7152 * f32::from(color.g) + 0.0722 * f32::from(color.b);
        let level = if luma > 0.6 * 255.0 { 0 } else { u8::MAX };
        Rgba8::new(level, level, level, color.a)
    }
}

/// A region that obscures whatever lies beneath it in z-order (the base
/// image and the annotations below it, not those above), as the style's
/// [`BlurMode`](super::BlurMode) says: pixelated into
/// [`PIXELATE_BLOCK`](Self::PIXELATE_BLOCK)-pixel squares, or blurred with a
/// radius of [`BLUR_RADIUS`](Self::BLUR_RADIUS). Color, stroke width, and font
/// size do not apply.
///
/// It acts on the whole pixels of [`BlurRegion::pixels`] and reads only
/// those, so nothing outside it leaks in. The mosaic's grid starts at that
/// rectangle's top-left corner, and the effect is in image pixels, so it looks
/// the same at every zoom and in the export.
#[derive(Debug, Clone, PartialEq)]
pub struct BlurRegion {
    pub rect: Rect,
}

impl BlurRegion {
    /// The side of a pixelated region's squares, in image pixels.
    pub const PIXELATE_BLOCK: NonZeroU32 = NonZeroU32::new(12).unwrap();

    /// A blurred region's blur radius (about its standard deviation), in
    /// image pixels.
    pub const BLUR_RADIUS: u32 = 8;

    /// The pixels the region acts on: its rectangle with each edge rounded to
    /// the nearest pixel boundary ([`Rect::pixels`]; it may reach past the
    /// image, which clips it). `None` if that leaves no pixels.
    #[must_use]
    pub fn pixels(&self) -> Option<PhysicalRect> {
        self.rect.pixels()
    }
}

/// A block of text, one or more lines separated by `\n`.
///
/// # Size
///
/// Laying out text needs fonts, which the pure model does not have. The canvas
/// (2E) measures the text it draws and reports it with
/// [`Document::set_text_size`](super::Document::set_text_size); until then (and
/// again after anything that changes the layout: the content or the font size)
/// the size is estimated from the font size. Renderers use a line height of
/// `font_size × LINE_HEIGHT` so measurements and estimates agree on height.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    /// The top-left corner of the layout box.
    pub position: Point,
    pub content: String,
    /// The measured layout size for the current content and font size, if the
    /// canvas has reported one. Never stale: cleared whenever the layout could
    /// change.
    measured: Option<Size>,
}

impl Text {
    /// Line height per unit of font size, for rendering and estimates.
    pub const LINE_HEIGHT: f32 = 1.2;
    /// Estimated average glyph advance per unit of font size.
    pub const ESTIMATED_ADVANCE: f32 = 0.6;

    #[must_use]
    pub fn new(position: Point, content: impl Into<String>) -> Self {
        Self {
            position,
            content: content.into(),
            measured: None,
        }
    }

    /// The measured size, if the canvas has reported one for the current
    /// content and font size.
    #[must_use]
    pub const fn measured(&self) -> Option<Size> {
        self.measured
    }

    pub(super) fn set_measured(&mut self, size: Option<Size>) {
        self.measured = size;
    }

    /// The layout size: measured if known, otherwise [`Text::estimate_size`].
    #[must_use]
    pub fn size(&self, font_size: f32) -> Size {
        self.measured
            .unwrap_or_else(|| Self::estimate_size(&self.content, font_size))
    }

    /// The layout box: `position` and [`Text::size`].
    #[must_use]
    pub fn bounds(&self, font_size: f32) -> Rect {
        Rect::new(self.position, self.size(font_size))
    }

    /// A font-free size estimate: the longest line's character count ×
    /// `ESTIMATED_ADVANCE × font_size` wide, and the line count (at least one,
    /// so empty text still has a caret-high box) × `LINE_HEIGHT × font_size`
    /// tall.
    #[must_use]
    pub fn estimate_size(content: &str, font_size: f32) -> Size {
        let font_size = font_size.max(0.0);
        let (lines, longest) = content
            .split('\n')
            .fold((0_u16, 0_usize), |(lines, longest), line| {
                (lines.saturating_add(1), longest.max(line.chars().count()))
            });
        // Precision loss only past 2^24 characters, far beyond any annotation.
        let longest = longest as f32;
        Size::new(
            longest * Self::ESTIMATED_ADVANCE * font_size,
            f32::from(lines) * Self::LINE_HEIGHT * font_size,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(stroke_width: f32) -> Style {
        Style {
            stroke_width,
            ..Style::default()
        }
    }

    fn line(ax: f32, ay: f32, bx: f32, by: f32) -> Shape {
        Shape::Line(Line {
            start: Point::new(ax, ay),
            end: Point::new(bx, by),
        })
    }

    fn arrow(ax: f32, ay: f32, bx: f32, by: f32) -> Shape {
        Shape::Arrow(Arrow {
            start: Point::new(ax, ay),
            end: Point::new(bx, by),
        })
    }

    #[test]
    fn line_hit_reaches_half_the_stroke_plus_tolerance() {
        let shape = line(0.0, 0.0, 100.0, 0.0);
        let style = style(4.0);
        // Half stroke 2 + tolerance 3 = 5.
        assert!(shape.hit(&style, Point::new(50.0, 5.0), 3.0));
        assert!(!shape.hit(&style, Point::new(50.0, 5.01), 3.0));
        assert!(shape.hit(&style, Point::new(50.0, -2.0), 0.0));
        assert!(!shape.hit(&style, Point::new(50.0, -2.01), 0.0));
    }

    #[test]
    fn line_hit_area_has_round_caps() {
        let shape = line(0.0, 0.0, 100.0, 0.0);
        let style = style(2.0);
        // Reach is 1 + 4 = 5 from the endpoint (3-4-5 triangle).
        assert!(shape.hit(&style, Point::new(103.0, 4.0), 4.0));
        assert!(!shape.hit(&style, Point::new(103.0, 4.1), 4.0));
    }

    #[test]
    fn negative_tolerance_counts_as_zero() {
        let shape = line(0.0, 0.0, 100.0, 0.0);
        assert!(shape.hit(&style(4.0), Point::new(50.0, 2.0), -10.0));
    }

    #[test]
    fn arrow_head_has_documented_proportions() {
        let Shape::Arrow(a) = arrow(0.0, 0.0, 100.0, 0.0) else {
            unreachable!()
        };
        // Thin strokes use the minimum length.
        let thin = a.head(1.0).unwrap();
        assert_eq!(thin.tip, Point::new(100.0, 0.0));
        assert_eq!(thin.base, Point::new(90.0, 0.0));
        assert_eq!(thin.left.x, 90.0);
        assert_eq!((thin.left.y - thin.right.y).abs(), 10.0);
        // Thick strokes scale it.
        let thick = a.head(8.0).unwrap();
        assert_eq!(thick.base, Point::new(76.0, 0.0));
        // Short arrows cap it at their length.
        let Shape::Arrow(short) = arrow(0.0, 0.0, 4.0, 0.0) else {
            unreachable!()
        };
        assert_eq!(short.head(8.0).unwrap().base, Point::ORIGIN);
        // A zero-length arrow has no direction and no head.
        let Shape::Arrow(dot) = arrow(5.0, 5.0, 5.0, 5.0) else {
            unreachable!()
        };
        assert_eq!(dot.head(8.0), None);
    }

    #[test]
    fn arrow_hits_its_head_beyond_the_shaft() {
        let arrow = arrow(0.0, 0.0, 100.0, 0.0);
        let style = style(2.0);
        // Head: length 10, back corners at (90, ±5). A point at (92, 3) is
        // inside the head but 3 away from the shaft (reach 1).
        let beside_head = Point::new(92.0, 3.0);
        assert!(arrow.hit(&style, beside_head, 0.0));
        assert!(!line(0.0, 0.0, 100.0, 0.0).hit(&style, beside_head, 0.0));
        // Beside the shaft, away from the head, it misses.
        assert!(!arrow.hit(&style, Point::new(50.0, 3.0), 0.0));
        // Just outside a back corner: within tolerance hits, beyond misses.
        assert!(arrow.hit(&style, Point::new(90.0, 6.0), 1.0));
        assert!(!arrow.hit(&style, Point::new(90.0, 6.5), 1.0));
    }

    #[test]
    fn zero_length_arrow_hits_like_a_dot() {
        let dot = arrow(5.0, 5.0, 5.0, 5.0);
        assert!(dot.hit(&style(4.0), Point::new(7.0, 5.0), 0.0));
        assert!(!dot.hit(&style(4.0), Point::new(7.1, 5.0), 0.0));
    }

    #[test]
    fn rectangle_hits_its_outline_but_not_its_interior() {
        let shape = Shape::Rectangle(Rectangle {
            rect: Rect::from_corners(Point::new(10.0, 10.0), Point::new(110.0, 60.0)),
        });
        let style = style(4.0);
        // On each edge.
        for p in [(60.0, 10.0), (110.0, 35.0), (60.0, 60.0), (10.0, 35.0)] {
            assert!(shape.hit(&style, Point::new(p.0, p.1), 0.0), "{p:?}");
        }
        // Inside, the stroke reaches 2 + tolerance 1 inward.
        assert!(shape.hit(&style, Point::new(13.0, 35.0), 1.0));
        assert!(!shape.hit(&style, Point::new(13.1, 35.0), 1.0));
        assert!(!shape.hit(&style, Point::new(60.0, 35.0), 1.0));
        // Outside, likewise outward, with round corners.
        assert!(shape.hit(&style, Point::new(60.0, 7.0), 1.0));
        assert!(!shape.hit(&style, Point::new(60.0, 6.9), 1.0));
        assert!(shape.hit(&style, Point::new(112.0, 62.0), 1.0));
        assert!(!shape.hit(&style, Point::new(113.0, 63.0), 1.0));
    }

    #[test]
    fn text_estimate_uses_longest_line_and_line_count() {
        let size = Text::estimate_size("ab\nabcd\n", 10.0);
        assert_eq!(size.width, 4.0 * 0.6 * 10.0);
        assert_eq!(size.height, 3.0 * 1.2 * 10.0);
        let empty = Text::estimate_size("", 10.0);
        assert_eq!(empty.width, 0.0);
        assert_eq!(empty.height, 12.0);
        // Characters, not bytes.
        assert_eq!(Text::estimate_size("éé", 10.0).width, 12.0);
    }

    #[test]
    fn text_hits_its_box_grown_by_tolerance() {
        let text = Text::new(Point::new(10.0, 20.0), "hello");
        let style = Style {
            font_size: 10.0,
            ..Style::default()
        };
        // Estimated box: (10, 20) to (40, 32).
        let shape = Shape::Text(text.clone());
        assert!(shape.hit(&style, Point::new(25.0, 25.0), 0.0));
        assert!(shape.hit(&style, Point::new(40.0, 32.0), 0.0));
        assert!(!shape.hit(&style, Point::new(40.5, 32.0), 0.0));
        assert!(shape.hit(&style, Point::new(42.0, 32.0), 2.0));
        assert!(!shape.hit(&style, Point::new(10.0, 17.9), 2.0));

        // A measured size replaces the estimate.
        let mut measured = text;
        measured.set_measured(Some(Size::new(50.0, 12.0)));
        let shape = Shape::Text(measured);
        assert!(shape.hit(&style, Point::new(59.0, 25.0), 0.0));
        assert!(!shape.hit(&style, Point::new(61.0, 25.0), 0.0));
    }

    #[test]
    fn bounds_include_stroke_and_arrowhead() {
        let style = style(4.0);
        assert_eq!(
            line(10.0, 10.0, 30.0, 10.0).bounds(&style),
            Rect::from_corners(Point::new(8.0, 8.0), Point::new(32.0, 12.0))
        );
        // Head length 12, half-width 6: wider than the stroke.
        assert_eq!(
            arrow(0.0, 0.0, 100.0, 0.0).bounds(&style),
            Rect::from_corners(Point::new(-2.0, -6.0), Point::new(100.0, 6.0))
        );
    }

    #[test]
    fn ellipse_hits_its_outline_but_not_its_interior() {
        // Radii 50 and 25 around (60, 35).
        let rect = Rect::from_corners(Point::new(10.0, 10.0), Point::new(110.0, 60.0));
        let shape = Shape::Ellipse(Ellipse { rect });
        let style = style(4.0);
        // The vertices, and just past the reach (2 + 1) beside them.
        assert!(shape.hit(&style, Point::new(110.0, 35.0), 0.0));
        assert!(shape.hit(&style, Point::new(113.0, 35.0), 1.0));
        assert!(!shape.hit(&style, Point::new(113.1, 35.0), 1.0));
        assert!(shape.hit(&style, Point::new(60.0, 13.0), 1.0));
        assert!(!shape.hit(&style, Point::new(60.0, 13.1), 1.0));
        // The center and the rectangle's corners are far from the outline.
        assert!(!shape.hit(&style, rect.center(), 1.0));
        assert!(!shape.hit(&style, Point::new(11.0, 11.0), 1.0));
        assert_eq!(shape.bounds(&style), rect.expand(2.0));
    }

    #[test]
    fn ellipse_curves_pass_through_the_four_vertices() {
        let ellipse = Ellipse {
            rect: Rect::from_corners(Point::new(0.0, 0.0), Point::new(20.0, 10.0)),
        };
        let (start, curves) = ellipse.curves();
        assert_eq!(start, Point::new(20.0, 5.0));
        let ends: Vec<_> = curves.iter().map(|curve| curve[2]).collect();
        assert_eq!(
            ends,
            [
                Point::new(10.0, 10.0),
                Point::new(0.0, 5.0),
                Point::new(10.0, 0.0),
                Point::new(20.0, 5.0),
            ]
        );
        // Each quarter's midpoint (t = ½) lies on the true ellipse, to well
        // under a pixel.
        let mut from = start;
        for [c1, c2, to] in curves {
            let mid = Point::new(
                (from.x + 3.0 * c1.x + 3.0 * c2.x + to.x) / 8.0,
                (from.y + 3.0 * c1.y + 3.0 * c2.y + to.y) / 8.0,
            );
            assert!(distance_to_ellipse(mid, ellipse.rect) < 0.01, "{mid:?}");
            from = to;
        }
    }

    fn pen(points: &[(f32, f32)]) -> Shape {
        Shape::Pen(Polyline {
            points: points.iter().map(|&(x, y)| Point::new(x, y)).collect(),
        })
    }

    #[test]
    fn pen_hits_near_its_path_and_bounds_every_point() {
        let shape = pen(&[(0.0, 0.0), (10.0, 0.0), (10.0, 20.0), (30.0, 25.0)]);
        let style = style(4.0);
        // Beside the middle segment, within 2 + 1 and just beyond.
        assert!(shape.hit(&style, Point::new(13.0, 10.0), 1.0));
        assert!(!shape.hit(&style, Point::new(13.1, 10.0), 1.0));
        // Inside the path's bounding box but away from the path.
        assert!(!shape.hit(&style, Point::new(20.0, 5.0), 1.0));
        assert_eq!(
            shape.bounds(&style),
            Rect::from_corners(Point::new(-2.0, -2.0), Point::new(32.0, 27.0))
        );
        // One point is a dot; none is nothing.
        assert!(pen(&[(5.0, 5.0)]).hit(&style, Point::new(7.0, 5.0), 0.0));
        assert!(!pen(&[]).hit(&style, Point::ORIGIN, 100.0));
    }

    #[test]
    fn a_step_marker_is_a_disc_sized_by_the_font() {
        let shape = Shape::Step(StepMarker {
            center: Point::new(50.0, 50.0),
        });
        // Font size 20: radius 16; the stroke width does not matter.
        let style = Style {
            font_size: 20.0,
            stroke_width: 100.0,
            ..Style::default()
        };
        assert!(shape.hit(&style, Point::new(50.0, 50.0), 0.0));
        assert!(shape.hit(&style, Point::new(50.0, 67.0), 1.0));
        assert!(!shape.hit(&style, Point::new(50.0, 67.1), 1.0));
        assert_eq!(
            shape.bounds(&style),
            Rect::from_corners(Point::new(34.0, 34.0), Point::new(66.0, 66.0))
        );
    }

    #[test]
    fn step_numbers_contrast_with_their_disc() {
        let white = Rgba8::rgb(255, 255, 255);
        let black = Rgba8::rgb(0, 0, 0);
        assert_eq!(StepMarker::number_color(Rgba8::rgb(255, 204, 0)), black);
        assert_eq!(StepMarker::number_color(white), black);
        assert_eq!(StepMarker::number_color(Style::DEFAULT_COLOR), white);
        assert_eq!(StepMarker::number_color(Rgba8::rgb(0, 122, 255)), white);
        // The disc's alpha carries over.
        assert_eq!(
            StepMarker::number_color(Rgba8::new(0, 0, 0, 100)),
            Rgba8::new(255, 255, 255, 100)
        );
    }

    #[test]
    fn a_highlighter_reaches_its_wider_width() {
        let points = vec![Point::new(0.0, 0.0), Point::new(40.0, 0.0)];
        let shape = Shape::Highlighter(Polyline { points });
        // Stroke width 3 draws 12 wide: a reach of 6 + tolerance 1.
        let style = style(3.0);
        assert!(shape.hit(&style, Point::new(20.0, 7.0), 1.0));
        assert!(!shape.hit(&style, Point::new(20.0, 7.1), 1.0));
        assert_eq!(
            shape.bounds(&style),
            Rect::from_corners(Point::new(-6.0, -6.0), Point::new(46.0, 6.0))
        );
    }

    #[test]
    fn a_blur_region_hits_all_over_and_acts_on_rounded_pixels() {
        let region = BlurRegion {
            rect: Rect::from_corners(Point::new(10.4, 20.6), Point::new(30.5, 40.0)),
        };
        let shape = Shape::Blur(region.clone());
        let style = style(100.0);
        // Inside hits, however far from the edge; the stroke width is ignored.
        assert!(shape.hit(&style, Point::new(20.0, 30.0), 0.0));
        assert!(shape.hit(&style, Point::new(32.5, 30.0), 2.0));
        assert!(!shape.hit(&style, Point::new(32.6, 30.0), 2.0));
        assert_eq!(shape.bounds(&style), region.rect);
        // Each edge rounds to the nearest pixel boundary (30.5 rounds away).
        assert_eq!(region.pixels(), Some(PhysicalRect::new(10, 21, 21, 19)));
        // Off the image is fine; the image clips it.
        let off = BlurRegion {
            rect: Rect::from_corners(Point::new(-5.0, -5.0), Point::new(2.0, 2.0)),
        };
        assert_eq!(off.pixels(), Some(PhysicalRect::new(-5, -5, 7, 7)));
        // Thinner than half a pixel: nothing.
        let thin = BlurRegion {
            rect: Rect::from_corners(Point::new(4.6, 0.0), Point::new(5.4, 10.0)),
        };
        assert_eq!(thin.pixels(), None);
    }

    #[test]
    fn translate_moves_every_point_of_every_kind() {
        let delta = Vector::new(3.0, -2.0);
        let rect = Rect::from_corners(Point::ORIGIN, Point::new(4.0, 4.0));
        let cases = [
            (line(0.0, 0.0, 1.0, 1.0), line(3.0, -2.0, 4.0, -1.0)),
            (arrow(0.0, 0.0, 1.0, 1.0), arrow(3.0, -2.0, 4.0, -1.0)),
            (
                Shape::Rectangle(Rectangle { rect }),
                Shape::Rectangle(Rectangle {
                    rect: rect.translate(delta),
                }),
            ),
            (
                Shape::Ellipse(Ellipse { rect }),
                Shape::Ellipse(Ellipse {
                    rect: rect.translate(delta),
                }),
            ),
            (
                pen(&[(0.0, 0.0), (1.0, 5.0)]),
                pen(&[(3.0, -2.0), (4.0, 3.0)]),
            ),
            (
                Shape::Highlighter(Polyline {
                    points: vec![Point::ORIGIN],
                }),
                Shape::Highlighter(Polyline {
                    points: vec![Point::new(3.0, -2.0)],
                }),
            ),
            (
                Shape::Step(StepMarker {
                    center: Point::ORIGIN,
                }),
                Shape::Step(StepMarker {
                    center: Point::new(3.0, -2.0),
                }),
            ),
            (
                Shape::Text(Text::new(Point::ORIGIN, "x")),
                Shape::Text(Text::new(Point::new(3.0, -2.0), "x")),
            ),
            (
                Shape::Blur(BlurRegion { rect }),
                Shape::Blur(BlurRegion {
                    rect: rect.translate(delta),
                }),
            ),
        ];
        for (mut shape, expected) in cases {
            shape.translate(delta);
            assert_eq!(shape, expected);
        }
    }
}
