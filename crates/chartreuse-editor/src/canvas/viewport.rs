//! Zoom, pan, and the mapping between canvas and document coordinates.
//!
//! Canvas coordinates are logical screen pixels relative to the canvas
//! widget's top-left corner (iced's `Point`, `Vector`, and `Size`). Document
//! coordinates are base-image pixels (see [`model`](crate::model)). The
//! user's choice of zoom and pan is a [`View`]; combined with the current
//! canvas size and the part of the document shown (the *area*: the whole
//! image, or a crop of it) it yields a [`Viewport`], the actual mapping.
//! Zooming, fitting, centering, and the pan limits all treat the area as the
//! image.

use iced::{Point as CanvasPoint, Rectangle, Size as CanvasSize, Vector as CanvasVector};

use crate::model::{Point, Rect, Size};

/// Space kept around the image when fitting it to the canvas, and how far
/// past the canvas edge a zoomed image's edge can be panned, in canvas pixels.
pub const MARGIN: f32 = 16.0;

/// The smallest zoom scale (canvas pixels per image pixel).
pub const MIN_SCALE: f32 = 0.02;

/// The largest zoom scale (canvas pixels per image pixel).
pub const MAX_SCALE: f32 = 32.0;

/// How much one zoom-in step magnifies.
pub const ZOOM_STEP: f32 = 1.25;

/// A zoom setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    /// As large as fits in the canvas with a [`MARGIN`] around it, but never
    /// magnified past one canvas pixel per image pixel. Follows the canvas as
    /// it resizes.
    Fit,
    /// A fixed number of canvas pixels per image pixel, clamped to
    /// [`MIN_SCALE`]`..=`[`MAX_SCALE`].
    Scale(f32),
}

/// The zoom and pan the user chose.
///
/// The pan is the image's offset from centered, in canvas pixels. It is kept
/// within limits that stop the image from leaving the canvas: along an axis
/// where the image fits it is zero (the image stays centered), and elsewhere
/// the image's edges can move at most [`MARGIN`] inside the canvas's edges.
/// [`Zoom::Fit`] never pans.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    zoom: Zoom,
    pan: CanvasVector,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: Zoom::Fit,
            pan: CanvasVector::ZERO,
        }
    }
}

impl View {
    #[must_use]
    pub const fn zoom(&self) -> Zoom {
        self.zoom
    }

    /// The scale `zoom` stands for, for a canvas and area of these sizes.
    #[must_use]
    pub fn scale(&self, canvas: CanvasSize, area: Size) -> f32 {
        match self.zoom {
            Zoom::Fit => fit_scale(canvas, area),
            Zoom::Scale(scale) => scale,
        }
    }

    /// The mapping this view gives for a canvas of this size showing `area`
    /// (document coordinates).
    #[must_use]
    pub fn viewport(&self, canvas: CanvasSize, area: Rect) -> Viewport {
        let size = area.size();
        let scale = self.scale(canvas, size);
        let pan = clamp_pan(self.pan, canvas, size, scale);
        let corner = centered_origin(canvas, size, scale) + pan;
        Viewport {
            scale,
            axes: CanvasVector::new(scale, scale),
            origin: CanvasPoint::new(
                corner.x - area.min().x * scale,
                corner.y - area.min().y * scale,
            ),
        }
    }

    /// Switches to `zoom`, keeping the document point under `anchor` (canvas
    /// coordinates) where it is, as far as the pan limits allow. `Fit` always
    /// centers.
    pub fn zoom_to(&mut self, zoom: Zoom, anchor: CanvasPoint, canvas: CanvasSize, area: Rect) {
        let under = self.viewport(canvas, area).to_document(anchor);
        self.zoom = match zoom {
            Zoom::Fit => Zoom::Fit,
            Zoom::Scale(scale) => Zoom::Scale(clamp_scale(scale)),
        };
        self.pan = match self.zoom {
            Zoom::Fit => CanvasVector::ZERO,
            Zoom::Scale(scale) => {
                let size = area.size();
                let centered = centered_origin(canvas, size, scale);
                // Where the area's corner goes to keep `under` at `anchor`.
                let wanted = CanvasPoint::new(
                    anchor.x - (under.x - area.min().x) * scale,
                    anchor.y - (under.y - area.min().y) * scale,
                );
                clamp_pan(wanted - centered, canvas, size, scale)
            }
        };
    }

    /// Multiplies the current scale by `factor` around `anchor` (see
    /// [`zoom_to`](Self::zoom_to)).
    pub fn zoom_by(&mut self, factor: f32, anchor: CanvasPoint, canvas: CanvasSize, area: Rect) {
        let scale = self.scale(canvas, area.size()) * factor;
        if scale.is_finite() {
            self.zoom_to(Zoom::Scale(scale), anchor, canvas, area);
        }
    }

