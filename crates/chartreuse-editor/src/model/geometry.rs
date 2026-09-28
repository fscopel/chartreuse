//! Continuous 2D geometry in document (base-image pixel) coordinates.
//!
//! Unlike `chartreuse_core::geometry`, which models whole pixels on the desktop,
//! these types are `f32` and continuous: an annotation can sit between pixels.
//! The origin is the base image's top-left corner, `x` grows right and `y`
//! grows down, and one unit is one base-image pixel.

use std::ops::{Add, AddAssign, Mul, Neg, Sub};

use chartreuse_core::geometry::PhysicalRect;

/// A position in document coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ORIGIN: Self = Self::new(0.0, 0.0);

    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// The Euclidean distance to `other`.
    #[must_use]
    pub fn distance(self, other: Self) -> f32 {
        (other - self).length()
    }

    /// The point halfway between `self` and `other`.
    #[must_use]
    pub fn midpoint(self, other: Self) -> Self {
        Self::new((self.x + other.x) / 2.0, (self.y + other.y) / 2.0)
    }

    /// True if both coordinates are finite.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// A displacement in document coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vector {
    pub x: f32,
    pub y: f32,
}

impl Vector {
    pub const ZERO: Self = Self::new(0.0, 0.0);

    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    #[must_use]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
    }

    /// The z component of the 3D cross product; positive when `other` is
    /// clockwise from `self` in y-down coordinates.
    #[must_use]
    pub fn cross(self, other: Self) -> f32 {
        self.x * other.y - self.y * other.x
    }

    /// This vector rotated a quarter turn (clockwise on screen, y down).
    #[must_use]
    pub const fn perpendicular(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// True if both components are finite.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Add<Vector> for Point {
    type Output = Self;
    fn add(self, rhs: Vector) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign<Vector> for Point {
    fn add_assign(&mut self, rhs: Vector) {
        *self = *self + rhs;
    }
}

impl Sub<Vector> for Point {
    type Output = Self;
    fn sub(self, rhs: Vector) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Sub for Point {
    type Output = Vector;
    fn sub(self, rhs: Self) -> Vector {
        Vector::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Add for Vector {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Neg for Vector {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl Mul<f32> for Vector {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

/// A width and height in document coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// An axis-aligned rectangle, always normalized (`min` is the top-left corner,
/// `max` the bottom-right; width and height are never negative).
///
/// Geometric queries treat it as closed: points on its edges are inside.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    min: Point,
    max: Point,
}

impl Rect {
    /// The rectangle spanned by two opposite corners, in either order (as from a
    /// drag).
    #[must_use]
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self {
            min: Point::new(a.x.min(b.x), a.y.min(b.y)),
            max: Point::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    /// The rectangle with top-left corner `origin` and `size`. A negative width
    /// or height extends left or up from `origin` instead.
    #[must_use]
    pub fn new(origin: Point, size: Size) -> Self {
        Self::from_corners(
            origin,
            Point::new(origin.x + size.width, origin.y + size.height),
        )
    }

    /// The top-left corner.
    #[must_use]
    pub const fn min(&self) -> Point {
        self.min
    }

    /// The bottom-right corner.
    #[must_use]
    pub const fn max(&self) -> Point {
        self.max
    }

    #[must_use]
    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    #[must_use]
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    #[must_use]
    pub fn size(&self) -> Size {
        Size::new(self.width(), self.height())
    }

    #[must_use]
    pub fn center(&self) -> Point {
        self.min.midpoint(self.max)
    }

    /// The four corners, clockwise from the top-left.
    #[must_use]
    pub const fn corners(&self) -> [Point; 4] {
        [
            self.min,
            Point::new(self.max.x, self.min.y),
            self.max,
            Point::new(self.min.x, self.max.y),
        ]
    }

    /// True if `point` is inside or on the edge.
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        (self.min.x..=self.max.x).contains(&point.x) && (self.min.y..=self.max.y).contains(&point.y)
    }

    /// True if the two rectangles share at least one point (touching counts).
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.min.x <= other.max.x
            && other.min.x <= self.max.x
            && self.min.y <= other.max.y
            && other.min.y <= self.max.y
    }

    /// The smallest rectangle containing both.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: Point::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Point::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    /// Grown by `amount` on every side (shrunk if negative, never past a
    /// zero-size rectangle at the center).
    #[must_use]
    pub fn expand(&self, amount: f32) -> Self {
        let center = self.center();
        let half_w = (self.width() / 2.0 + amount).max(0.0);
        let half_h = (self.height() / 2.0 + amount).max(0.0);
        Self {
            min: Point::new(center.x - half_w, center.y - half_h),
            max: Point::new(center.x + half_w, center.y + half_h),
        }
    }

    /// Moved by `delta`.
    #[must_use]
    pub fn translate(&self, delta: Vector) -> Self {
        Self {
            min: self.min + delta,
            max: self.max + delta,
        }
    }

    /// The whole pixels the rectangle covers once each edge is rounded to the
    /// nearest pixel boundary (so it may reach past an image, which clips
    /// it). `None` if that leaves no pixels.
    #[must_use]
    pub fn pixels(&self) -> Option<PhysicalRect> {
        // Far beyond any image, and small enough that the differences below
        // cannot overflow.
        const LIMIT: f32 = (1 << 30) as f32;
        let edge = |value: f32| value.round().clamp(-LIMIT, LIMIT) as i32;
        let (x0, y0) = (edge(self.min.x), edge(self.min.y));
        let width = u32::try_from(edge(self.max.x) - x0).ok()?;
        let height = u32::try_from(edge(self.max.y) - y0).ok()?;
        let pixels = PhysicalRect::new(x0, y0, width, height);
        (!pixels.is_empty()).then_some(pixels)
    }

    /// The rectangle a block of whole pixels covers.
    #[must_use]
    pub fn from_pixels(pixels: PhysicalRect) -> Self {
        let min = Point::new(pixels.origin.x as f32, pixels.origin.y as f32);
        Self::new(
            min,
            Size::new(pixels.size.width as f32, pixels.size.height as f32),
        )
    }

    /// The distance from `point` to the nearest point on the rectangle's outline,
    /// whether `point` is inside or outside.
    #[must_use]
    pub fn distance_to_outline(&self, point: Point) -> f32 {
        let [a, b, c, d] = self.corners();
        [(a, b), (b, c), (c, d), (d, a)]
            .into_iter()
            .map(|(start, end)| distance_to_segment(point, start, end))
            .fold(f32::INFINITY, f32::min)
    }
}

/// The distance from `point` to the closest point of the segment `start..=end`.
/// A zero-length segment is a single point.
#[must_use]
pub fn distance_to_segment(point: Point, start: Point, end: Point) -> f32 {
    let along = end - start;
    let length_sq = along.dot(along);
    if length_sq == 0.0 {
        return point.distance(start);
    }
    let t = ((point - start).dot(along) / length_sq).clamp(0.0, 1.0);
    point.distance(start + along * t)
}

/// The distance from `point` to the closest point of the path through
/// `points` in order (a single point is itself; no points is infinitely far).
#[must_use]
pub fn distance_to_polyline(point: Point, points: &[Point]) -> f32 {
    match points {
        [] => f32::INFINITY,
        [only] => point.distance(*only),
        _ => points
            .windows(2)
            .map(|pair| distance_to_segment(point, pair[0], pair[1]))
            .fold(f32::INFINITY, f32::min),
    }
}

/// The distance from `point` to the filled triangle `corners`: zero inside or on
/// an edge, otherwise the distance to the nearest edge. Works for either
/// winding and for degenerate (collinear) triangles.
#[must_use]
pub fn distance_to_triangle(point: Point, corners: [Point; 3]) -> f32 {
    let [a, b, c] = corners;
    let d1 = (b - a).cross(point - a);
    let d2 = (c - b).cross(point - b);
    let d3 = (a - c).cross(point - c);
    let has_negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    let area = (b - a).cross(c - a);
    if area != 0.0 && !(has_negative && has_positive) {
        return 0.0;
    }
    distance_to_segment(point, a, b)
        .min(distance_to_segment(point, b, c))
        .min(distance_to_segment(point, c, a))
}

/// The distance from `point` to the outline of the ellipse inscribed in
/// `bounds` (axis-aligned, centered in it, touching all four sides), whether
/// `point` is inside or outside. An ellipse of zero width or height is the
/// segment it collapses to, and one of zero size is a single point.
///
/// Exact up to floating-point rounding: it finds the nearest outline point
/// by bisection (David Eberly, "Distance from a Point to an Ellipse"), in
/// `f64`.
#[must_use]
pub fn distance_to_ellipse(point: Point, bounds: Rect) -> f32 {
    let center = bounds.center();
    let radii = (
        f64::from(bounds.width()) / 2.0,
        f64::from(bounds.height()) / 2.0,
    );
    // By symmetry, work in the first quadrant, with the major axis first.
    let offset = (
        f64::from((point.x - center.x).abs()),
        f64::from((point.y - center.y).abs()),
    );
    let ((e0, y0), (e1, y1)) = if radii.0 >= radii.1 {
        ((radii.0, offset.0), (radii.1, offset.1))
    } else {
        ((radii.1, offset.1), (radii.0, offset.0))
    };
    let distance = if e1 == 0.0 {
        // A segment along the major axis (or a point).
        (y0 - e0).max(0.0).hypot(y1)
    } else if y1 > 0.0 {
        if y0 > 0.0 {
            let (z0, z1) = (y0 / e0, y1 / e1);
            let g = z0 * z0 + z1 * z1 - 1.0;
            if g == 0.0 {
                0.0
            } else {
                let r0 = (e0 / e1) * (e0 / e1);
                let s = ellipse_root(r0, z0, z1, g);
                let x0 = r0 * y0 / (s + r0);
                let x1 = y1 / (s + 1.0);
                (x0 - y0).hypot(x1 - y1)
            }
        } else {
            (y1 - e1).abs()
        }
    } else {
        let numerator = e0 * y0;
        let denominator = e0 * e0 - e1 * e1;
        if numerator < denominator {
            let ratio = numerator / denominator;
            let x0 = e0 * ratio;
            let x1 = e1 * (1.0 - ratio * ratio).sqrt();
            (x0 - y0).hypot(x1)
        } else {
            (y0 - e0).abs()
        }
    };
    // Narrowing back to the model's precision is intended.
    distance as f32
}

/// The root `s` of `(r0·z0 / (s + r0))² + (z1 / (s + 1))² = 1` that
/// [`distance_to_ellipse`] needs, by bisection until the interval stops
/// shrinking.
fn ellipse_root(r0: f64, z0: f64, z1: f64, g: f64) -> f64 {
    let n0 = r0 * z0;
    let mut s0 = z1 - 1.0;
    let mut s1 = if g < 0.0 { 0.0 } else { n0.hypot(z1) - 1.0 };
    let mut s = 0.0;
    for _ in 0..200 {
        s = (s0 + s1) / 2.0;
        if s == s0 || s == s1 {
            break;
        }
        let (ratio0, ratio1) = (n0 / (s + r0), z1 / (s + 1.0));
        let g = ratio0 * ratio0 + ratio1 * ratio1 - 1.0;
        if g > 0.0 {
            s0 = s;
        } else if g < 0.0 {
            s1 = s;
        } else {
            break;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn segment_distance_projects_onto_the_interior() {
        let (a, b) = (Point::new(0.0, 0.0), Point::new(10.0, 0.0));
        assert!(close(distance_to_segment(Point::new(5.0, 3.0), a, b), 3.0));
        assert!(close(distance_to_segment(Point::new(5.0, -3.0), a, b), 3.0));
        assert!(close(distance_to_segment(Point::new(7.0, 0.0), a, b), 0.0));
    }

    #[test]
    fn segment_distance_beyond_the_ends_measures_to_the_endpoint() {
        let (a, b) = (Point::new(0.0, 0.0), Point::new(10.0, 0.0));
        assert!(close(distance_to_segment(Point::new(-3.0, 4.0), a, b), 5.0));
        assert!(close(distance_to_segment(Point::new(13.0, 4.0), a, b), 5.0));
    }

    #[test]
    fn zero_length_segment_is_a_point() {
        let a = Point::new(2.0, 2.0);
        assert!(close(distance_to_segment(Point::new(5.0, 6.0), a, a), 5.0));
    }

    #[test]
    fn rect_normalizes_corners_and_negative_sizes() {
        let r = Rect::from_corners(Point::new(10.0, 2.0), Point::new(4.0, 8.0));
        assert_eq!(r.min(), Point::new(4.0, 2.0));
        assert_eq!(r.max(), Point::new(10.0, 8.0));
        assert_eq!(Rect::new(Point::new(10.0, 8.0), Size::new(-6.0, -6.0)), r);
    }

    #[test]
    fn rect_contains_is_closed() {
        let r = Rect::new(Point::ORIGIN, Size::new(10.0, 5.0));
        assert!(r.contains(Point::new(0.0, 0.0)));
        assert!(r.contains(Point::new(10.0, 5.0)));
        assert!(!r.contains(Point::new(10.001, 5.0)));
        assert!(!r.contains(Point::new(5.0, -0.001)));
    }

    #[test]
    fn rect_outline_distance_inside_and_outside() {
        let r = Rect::new(Point::ORIGIN, Size::new(10.0, 6.0));
        // Inside: nearest edge.
        assert!(close(r.distance_to_outline(Point::new(5.0, 2.0)), 2.0));
        assert!(close(r.distance_to_outline(Point::new(9.0, 3.0)), 1.0));
        // Outside, beside an edge and beyond a corner.
        assert!(close(r.distance_to_outline(Point::new(5.0, -2.0)), 2.0));
        assert!(close(r.distance_to_outline(Point::new(13.0, 10.0)), 5.0));
        // On the outline.
        assert!(close(r.distance_to_outline(Point::new(10.0, 4.0)), 0.0));
    }

    #[test]
    fn rect_expand_grows_and_clamps_when_shrinking() {
        let r = Rect::new(Point::new(2.0, 2.0), Size::new(4.0, 2.0));
        let grown = r.expand(1.0);
        assert_eq!(grown.min(), Point::new(1.0, 1.0));
        assert_eq!(grown.max(), Point::new(7.0, 5.0));
        let shrunk = r.expand(-3.0);
        assert_eq!(shrunk.size(), Size::new(0.0, 0.0));
        assert_eq!(shrunk.center(), r.center());
    }

    #[test]
    fn rect_intersects_counts_touching_edges() {
        let a = Rect::new(Point::ORIGIN, Size::new(10.0, 10.0));
        let touching = Rect::new(Point::new(10.0, 0.0), Size::new(5.0, 5.0));
        let apart = Rect::new(Point::new(10.5, 0.0), Size::new(5.0, 5.0));
        assert!(a.intersects(&touching));
        assert!(!a.intersects(&apart));
        assert_eq!(
            a.union(&apart),
            Rect::from_corners(Point::ORIGIN, Point::new(15.5, 10.0))
        );
    }

    #[test]
    fn triangle_distance_is_zero_inside_for_either_winding() {
        let clockwise = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(0.0, 10.0),
        ];
        let [a, b, c] = clockwise;
        for tri in [clockwise, [a, c, b]] {
            assert_eq!(distance_to_triangle(Point::new(2.0, 2.0), tri), 0.0);
            assert_eq!(distance_to_triangle(Point::new(5.0, 0.0), tri), 0.0);
            assert!(close(distance_to_triangle(Point::new(-3.0, 5.0), tri), 3.0));
            assert!(close(
                distance_to_triangle(Point::new(10.0, 10.0), tri),
                50.0_f32.sqrt()
            ));
        }
    }

    #[test]
    fn degenerate_triangle_measures_to_its_edges() {
        let flat = [
            Point::new(0.0, 0.0),
            Point::new(5.0, 0.0),
            Point::new(10.0, 0.0),
        ];
        assert!(close(distance_to_triangle(Point::new(5.0, 2.0), flat), 2.0));
    }

    #[test]
    fn ellipse_distance_is_measured_to_the_outline_inside_and_out() {
        // Radii 10 and 5, centered at (10, 5).
        let bounds = Rect::new(Point::ORIGIN, Size::new(20.0, 10.0));
        // On the axes: straight to the vertex.
        assert!(close(
            distance_to_ellipse(Point::new(25.0, 5.0), bounds),
            5.0
        ));
        assert!(close(
            distance_to_ellipse(Point::new(10.0, -3.0), bounds),
            3.0
        ));
        assert!(close(
            distance_to_ellipse(Point::new(10.0, 5.0), bounds),
            5.0
        ));
        // On the outline itself.
        let angle = 0.7_f32;
        let on = Point::new(10.0 + 10.0 * angle.cos(), 5.0 + 5.0 * angle.sin());
        assert!(distance_to_ellipse(on, bounds) < 1e-3);
        // Off-axis, the nearest point is not radial: check against a dense
        // sampling of the outline, from both sides.
        for p in [
            Point::new(24.0, 14.0),
            Point::new(13.0, 6.0),
            Point::new(1.0, 1.0),
        ] {
            let sampled = (0..20_000)
                .map(|i| {
                    let t = i as f32 / 20_000.0 * std::f32::consts::TAU;
                    p.distance(Point::new(10.0 + 10.0 * t.cos(), 5.0 + 5.0 * t.sin()))
                })
                .fold(f32::INFINITY, f32::min);
            assert!(
                (distance_to_ellipse(p, bounds) - sampled).abs() < 1e-2,
                "{p:?}"
            );
        }
        // A circle is exactly radial.
        let circle = Rect::new(Point::ORIGIN, Size::new(10.0, 10.0));
        assert!(close(
            distance_to_ellipse(Point::new(8.0, 9.0), circle),
            0.0
        ));
    }

    #[test]
    fn degenerate_ellipses_are_segments_and_points() {
        let flat = Rect::from_corners(Point::new(0.0, 5.0), Point::new(10.0, 5.0));
        assert!(close(distance_to_ellipse(Point::new(5.0, 8.0), flat), 3.0));
        assert!(close(distance_to_ellipse(Point::new(13.0, 9.0), flat), 5.0));
        let tall = Rect::from_corners(Point::new(2.0, 0.0), Point::new(2.0, 10.0));
        assert!(close(distance_to_ellipse(Point::new(6.0, 5.0), tall), 4.0));
        let dot = Rect::from_corners(Point::new(1.0, 1.0), Point::new(1.0, 1.0));
        assert!(close(distance_to_ellipse(Point::new(4.0, 5.0), dot), 5.0));
    }
}
