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
    distance_to_ellipse, distance_to_polyline, distance_to_segment, distance_to_tapered_segment,
    distance_to_triangle, Point, Rect, Size, Vector,
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
    /// - Arrow: within `tolerance` of the tapered shaft (`start` to
    ///   [`ArrowHead::base`]; see [`Arrow`]) or of the filled [`ArrowHead`]
    ///   triangle.
    /// - Rectangle: within `stroke_width / 2 + tolerance` of the outline, its
    ///   corners rounded to [`Rectangle::radius`] (so the outer corners are
    ///   rounded, like the stroke's round joins, even at a radius of zero).
    ///   The interior does not hit, so annotations and image content inside
    ///   an outline stay clickable. (A future filled rectangle would also hit
    ///   inside.)
    /// - Ellipse: within `stroke_width / 2 + tolerance` of the outline
    ///   ([`distance_to_ellipse`]); like a rectangle, not inside.
    /// - Pen: within `stroke_width / 2 + tolerance` of the path.
    /// - Highlighter: within [`highlighter::width`]` / 2 + tolerance` of the
    ///   path.
    /// - Step marker: within `tolerance` of its disc ([`StepMarker::radius`]).
    /// - Blur region: inside its rectangle grown by `tolerance`; it covers
    ///   what is beneath it, so all of it hits.
    /// - Text: inside [`Text::covered`] (its background, or if it has none
    ///   its layout box) grown by `tolerance`.
    #[must_use]
    pub fn hit(&self, style: &Style, point: Point, tolerance: f32) -> bool {
        let tolerance = tolerance.max(0.0);
        let reach = half_stroke(style) + tolerance;
        match self {
            Self::Line(line) => distance_to_segment(point, line.start, line.end) <= reach,
            Self::Arrow(arrow) => match arrow.head(style.stroke_width) {
                Some(head) => {
                    let (tail, base) = Arrow::shaft_radii(style.stroke_width);
                    distance_to_tapered_segment(point, arrow.start, tail, head.base, base)
                        <= tolerance
                        || distance_to_triangle(point, head.corners()) <= tolerance
                }
                None => distance_to_segment(point, arrow.start, arrow.end) <= reach,
            },
            Self::Rectangle(rectangle) => {
                let radius = rectangle.radius(style.corner_radius);
                rectangle.rect.distance_to_outline(point, radius) <= reach
            }
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
            Self::Text(text) => text.covered(style).expand(tolerance).contains(point),
        }
    }

    /// The smallest rectangle containing everything the shape draws with
    /// `style`: strokes reach `stroke_width / 2` past their path in every
    /// direction (round caps and joins; see [`Style`]), arrows include their
    /// tapered shaft and head, text its background or layout box. Used for
    /// selection outlines and marquee selection.
    #[must_use]
    pub fn bounds(&self, style: &Style) -> Rect {
        let half = half_stroke(style);
        match self {
            Self::Line(line) => Rect::from_corners(line.start, line.end).expand(half),
            Self::Arrow(arrow) => match arrow.head(style.stroke_width) {
                Some(head) => {
                    let (tail, base) = Arrow::shaft_radii(style.stroke_width);
                    Rect::from_corners(arrow.start, arrow.start)
                        .expand(tail)
                        .union(&Rect::from_corners(head.base, head.base).expand(base))
                        .union(&Rect::from_corners(head.left, head.right))
                        .union(&Rect::from_corners(head.tip, head.tip))
                }
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
            Self::Text(text) => text.covered(style),
        }
    }

    /// The style fields the shape draws with, the only ones restyling
    /// changes ([`Command::Restyle`](super::Command::Restyle)): color and
    /// stroke width for strokes (a line, arrow, ellipse, pen, or
    /// highlighter), those and the corner radius for rectangles, color and
    /// font size for step markers, those and the background for text, and
    /// only the blur mode for a blur region.
    #[must_use]
    pub const fn style_fields(&self) -> StyleFields {
        match self {
            Self::Line(_)
            | Self::Arrow(_)
            | Self::Ellipse(_)
            | Self::Pen(_)
            | Self::Highlighter(_) => StyleFields::STROKE,
            Self::Rectangle(_) => StyleFields::RECTANGLE,
            Self::Step(_) => StyleFields::TEXT,
            Self::Text(_) => StyleFields::TEXT_BOX,
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

    /// Scales the shape's geometry about the origin by `x` horizontally and
    /// `y` vertically (both positive), as when the image beneath it is
    /// resized. Only positions scale: stroke widths and font sizes are the
    /// style's, and text's measured size is cleared, as its layout changes.
    pub fn scale(&mut self, x: f32, y: f32) {
        match self {
            Self::Line(line) => {
                line.start = line.start.scaled(x, y);
                line.end = line.end.scaled(x, y);
            }
            Self::Arrow(arrow) => {
                arrow.start = arrow.start.scaled(x, y);
                arrow.end = arrow.end.scaled(x, y);
            }
            Self::Rectangle(rectangle) => rectangle.rect = rectangle.rect.scaled(x, y),
            Self::Ellipse(ellipse) => ellipse.rect = ellipse.rect.scaled(x, y),
            Self::Pen(stroke) | Self::Highlighter(stroke) => {
                for point in &mut stroke.points {
                    *point = point.scaled(x, y);
                }
            }
            Self::Step(step) => step.center = step.center.scaled(x, y),
            Self::Blur(region) => region.rect = region.rect.scaled(x, y),
            Self::Text(text) => {
                text.position = text.position.scaled(x, y);
                text.set_measured(None);
            }
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

pub mod shadow {
    //! The subtle drop shadow cast by lines, arrows, rectangles, ellipses,
    //! pen strokes, and step markers (their discs, not their numbers); see
    //! [`casts`].
    //!
    //! A shadow is the shape's silhouette (the shape as drawn, in black at
    //! the color's own alpha) moved by [`OFFSET`], blurred with
    //! `chartreuse_imaging::blur` at a radius of [`BLUR_RADIUS`], and faded
    //! to [`OPACITY`]. It is computed in image pixels, beneath its shape and
    //! above everything below it, so it looks the same at every zoom and in
    //! the export.

    use super::{Rect, Shape, Style, Vector};

    /// How far the shadow falls from its shape: right and down, as if lit
    /// from the top left.
    pub const OFFSET: Vector = Vector::new(1.0, 2.0);

    /// The blur's radius, in image pixels.
    pub const BLUR_RADIUS: u32 = 2;

    /// How far the blur spreads the silhouette: its three box-blur passes
    /// each reach [`BLUR_RADIUS`] further.
    pub const SPREAD: f32 = 3.0 * BLUR_RADIUS as f32;

    /// The shadow's opacity where the silhouette is solid, times the
    /// color's own alpha.
    pub const OPACITY: f32 = 0.35;

    /// Whether `shape` casts a shadow: lines, arrows, rectangles, ellipses,
    /// pen strokes, and step markers do. Highlighters are translucent ink,
    /// blur regions part of the image, and text sits on its own background.
    #[must_use]
    pub const fn casts(shape: &Shape) -> bool {
        matches!(
            shape,
            Shape::Line(_)
                | Shape::Arrow(_)
                | Shape::Rectangle(_)
                | Shape::Ellipse(_)
                | Shape::Pen(_)
                | Shape::Step(_)
        )
    }

    /// A rectangle outside which the shadow `shape` casts in `style` (if it
    /// casts one) is transparent: its bounds moved by [`OFFSET`] and grown
    /// by the [`SPREAD`], plus a pixel for anti-aliasing.
    #[must_use]
    pub fn reach(shape: &Shape, style: &Style) -> Rect {
        shape.bounds(style).translate(OFFSET).expand(SPREAD + 1.0)
    }
}

/// A straight line segment, stroked with round caps (see [`Style`]). A
/// zero-length line draws a dot `stroke_width` across.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub start: Point,
    pub end: Point,
}

/// A tapered line with an arrowhead at `end`.
///
/// The geometry comes from [`Arrow::head`] and [`Arrow::outline`] so the
/// canvas, flatten, and hit-testing agree on it. The shaft runs from `start`
/// to [`ArrowHead::base`], narrowing away from the head: it is the convex
/// hull of a disc `stroke_width` across at the base and one
/// [`TAIL_WIDTH_RATIO`](Self::TAIL_WIDTH_RATIO) as wide at `start`, so the
/// tail is round. The head covers the shaft's round end at the base, and
/// ending the shaft there keeps a thick shaft from poking out past the head's
/// point. Renderers fill the outline, shaft and head in one path, never
/// stroked, so a translucent arrow is one even tint.
///
/// An arrow shorter than its head's full length is all head (`base` is
/// `start`, so the shaft is the disc at the base); a zero-length arrow has no
/// head and draws a dot `stroke_width` across, like a zero-length line.
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
    /// The shaft's width at the tail per unit of its width at the head (the
    /// stroke width).
    pub const TAIL_WIDTH_RATIO: f32 = 0.25;

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

    /// The radii of the shaft's discs for a given stroke width: at the tail
    /// (`start`) and at the head's base.
    #[must_use]
    pub fn shaft_radii(stroke_width: f32) -> (f32, f32) {
        let base = stroke_width.max(0.0) / 2.0;
        (base * Self::TAIL_WIDTH_RATIO, base)
    }

    /// The outline of the shaft and head together for a given stroke width,
    /// as a closed path: a start point where the shaft's side leaves the
    /// tail's disc, then that side to the base's disc, around it to the
    /// head's back edge, the head (a back corner, the tip, the other back
    /// corner), back around the base's disc, the other side back to the
    /// tail, and the tail's round end. `None` for a zero-length arrow.
    ///
    /// The sides are tangent to both of the shaft's discs. Where the tail's
    /// disc lies within the base's (an arrow that is all head, or nearly),
    /// the shaft is the base's disc, its back half the round end.
    #[must_use]
    pub fn outline(&self, stroke_width: f32) -> Option<(Point, [PathSegment; 10])> {
        let head = self.head(stroke_width)?;
        let along = head.tip - head.base;
        let u = along * (1.0 / along.length());
        let n = u.perpendicular();
        let (tail_radius, base_radius) = Self::shaft_radii(stroke_width);
        let length = (head.base - self.start).length();
        let (tail, tail_radius, lean) = if length > base_radius - tail_radius {
            // The sides' outward normals lean back from straight across by
            // this angle, as the shaft widens toward the head.
            let lean = ((base_radius - tail_radius) / length).asin();
            (self.start, tail_radius, lean)
        } else {
            (head.base, base_radius, 0.0)
        };
        // The direction at `angle` from `u` toward `n`, and its derivative.
        let at = |angle: f32| u * angle.cos() + n * angle.sin();
        let turning = |angle: f32| n * angle.cos() + u * -angle.sin();
        // A cubic Bézier along the circle around `center` from angle `from`
        // to `to` (at most a quarter turn).
        let arc = |center: Point, radius: f32, from: f32, to: f32| {
            let k = radius * 4.0 / 3.0 * ((to - from) / 4.0).tan();
            let (a, b) = (center + at(from) * radius, center + at(to) * radius);
            PathSegment::Cubic([a + turning(from) * k, b - turning(to) * k, b])
        };
        let half_pi = std::f32::consts::FRAC_PI_2;
        // The sides' outward normals; straight across either way; straight
        // back.
        let (plus, minus) = (half_pi + lean, 3.0 * half_pi - lean);
        let (right, left, back) = (half_pi, 3.0 * half_pi, 2.0 * half_pi);
        Some((
            tail + at(plus) * tail_radius,
            [
                PathSegment::Line(head.base + at(plus) * base_radius),
                arc(head.base, base_radius, plus, right),
                PathSegment::Line(head.right),
                PathSegment::Line(head.tip),
                PathSegment::Line(head.left),
                PathSegment::Line(head.base + at(left) * base_radius),
                arc(head.base, base_radius, left, minus),
                PathSegment::Line(tail + at(minus) * tail_radius),
                arc(tail, tail_radius, minus, back),
                arc(tail, tail_radius, back, plus),
            ],
        ))
    }
}

/// A piece of a path, from the point the path has reached.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathSegment {
    /// A straight line to the point.
    Line(Point),
    /// A cubic Bézier `[control, control, end]`.
    Cubic([Point; 3]),
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

/// An unfilled rectangle outline, stroke centered on `rect`'s edges with its
/// corners rounded to [`Rectangle::radius`] (see [`Rectangle::outline`]), and
/// round joins, so even at a radius of zero its outer corners are rounded
/// (see [`Style`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Rectangle {
    pub rect: Rect,
}

impl Rectangle {
    /// The radius its corners are rounded to for a style's `corner_radius`:
    /// that, but no more than half the shorter side (so a square can round
    /// into a circle) and no less than zero.
    #[must_use]
    pub fn radius(&self, corner_radius: f32) -> f32 {
        let shorter = self.rect.width().min(self.rect.height());
        corner_radius.min(shorter / 2.0).max(0.0)
    }

    /// The outline for a style's `corner_radius`, as a closed path: a start
    /// point on the top edge, then, clockwise on screen from the top-right
    /// corner, for each corner the end of the edge before it and the
    /// quarter-circle cubic Bézier `[control, control, end]` around it.
    ///
    /// Renderers draw a line to each edge end, then the corner's curve,
    /// except at a [radius](Self::radius) of zero: then each edge ends at its
    /// corner, and the curves, which would be empty, are left out.
    #[must_use]
    pub fn outline(&self, corner_radius: f32) -> (Point, [(Point, [Point; 3]); 4]) {
        let r = self.radius(corner_radius);
        let k = r * (1.0 - Ellipse::KAPPA);
        let (min, max) = (self.rect.min(), self.rect.max());
        let p = Point::new;
        (
            p(min.x + r, min.y),
            [
                (
                    p(max.x - r, min.y),
                    [
                        p(max.x - k, min.y),
                        p(max.x, min.y + k),
                        p(max.x, min.y + r),
                    ],
                ),
                (
                    p(max.x, max.y - r),
                    [
                        p(max.x, max.y - k),
                        p(max.x - k, max.y),
                        p(max.x - r, max.y),
                    ],
                ),
                (
                    p(min.x + r, max.y),
                    [
                        p(min.x + k, max.y),
                        p(min.x, max.y - k),
                        p(min.x, max.y - r),
                    ],
                ),
                (
                    p(min.x, min.y + r),
                    [
                        p(min.x, min.y + k),
                        p(min.x + k, min.y),
                        p(min.x + r, min.y),
                    ],
                ),
            ],
        )
    }
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
///
/// # Background
///
/// Text is drawn on a background: a rectangle filled with the style's
/// [`text_background_fill`](Style::text_background_fill) ([`Text::background`]),
/// its layout box grown by [`Text::BACKGROUND_PADDING`] on every side. A fully
/// transparent background draws nothing and does not count as part of the text.
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
    /// How far the background reaches past the layout box on each side, per
    /// unit of font size.
    pub const BACKGROUND_PADDING: f32 = 0.2;

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

    /// The background behind a layout box `bounds` of text at `font_size`:
    /// the box grown by [`Text::BACKGROUND_PADDING`] × `font_size`, its edges
    /// then rounded to whole pixels so they are crisp.
    #[must_use]
    pub fn background(bounds: Rect, font_size: f32) -> Rect {
        let padded = bounds.expand(Self::BACKGROUND_PADDING * font_size.max(0.0));
        padded.pixels().map_or(padded, Rect::from_pixels)
    }

    /// What the text covers drawn in `style`: its [background](Self::background)
    /// if that shows (is not fully transparent), otherwise its layout box.
    #[must_use]
    pub fn covered(&self, style: &Style) -> Rect {
        let bounds = self.bounds(style.font_size);
        if style.text_background_fill().a == 0 {
            bounds
        } else {
            Self::background(bounds, style.font_size)
        }
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
    fn arrow_shaft_narrows_toward_the_tail() {
        // Head length 12, so the shaft is 4 wide at x = 88 and 1 at the tail.
        let arrow = arrow(0.0, 0.0, 100.0, 0.0);
        let style = style(4.0);
        assert!(
            arrow.hit(&style, Point::new(80.0, 1.8), 0.0),
            "near the head"
        );
        assert!(
            !arrow.hit(&style, Point::new(5.0, 1.8), 0.0),
            "near the tail"
        );
        assert!(arrow.hit(&style, Point::new(5.0, 0.5), 0.0));
        // The tail is round.
        assert!(arrow.hit(&style, Point::new(-0.5, 0.0), 0.0));
        assert!(!arrow.hit(&style, Point::new(-0.4, 0.4), 0.0));
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
        let style = Style {
            corner_radius: 0.0,
            ..style(4.0)
        };
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

        // Rounded corners: the reach follows the arc, centered at (100, 50).
        let rounded = Style {
            corner_radius: 10.0,
            ..style
        };
        assert!(shape.hit(&rounded, Point::new(109.1, 59.1), 1.0));
        assert!(!shape.hit(&rounded, Point::new(109.4, 59.4), 1.0));
        assert!(!shape.hit(&rounded, Point::new(112.0, 62.0), 1.0));
        assert!(
            shape.hit(&rounded, Point::new(60.0, 7.0), 1.0),
            "edges unchanged"
        );
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
            text_background_opacity: 0,
            ..Style::default()
        };
        // Estimated box: (10, 20) to (40, 32).
        let shape = Shape::Text(text.clone());
        assert!(shape.hit(&style, Point::new(25.0, 25.0), 0.0));
        assert!(shape.hit(&style, Point::new(40.0, 32.0), 0.0));
        assert!(!shape.hit(&style, Point::new(40.5, 32.0), 0.0));
        assert!(shape.hit(&style, Point::new(42.0, 32.0), 2.0));
        assert!(!shape.hit(&style, Point::new(10.0, 17.9), 2.0));

        // A background that shows is part of the text: the box grown by 2.
        let backed = Style {
            text_background_opacity: Style::DEFAULT_TEXT_BACKGROUND_OPACITY,
            ..style
        };
        assert!(shape.hit(&backed, Point::new(42.0, 34.0), 0.0));
        assert!(!shape.hit(&backed, Point::new(42.5, 34.0), 0.0));
        assert_eq!(
            shape.bounds(&backed),
            Rect::from_corners(Point::new(8.0, 18.0), Point::new(42.0, 34.0))
        );

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
        // Head length 12, half-width 6: wider than the stroke. The tail is a
        // quarter as wide as the stroke.
        assert_eq!(
            arrow(0.0, 0.0, 100.0, 0.0).bounds(&style),
            Rect::from_corners(Point::new(-0.5, -6.0), Point::new(100.0, 6.0))
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