    /// Moves the area by `delta` canvas pixels, within the pan limits.
    pub fn pan_by(&mut self, delta: CanvasVector, canvas: CanvasSize, area: Rect) {
        if let Zoom::Scale(scale) = self.zoom
            && delta.x.is_finite()
            && delta.y.is_finite()
        {
            self.pan = clamp_pan(self.pan + delta, canvas, area.size(), scale);
        }
    }
}

fn clamp_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(MIN_SCALE, MAX_SCALE)
    } else {
        1.0
    }
}

/// The [`Zoom::Fit`] scale.
fn fit_scale(canvas: CanvasSize, image: Size) -> f32 {
    let room = |canvas: f32, image: f32| {
        if image > 0.0 {
            (canvas - 2.0 * MARGIN).max(1.0) / image
        } else {
            f32::INFINITY
        }
    };
    let scale = room(canvas.width, image.width)
        .min(room(canvas.height, image.height))
        .min(1.0);
    scale.max(MIN_SCALE)
}

/// Where the image's top-left corner goes to center it at `scale`.
fn centered_origin(canvas: CanvasSize, image: Size, scale: f32) -> CanvasPoint {
    CanvasPoint::new(
        (canvas.width - image.width * scale) / 2.0,
        (canvas.height - image.height * scale) / 2.0,
    )
}

/// `pan` limited per axis to what keeps the image's edges within [`MARGIN`]
/// of the canvas's (zero where the image fits).
fn clamp_pan(pan: CanvasVector, canvas: CanvasSize, image: Size, scale: f32) -> CanvasVector {
    let axis = |pan: f32, canvas: f32, image: f32| {
        let limit = ((image * scale - canvas) / 2.0 + MARGIN).max(0.0);
        if image * scale + 2.0 * MARGIN <= canvas {
            0.0
        } else {
            pan.clamp(-limit, limit)
        }
    };
    CanvasVector::new(
        axis(pan.x, canvas.width, image.width),
        axis(pan.y, canvas.height, image.height),
    )
}

/// The mapping between canvas and document coordinates:
/// `canvas = origin + document × axes`, per axis. A view's mapping scales
/// both axes alike.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Canvas pixels per document unit of length: the geometric mean of
    /// `axes`.
    scale: f32,
    /// Canvas pixels per document unit along each axis.
    axes: CanvasVector,
    origin: CanvasPoint,
}

impl Viewport {
    /// Canvas pixels per document unit of length (stroke widths, font
    /// sizes, tolerances): the geometric mean of the [`axes`](Self::axes),
    /// which is the scale itself where they agree.
    #[must_use]
    pub const fn scale(&self) -> f32 {
        self.scale
    }

    /// Canvas pixels per document unit horizontally (`x`) and vertically
    /// (`y`).
    #[must_use]
    pub const fn axes(&self) -> CanvasVector {
        self.axes
    }

    /// Where the document's origin (the image's top-left corner) is on the
    /// canvas.
    #[must_use]
    pub const fn origin(&self) -> CanvasPoint {
        self.origin
    }

    /// The document point under a canvas point.
    #[must_use]
    pub fn to_document(&self, point: CanvasPoint) -> Point {
        Point::new(
            (point.x - self.origin.x) / self.axes.x,
            (point.y - self.origin.y) / self.axes.y,
        )
    }

    /// Where a document point is on the canvas.
    #[must_use]
    pub fn to_canvas(&self, point: Point) -> CanvasPoint {
        CanvasPoint::new(
            self.origin.x + point.x * self.axes.x,
            self.origin.y + point.y * self.axes.y,
        )
    }

    /// A canvas distance (such as a hit tolerance in pixels) in document
    /// units.
    #[must_use]
    pub fn to_document_length(&self, length: f32) -> f32 {
        length / self.scale
    }

