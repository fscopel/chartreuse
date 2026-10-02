//! Drawing annotations onto a canvas frame, following the model's stroke
//! geometry (see [`Style`]) and the text layout in [`font`].

use chartreuse_core::color::Rgba8;
use iced::advanced::image;
use iced::advanced::text::{LineHeight, Shaping};
use iced::widget::canvas::{self, Frame, LineCap, LineDash, LineJoin, Path, Stroke};
use iced::widget::image::FilterMethod;
use iced::{Color, Pixels, Point as CanvasPoint, Rectangle, Vector as CanvasVector};
use tiny_skia::Transform;

use super::{Obscured, Shadow, Viewport};
use crate::flatten;
use crate::font;
use crate::model::{
    highlighter, PathSegment, Point, Polyline, Rect, Shape, Size, StepMarker, Style, Text, Vector,
};
use crate::tools::HANDLE_SIZE;

/// How far right and down raster images are nudged, in canvas pixels. iced's
/// tiny-skia renderer places an image at its position truncated to a whole
/// device pixel after an f32 round trip that can land a hair short of it
/// (see `Rectangle::with_vertices`), which would shift an image drawn on the
/// device pixel grid by one; the nudge keeps it in place and is otherwise
/// invisible.
const IMAGE_NUDGE: f32 = 1.0 / 256.0;

/// Dashes for chrome outlines, in canvas pixels.
const DASH: [f32; 2] = [4.0, 3.0];

/// How far chrome outlines sit outside what they outline, in canvas pixels.
const OUTLINE_GAP: f32 = 4.0;

/// Converts a model color (straight alpha) to an iced color.
#[must_use]
pub fn color(color: Rgba8) -> Color {
    Color::from_rgba8(color.r, color.g, color.b, f32::from(color.a) / 255.0)
}

/// Where [`shape`] renders an annotation's raster parts (a highlighter's
/// layer), and how finely.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Raster {
    /// The part of the canvas that can show annotations, in canvas
    /// coordinates: the canvas's bounds clipped to the image. Nothing outside
    /// it is rasterized.
    pub visible: Rectangle,
    /// Device pixels per canvas pixel (the window's scale factor): raster
    /// parts get one pixel per device pixel.
    pub scale_factor: f32,
}

/// Draws `shape` in `style`, mapped onto the canvas by `viewport`, following
/// the rules in the [canvas docs](super#drawing), with its raster parts as
/// `raster` says. `number` is a step marker's number.
///
/// Draws nothing for a blur region, whose pixels depend on what is beneath
/// it; see [`obscured`].
pub fn shape(
    frame: &mut Frame,
    viewport: &Viewport,
    raster: Raster,
    shape: &Shape,
    style: &Style,
    number: Option<usize>,
) {
    let paint = color(style.color);
    let width = style.stroke_width.max(0.0) * viewport.scale();
    match shape {
        Shape::Line(line) => stroke_segment(
            frame,
            viewport.to_canvas(line.start),
            viewport.to_canvas(line.end),
            width,
            paint,
        ),
        Shape::Arrow(arrow) => match arrow.outline(style.stroke_width) {
            Some((start, segments)) => {
                let outline = Path::new(|path| {
                    path.move_to(viewport.to_canvas(start));
                    for segment in segments {
                        match segment {
                            PathSegment::Line(to) => path.line_to(viewport.to_canvas(to)),
                            PathSegment::Cubic(curve) => {
                                let [a, b, to] = curve.map(|p| viewport.to_canvas(p));
                                path.bezier_curve_to(a, b, to);
                            }
                        }
                    }
                    path.close();
                });
                frame.fill(&outline, paint);
            }
            None => dot(frame, viewport.to_canvas(arrow.start), width, paint),
        },
        Shape::Rectangle(rectangle) => {
            let (start, corners) = rectangle.outline(style.corner_radius);
            let start = viewport.to_canvas(start);
            if rectangle.rect.width() == 0.0 && rectangle.rect.height() == 0.0 {
                dot(frame, start, width, paint);
            } else {
                let rounded = rectangle.radius(style.corner_radius) > 0.0;
                let outline = Path::new(|path| {
                    path.move_to(start);
                    for (edge_end, curve) in corners {
                        path.line_to(viewport.to_canvas(edge_end));
                        if rounded {
                            let [a, b, to] = curve.map(|p| viewport.to_canvas(p));
                            path.bezier_curve_to(a, b, to);
                        }
                    }
                    path.close();
                });
                frame.stroke(&outline, stroke(width, paint));
            }
        }
        Shape::Ellipse(ellipse) => {
            let (start, curves) = ellipse.curves();
            let start = viewport.to_canvas(start);
            if ellipse.rect.width() == 0.0 && ellipse.rect.height() == 0.0 {
                dot(frame, start, width, paint);
            } else {
                let outline = Path::new(|path| {
                    path.move_to(start);
                    for curve in curves {
                        let [a, b, to] = curve.map(|p| viewport.to_canvas(p));
                        path.bezier_curve_to(a, b, to);
                    }
                    path.close();
                });
                frame.stroke(&outline, stroke(width, paint));
            }
        }
        Shape::Pen(pen) => polyline(frame, viewport, &pen.points, width, paint),
        Shape::Highlighter(stroke) => highlighter_stroke(frame, viewport, raster, stroke, style),
        Shape::Step(step) => {
            let diameter = 2.0 * StepMarker::radius(style.font_size) * viewport.scale();
            dot(frame, viewport.to_canvas(step.center), diameter, paint);
            if let Some(number) = number {
                let label = number.to_string();
                let origin = step.label_origin(font::measure(&label, style.font_size));
                let style = Style {
                    color: StepMarker::number_color(style.color),
                    ..*style
                };
                frame.fill_text(canvas_text(
                    &label,
                    viewport.to_canvas(origin),
                    &style,
                    viewport.scale(),
                ));
            }
        }
        Shape::Blur(_) => {}
        Shape::Text(text) => {
            text_background(frame, viewport, text.bounds(style.font_size), style);
            frame.fill_text(canvas_text(
                &text.content,
                viewport.to_canvas(text.position),
                style,
                viewport.scale(),
            ));
        }
    }
}

/// A blur region's pixels (see the [canvas docs](super#drawing)), on a fill
/// of `backdrop`.
pub fn obscured(frame: &mut Frame, viewport: &Viewport, obscured: &Obscured, backdrop: Color) {
    let rect = viewport.to_canvas_rect(Rect::from_pixels(obscured.pixels));
    frame.fill_rectangle(rect.position(), rect.size(), backdrop);
    frame.draw_image(
        Rectangle {
            x: rect.x + IMAGE_NUDGE,
            y: rect.y + IMAGE_NUDGE,
            ..rect
        },
        canvas::Image::new(obscured.image.clone())
            .filter_method(if viewport.scale() >= 1.0 {
                FilterMethod::Nearest
            } else {
                FilterMethod::Linear
            })
            .snap(true),
    );
}

/// A shape's drop shadow (see the [canvas docs](super#drawing)).
pub fn shadow(frame: &mut Frame, viewport: &Viewport, shadow: &Shadow) {
    let rect = viewport.to_canvas_rect(Rect::from_pixels(shadow.pixels));
    frame.draw_image(
        Rectangle {
            x: rect.x + IMAGE_NUDGE,
            y: rect.y + IMAGE_NUDGE,
            ..rect
        },
        canvas::Image::new(shadow.image.clone())
            .filter_method(FilterMethod::Linear)
            .snap(true),
    );
}

/// The background of text whose layout box is `bounds`, in `style` (see
/// [`Text::background`]); nothing if it is fully transparent.
pub fn text_background(frame: &mut Frame, viewport: &Viewport, bounds: Rect, style: &Style) {
    let fill = style.text_background_fill();
    if fill.a == 0 {
        return;
    }
    let rect = viewport.to_canvas_rect(Text::background(bounds, style.font_size));
    frame.fill_rectangle(rect.position(), rect.size(), color(fill));
}