    /// Where a document rectangle is on the canvas.
    #[must_use]
    pub fn to_canvas_rect(&self, rect: Rect) -> Rectangle {
        let min = self.to_canvas(rect.min());
        Rectangle::new(
            min,
            CanvasSize::new(rect.width() * self.axes.x, rect.height() * self.axes.y),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Point;

    const IMAGE: Size = Size::new(2000.0, 1000.0);

    /// The whole of an [`IMAGE`]-sized image.
    fn image() -> Rect {
        Rect::new(Point::ORIGIN, IMAGE)
    }

    fn close(a: CanvasPoint, b: CanvasPoint) -> bool {
        a.distance(b) < 1e-3
    }

    #[test]
    fn fit_shrinks_the_image_into_the_margins_and_centers_it() {
        let canvas = CanvasSize::new(1032.0, 800.0);
        let viewport = View::default().viewport(canvas, image());
        assert_eq!(viewport.scale(), 0.5);
        assert_eq!(viewport.origin(), CanvasPoint::new(16.0, 150.0));
    }

    #[test]
    fn fit_never_magnifies() {
        let canvas = CanvasSize::new(3000.0, 3000.0);
        let viewport = View::default().viewport(canvas, image());
        assert_eq!(viewport.scale(), 1.0);
        assert_eq!(viewport.origin(), CanvasPoint::new(500.0, 1000.0));
    }

    #[test]
    fn canvas_and_document_coordinates_round_trip() {
        let mut view = View::default();
        let canvas = CanvasSize::new(800.0, 600.0);
        view.zoom_to(
            Zoom::Scale(3.0),
            CanvasPoint::new(100.0, 50.0),
            canvas,
            image(),
        );
        let viewport = view.viewport(canvas, image());
        let point = CanvasPoint::new(123.5, 456.25);
        assert!(close(
            viewport.to_canvas(viewport.to_document(point)),
            point
        ));
        assert_eq!(viewport.to_document_length(6.0), 2.0);
    }

    #[test]
    fn an_area_off_the_origin_is_shown_as_an_image_of_its_size_would_be() {
        let canvas = CanvasSize::new(1032.0, 800.0);
        let area = Rect::new(Point::new(300.0, 200.0), IMAGE);
        let mut view = View::default();
        let whole = view.viewport(canvas, image());
        let offset = view.viewport(canvas, area);
        assert_eq!(offset.scale(), whole.scale());
        assert_eq!(offset.to_canvas(area.min()), whole.to_canvas(Point::ORIGIN));

        // Zooming keeps the point under the anchor; the pan limits apply to
        // the area's edges.
        let anchor = CanvasPoint::new(300.0, 200.0);
        let under = offset.to_document(anchor);
        view.zoom_by(4.0, anchor, canvas, area);
        assert!(close(view.viewport(canvas, area).to_canvas(under), anchor));
        view.pan_by(CanvasVector::new(1e6, 1e6), canvas, area);
        let corner = view.viewport(canvas, area).to_canvas(area.min());
        assert_eq!((corner.x, corner.y), (MARGIN, MARGIN));
    }

    #[test]
    fn zooming_keeps_the_point_under_the_anchor_in_place() {
        let mut view = View::default();
        let canvas = CanvasSize::new(800.0, 600.0);
        let anchor = CanvasPoint::new(300.0, 200.0);
        let under = view.viewport(canvas, image()).to_document(anchor);

        view.zoom_by(4.0, anchor, canvas, image());
        let viewport = view.viewport(canvas, image());
        assert!((viewport.scale() - 4.0 * 0.384).abs() < 1e-4);
        assert!(close(viewport.to_canvas(under), anchor));
    }

    #[test]
    fn pan_stops_at_the_margin() {
        let mut view = View::default();
        let canvas = CanvasSize::new(800.0, 600.0);
        view.zoom_to(Zoom::Scale(1.0), CanvasPoint::ORIGIN, canvas, image());
        view.pan_by(CanvasVector::new(1e6, -1e6), canvas, image());
        let viewport = view.viewport(canvas, image());
        // Dragged right as far as it goes: the image's left edge sits MARGIN
        // inside the canvas. Dragged up: its bottom edge does.
        assert_eq!(viewport.origin().x, MARGIN);
        assert_eq!(viewport.origin().y + IMAGE.height, canvas.height - MARGIN);
    }

    #[test]
    fn an_image_that_fits_stays_centered_along_that_axis() {
        let mut view = View::default();
        let canvas = CanvasSize::new(800.0, 2000.0);
        let center = CanvasPoint::new(400.0, 1000.0);
        view.zoom_to(Zoom::Scale(1.0), center, canvas, image());
        view.pan_by(CanvasVector::new(50.0, 50.0), canvas, image());
        let origin = view.viewport(canvas, image()).origin();
        assert_eq!(origin.y, 500.0, "fits vertically: centered");
        assert_eq!(origin.x, (800.0 - 2000.0) / 2.0 + 50.0, "wider: pans");
    }

    #[test]
    fn scales_are_clamped_and_fit_recenters() {
        let mut view = View::default();
        let canvas = CanvasSize::new(800.0, 600.0);
        view.zoom_to(Zoom::Scale(1000.0), CanvasPoint::ORIGIN, canvas, image());
        assert_eq!(view.zoom(), Zoom::Scale(MAX_SCALE));
        view.zoom_by(0.0, CanvasPoint::ORIGIN, canvas, image());
        assert_eq!(view.zoom(), Zoom::Scale(MIN_SCALE));
        view.zoom_to(Zoom::Fit, CanvasPoint::new(10.0, 10.0), canvas, image());
        assert_eq!(view, View::default());
    }
}