/// Annotation text as canvas text at canvas position `position`, `scale`
/// canvas pixels per document unit.
pub fn canvas_text(
    content: &str,
    position: CanvasPoint,
    style: &Style,
    scale: f32,
) -> canvas::Text {
    let size = style.font_size.max(0.0) * scale;
    canvas::Text {
        content: content.to_owned(),
        position,
        color: color(style.color),
        size: Pixels(size),
        line_height: LineHeight::Absolute(Pixels(size * Text::LINE_HEIGHT)),
        font: font::FONT,
        shaping: Shaping::Advanced,
        ..canvas::Text::default()
    }
}

/// The chrome of a text edit whose layout box is at `position` and `size`: a
/// dashed `accent` outline around the box, and a caret (in the text's color)
/// `caret` from the box's top-left corner, one line tall.
pub fn text_edit(
    frame: &mut Frame,
    viewport: &Viewport,
    position: Point,
    size: Size,
    caret: Vector,
    style: &Style,
    accent: Color,
) {
    let outline = grow(
        viewport.to_canvas_rect(Rect::new(position, size)),
        OUTLINE_GAP,
    );
    frame.stroke_rectangle(
        outline.position(),
        outline.size(),
        Stroke {
            line_dash: LineDash {
                segments: &DASH,
                offset: 0,
            },
            ..Stroke::default().with_width(1.0).with_color(accent)
        },
    );

    let line = style.font_size.max(0.0) * Text::LINE_HEIGHT * viewport.scale();
    let top = viewport.to_canvas(position + caret);
    let width = (line * 0.06).clamp(1.5, 4.0);
    frame.fill_rectangle(
        CanvasPoint::new(top.x - width / 2.0, top.y),
        iced::Size::new(width, line),
        color(style.color),
    );
}

/// A selected annotation's outline: a thin `accent` rectangle just outside
/// its bounds (document units).
pub fn selection_outline(frame: &mut Frame, viewport: &Viewport, bounds: Rect, accent: Color) {
    let outline = grow(viewport.to_canvas_rect(bounds), OUTLINE_GAP);
    frame.stroke_rectangle(
        outline.position(),
        outline.size(),
        Stroke::default().with_width(1.0).with_color(accent),
    );
}

/// `color` over all of the frame but `area`.
pub fn mask(frame: &mut Frame, area: Rectangle, color: Color) {
    let size = frame.size();
    let (right, bottom) = (area.x + area.width, area.y + area.height);
    // Above, below, then left and right of the area.
    for (x0, y0, x1, y1) in [
        (0.0, 0.0, size.width, area.y),
        (0.0, bottom, size.width, size.height),
        (0.0, area.y, area.x, bottom),
        (right, area.y, size.width, bottom),
    ] {
        fill_between(frame, x0, y0, x1, y1, color);
    }
}

/// Fills the rectangle from `(x0, y0)` to `(x1, y1)`, if it is not empty.
fn fill_between(frame: &mut Frame, x0: f32, y0: f32, x1: f32, y1: f32, color: Color) {
    if x1 > x0 && y1 > y0 {
        frame.fill_rectangle(
            CanvasPoint::new(x0, y0),
            iced::Size::new(x1 - x0, y1 - y0),
            color,
        );
    }
}

/// The crop tool's chrome for `crop` (document units): the rest of `image`
/// (canvas coordinates) dimmed, an `accent` outline around the crop, and a
/// [`handle`] at each corner.
pub fn crop(frame: &mut Frame, viewport: &Viewport, image: Rectangle, crop: Rect, accent: Color) {
    let inner = viewport.to_canvas_rect(crop);
    let dim = Color::from_rgba(0.0, 0.0, 0.0, 0.55);
    let (left, top) = (image.x, image.y);
    let (right, bottom) = (image.x + image.width, image.y + image.height);
    let (crop_right, crop_bottom) = (inner.x + inner.width, inner.y + inner.height);
    // Above, below, then left and right of the crop.
    for (x0, y0, x1, y1) in [
        (left, top, right, inner.y),
        (left, crop_bottom, right, bottom),
        (left, inner.y, inner.x, crop_bottom),
        (crop_right, inner.y, right, crop_bottom),
    ] {
        fill_between(frame, x0, y0, x1, y1, dim);
    }
    frame.stroke_rectangle(
        inner.position(),
        inner.size(),
        Stroke::default().with_width(1.5).with_color(accent),
    );
    for corner in crop.corners() {
        handle(frame, viewport.to_canvas(corner), accent);
    }
}

/// A selection handle centered on `center`: a white square with an `accent`
/// border, [`HANDLE_SIZE`] across.
pub fn handle(frame: &mut Frame, center: CanvasPoint, accent: Color) {
    let half = HANDLE_SIZE / 2.0;
    let top_left = CanvasPoint::new(center.x - half, center.y - half);
    let size = iced::Size::new(HANDLE_SIZE, HANDLE_SIZE);
    frame.fill_rectangle(top_left, size, Color::WHITE);
    frame.stroke_rectangle(
        top_left,
        size,
        Stroke::default().with_width(1.5).with_color(accent),
    );
}

/// `rect` grown by `amount` on every side.
fn grow(rect: Rectangle, amount: f32) -> Rectangle {
    Rectangle::new(
        rect.position() - CanvasVector::new(amount, amount),
        iced::Size::new(rect.width + 2.0 * amount, rect.height + 2.0 * amount),
    )
}

fn stroke(width: f32, paint: Color) -> Stroke<'static> {
    Stroke::default()
        .with_width(width)
        .with_color(paint)
        .with_line_cap(LineCap::Round)
        .with_line_join(LineJoin::Round)
}

fn stroke_segment(frame: &mut Frame, a: CanvasPoint, b: CanvasPoint, width: f32, paint: Color) {
    if a == b {
        dot(frame, a, width, paint);
    } else {
        frame.stroke(&Path::line(a, b), stroke(width, paint));
    }
}

/// A stroke through document `points`, or a disc if they all coincide.
fn polyline(frame: &mut Frame, viewport: &Viewport, points: &[Point], width: f32, paint: Color) {
    let [first, rest @ ..] = points else {
        return;
    };
    if rest.iter().all(|p| p == first) {
        dot(frame, viewport.to_canvas(*first), width, paint);
    } else {
        let path = Path::new(|path| {
            path.move_to(viewport.to_canvas(*first));
            for p in rest {
                path.line_to(viewport.to_canvas(*p));
            }
        });
        frame.stroke(&path, stroke(width, paint));
    }
}

/// A highlighter stroke as an image: its flatten layer (see the
/// [canvas docs](super#drawing)) over the whole device pixels the stroke
/// covers within `raster`'s visible area, drawn at [`highlighter::alpha`]
/// opacity.
fn highlighter_stroke(
    frame: &mut Frame,
    viewport: &Viewport,
    raster: Raster,
    stroke: &Polyline,
    style: &Style,
) {
    let reach = highlighter::width(style) / 2.0;
    let bounds = viewport.to_canvas_rect(stroke.path_bounds().expand(reach));
    let Some(area) = raster.visible.intersection(&bounds) else {
        return;
    };
    let grid = DeviceBlock::covering(area, raster.scale_factor);
    let Some(layer) = flatten::highlighter_layer(
        stroke,
        style,
        grid.transform(viewport),
        grid.width,
        grid.height,
    ) else {
        return;
    };
    let straight: Vec<u8> = layer
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    let bounds = grid.bounds();
    frame.draw_image(
        Rectangle {
            x: bounds.x + IMAGE_NUDGE,
            y: bounds.y + IMAGE_NUDGE,
            ..bounds
        },
        canvas::Image::new(image::Handle::from_rgba(grid.width, grid.height, straight))
            .filter_method(FilterMethod::Nearest)
            .opacity(highlighter::alpha(style))
            .snap(true),
    );
}

/// A block of whole device pixels on the canvas: a raster image's pixels,
/// one per device pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DeviceBlock {
    /// Device pixels per canvas pixel.
    scale_factor: f32,
    /// The block's top-left corner, in device pixels from the canvas's
    /// top-left corner (whole numbers).
    x: f32,
    y: f32,
    width: u32,
    height: u32,
}

impl DeviceBlock {
    /// The device pixels, at `scale_factor` per canvas pixel, that `area`
    /// (canvas coordinates, within the canvas) touches.
    fn covering(area: Rectangle, scale_factor: f32) -> Self {
        let (x, y) = (
            (area.x * scale_factor).floor(),
            (area.y * scale_factor).floor(),
        );
        let right = ((area.x + area.width) * scale_factor).ceil();
        let bottom = ((area.y + area.height) * scale_factor).ceil();
        Self {
            scale_factor,
            x,
            y,
            // Whole, non-negative pixel counts within the canvas, so the
            // casts are exact.
            width: (right - x) as u32,
            height: (bottom - y) as u32,
        }
    }

    /// Where the block is on the canvas, in canvas coordinates.
    fn bounds(&self) -> Rectangle {
        let scale = self.scale_factor;
        Rectangle::new(
            CanvasPoint::new(self.x / scale, self.y / scale),
            iced::Size::new(self.width as f32 / scale, self.height as f32 / scale),
        )
    }

    /// Maps document coordinates onto the block's pixels, the document
    /// being on the canvas where `viewport` puts it.
    fn transform(&self, viewport: &Viewport) -> Transform {
        let scale = viewport.scale() * self.scale_factor;
        let origin = viewport.origin();
        Transform::from_row(
            scale,
            0.0,
            0.0,
            scale,
            origin.x * self.scale_factor - self.x,
            origin.y * self.scale_factor - self.y,
        )
    }
}

/// A zero-length stroke: a disc `width` across.
fn dot(frame: &mut Frame, center: CanvasPoint, width: f32, paint: Color) {
    if width > 0.0 {
        frame.fill(&Path::circle(center, width / 2.0), paint);
    }
}

#[cfg(test)]
mod tests {
    use iced::Size as CanvasSize;

    use super::*;
    use crate::canvas::View;

    #[test]
    fn a_raster_has_one_pixel_per_device_pixel() {
        let rect = |x, y, width, height| {
            Rectangle::new(CanvasPoint::new(x, y), CanvasSize::new(width, height))
        };
        // An area off the canvas pixel grid.
        let area = rect(10.25, 20.5, 30.0, 40.2);

        // At 1× it covers canvas pixels 10..41 by 20..61.
        let normal = DeviceBlock::covering(area, 1.0);
        assert_eq!((normal.width, normal.height), (31, 41));
        assert_eq!(normal.bounds(), rect(10.0, 20.0, 31.0, 41.0));

        // At 2× (Retina) it covers device pixels 20..81 by 41..122: twice the
        // pixels each way, on the half-canvas-pixel grid.
        let retina = DeviceBlock::covering(area, 2.0);
        assert_eq!((retina.width, retina.height), (61, 81));
        assert_eq!(retina.bounds(), rect(10.0, 20.5, 30.5, 40.5));

        // A 100 × 50 image fits a 200 × 100 canvas at 1:1, at (50, 25). At
        // 2×, document point (20, 5) is canvas point (70, 30), device pixel
        // (140, 60): pixel (30, 0) of a grid from device pixel (110, 60).
        // Each document unit is two pixels.
        let viewport = View::default().viewport(
            CanvasSize::new(200.0, 100.0),
            Rect::new(Point::ORIGIN, Size::new(100.0, 50.0)),
        );
        let grid = DeviceBlock::covering(rect(55.25, 30.0, 20.0, 20.0), 2.0);
        let mut points = [
            tiny_skia::Point::from_xy(20.0, 5.0),
            tiny_skia::Point::from_xy(30.0, 5.0),
        ];
        grid.transform(&viewport).map_points(&mut points);
        assert_eq!(
            points,
            [
                tiny_skia::Point::from_xy(30.0, 0.0),
                tiny_skia::Point::from_xy(50.0, 0.0),
            ]
        );
    }
}
