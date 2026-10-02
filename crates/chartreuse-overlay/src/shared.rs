//! Pieces shared by the selection overlays ([`crate::rectangle`] and
//! [`crate::window`]): the mapping between one display's canvas and global
//! coordinates, the frozen-capture layer under the canvas, and the dim, border
//! and label drawing.

use chartreuse_core::display::DisplayInfo;
use chartreuse_core::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use chartreuse_core::image::Image;
use iced::advanced::graphics::text::Paragraph;
use iced::advanced::text::{Alignment, Paragraph as _, Text, Wrapping};
use iced::alignment::Vertical;
use iced::widget::canvas::{self, Frame, Path, Stroke};
use iced::widget::image::Handle;
use iced::widget::{image, stack};
use iced::{Color, ContentFit, Element, Fill, Point, Rectangle, Size, Vector};

/// The translucent shade over everything the user is not selecting.
pub const DIM: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.45);
/// The width of a selection border, in canvas units.
pub(crate) const BORDER_WIDTH: f32 = 2.0;
/// A pill's text color; pills are accent-colored, and both flavor accents are
/// bright.
const PILL_TEXT: Color = Color::BLACK;

/// How a pill's text is set: its size and the padding around it, in canvas
/// units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PillStyle {
    pub(crate) text_size: f32,
    padding: f32,
}

/// The labels by a selection: its output size, a window's name.
pub(crate) const LABEL: PillStyle = PillStyle {
    text_size: 13.0,
    padding: 5.0,
};

/// Builds the iced image handle for one display's frozen capture, drawn under an
/// overlay's canvas. Consumes the image without copying its pixels; clone the
/// capture first if it is also needed for cropping.
#[must_use]
pub fn frozen_image(image: Image) -> Handle {
    let size = image.size();
    Handle::from_rgba(size.width, size.height, image.into_pixels())
}

/// Maps between one display's canvas and global logical desktop coordinates.
///
/// The canvas is stretched over the display's logical bounds. In an overlay
/// window, which is placed on the display and sized to it, one canvas unit is
/// one logical point and the window origin is the display's logical origin; a
/// smaller window (the development harnesses) scales uniformly.
///
/// Positions come in two frames: **window** positions, as reported by mouse
/// events and [`mouse::Cursor`](iced::mouse::Cursor) (relative to the window, so
/// they may lie outside the canvas bounds or the window itself during a drag),
/// and **frame** positions, relative to the canvas's top-left corner, used for
/// drawing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    display: LogicalRect,
    canvas: Rectangle,
}

impl Projection {
    /// The projection for `display` drawn into a canvas occupying `canvas` (its
    /// layout bounds in window coordinates).
    #[must_use]
    pub const fn new(display: &DisplayInfo, canvas: Rectangle) -> Self {
        Self {
            display: display.logical_bounds,
            canvas,
        }
    }

    /// Canvas units per logical point along each axis (1 if either extent is
    /// zero, so conversions stay finite).
    fn scale(&self) -> (f64, f64) {
        let ratio = |canvas: f32, logical: f64| {
            if canvas > 0.0 && logical > 0.0 {
                f64::from(canvas) / logical
            } else {
                1.0
            }
        };
        (
            ratio(self.canvas.width, self.display.size.width),
            ratio(self.canvas.height, self.display.size.height),
        )
    }

    /// Converts a window position to global logical coordinates.
    #[must_use]
    pub fn to_global(&self, window: Point) -> LogicalPoint {
        let (sx, sy) = self.scale();
        LogicalPoint::new(
            self.display.origin.x + f64::from(window.x - self.canvas.x) / sx,
            self.display.origin.y + f64::from(window.y - self.canvas.y) / sy,
        )
    }

    /// Converts a global logical point to a frame position.
    #[must_use]
    pub fn to_frame(&self, point: LogicalPoint) -> Point {
        let (sx, sy) = self.scale();
        Point::new(
            ((point.x - self.display.origin.x) * sx) as f32,
            ((point.y - self.display.origin.y) * sy) as f32,
        )
    }

    /// Converts a global logical rectangle to frame coordinates (not clipped to
    /// the canvas).
    #[must_use]
    pub fn rect_to_frame(&self, rect: &LogicalRect) -> Rectangle {
        let (sx, sy) = self.scale();
        Rectangle::new(
            self.to_frame(rect.origin),
            Size::new(
                (rect.size.width * sx) as f32,
                (rect.size.height * sy) as f32,
            ),
        )
    }

    /// Converts a size in canvas units to logical points.
    #[must_use]
    pub fn size_to_logical(&self, size: Size) -> LogicalSize {
        let (sx, sy) = self.scale();
        LogicalSize::new(f64::from(size.width) / sx, f64::from(size.height) / sy)
    }
}

/// Per-canvas pointer memory: the `Program::State` of the overlay canvases.
#[derive(Debug, Default)]
pub struct PointerState {
    /// The last window position the pointer was seen at.
    pub(crate) last: Option<Point>,
    /// True between a press on this canvas and the matching release. Kept
    /// locally because a release can arrive in the same event batch as its
    /// press, before the app's selection (and the canvas program) is rebuilt.
    pub(crate) pressed: bool,
}

/// The frozen capture stretched over the whole window, with `canvas` on top.
///
/// The renderer draws a layer's images above its shapes, so the capture gets a
/// layer of its own underneath the canvas.
pub(crate) fn over_capture<'a, Message: 'a>(
    capture: &'a Handle,
    canvas: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let capture = image(capture)
        .width(Fill)
        .height(Fill)
        .content_fit(ContentFit::Fill);
    stack![capture, canvas.into()].into()
}

/// Dims all of `area` except `hole` (which may extend past `area`, or lie
/// entirely outside it).
pub(crate) fn fill_dim(frame: &mut Frame, area: Rectangle, hole: Option<Rectangle>) {
    for region in dim_regions(area, hole) {
        if region.width > 0.0 && region.height > 0.0 {
            frame.fill_rectangle(region.position(), region.size(), DIM);
        }
    }
}

/// The parts of `area` outside `hole` (clipped to `area`): top, bottom, left and
/// right bands. Bands with nothing to cover are empty.
fn dim_regions(area: Rectangle, hole: Option<Rectangle>) -> [Rectangle; 4] {
    let empty = Rectangle::default();
    let Some(hole) = hole.and_then(|hole| hole.intersection(&area)) else {
        return [area, empty, empty, empty];
    };
    let (right, bottom) = (area.x + area.width, area.y + area.height);
    let (hole_right, hole_bottom) = (hole.x + hole.width, hole.y + hole.height);
    [
        Rectangle::new(area.position(), Size::new(area.width, hole.y - area.y)),
        Rectangle::new(
            Point::new(area.x, hole_bottom),
            Size::new(area.width, bottom - hole_bottom),
        ),
        Rectangle::new(
            Point::new(area.x, hole.y),
            Size::new(hole.x - area.x, hole.height),
        ),
        Rectangle::new(
            Point::new(hole_right, hole.y),
            Size::new(right - hole_right, hole.height),
        ),
    ]
}

/// Strokes a [`BORDER_WIDTH`] border in `color` just outside `hole`, so it never
/// covers what is inside.
pub(crate) fn border_around(frame: &mut Frame, hole: Rectangle, color: Color) {
    let inset = BORDER_WIDTH / 2.0;
    frame.stroke_rectangle(
        hole.position() - Vector::new(inset, inset),
        hole.size().expand(Size::new(BORDER_WIDTH, BORDER_WIDTH)),
        Stroke::default().with_color(color).with_width(BORDER_WIDTH),
    );
}

/// Strokes a [`BORDER_WIDTH`] border in `color` just inside `rect`, so it stays
/// visible when `rect` fills the canvas.
pub(crate) fn border_within(frame: &mut Frame, rect: Rectangle, color: Color) {
    let inset = BORDER_WIDTH / 2.0;
    frame.stroke_rectangle(
        rect.position() + Vector::new(inset, inset),
        Size::new(
            (rect.width - BORDER_WIDTH).max(0.0),
            (rect.height - BORDER_WIDTH).max(0.0),
        ),
        Stroke::default().with_color(color).with_width(BORDER_WIDTH),
    );
}

/// The size of `content` set in `style` (on one line), measured with the same
/// font and shaping [`draw_pill`] draws it with.
pub(crate) fn measure_text(content: &str, style: PillStyle) -> Size {
    let text = canvas::Text::default();
    Paragraph::with_text(Text {
        content,
        bounds: Size::INFINITE,
        size: style.text_size.into(),
        line_height: text.line_height,
        font: text.font,
        align_x: Alignment::Center,
        align_y: Vertical::Center,
        shaping: text.shaping,
        wrapping: Wrapping::None,
    })
    .min_bounds()
}

/// The size of the pill holding text in `style` that measures `text`.
pub(crate) fn pill_size(text: Size, style: PillStyle) -> Size {
    text.expand(Size::new(2.0 * style.padding, 2.0 * style.padding))
}

/// Draws `content` in `style` centered on an `accent`-colored pill of size
/// `pill` (see [`pill_size`]) at `top_left`, in frame coordinates.
pub(crate) fn draw_pill(
    frame: &mut Frame,
    top_left: Point,
    pill: Size,
    content: String,
    style: PillStyle,
    accent: Color,
) {
    frame.fill(
        &Path::rounded_rectangle(top_left, pill, (pill.height / 2.0).into()),
        accent,
    );
    frame.fill_text(canvas::Text {
        content,
        position: top_left + Vector::new(pill.width / 2.0, pill.height / 2.0),
        color: PILL_TEXT,
        size: style.text_size.into(),
        align_x: Alignment::Center,
        align_y: Vertical::Center,
        ..canvas::Text::default()
    });
}

#[cfg(test)]
mod tests {
    use chartreuse_core::display::{DisplayId, DisplayLayout};
    use chartreuse_platform::fake::default_displays;

    use super::*;

    fn layout() -> DisplayLayout {
        DisplayLayout::new(default_displays()).expect("the fake displays form a layout")
    }

    fn display(id: u64) -> DisplayInfo {
        layout()
            .get(DisplayId(id))
            .cloned()
            .expect("a fake display")
    }

    /// A canvas filling a window sized to the display, as in an overlay.
    fn overlay_bounds(display: &DisplayInfo) -> Rectangle {
        let size = display.logical_bounds.size;
        Rectangle::new(
            Point::ORIGIN,
            Size::new(size.width as f32, size.height as f32),
        )
    }

    fn pt(x: f64, y: f64) -> LogicalPoint {
        LogicalPoint::new(x, y)
    }

    #[test]
    fn window_positions_map_to_global_coordinates_per_display() {
        // Primary 2× at the origin, external 1× at (-1920, -400), portrait 1.5×
        // at (1512, 100). The scale factor plays no part: windows are sized in
        // logical points.
        for (id, window, global) in [
            (1, Point::new(10.0, 20.0), pt(10.0, 20.0)),
            (2, Point::new(0.0, 0.0), pt(-1920.0, -400.0)),
            (2, Point::new(1919.5, 400.0), pt(-0.5, 0.0)),
            (3, Point::new(0.0, 0.0), pt(1512.0, 100.0)),
            (3, Point::new(88.5, 1000.25), pt(1600.5, 1100.25)),
        ] {
            let display = display(id);
            let projection = Projection::new(&display, overlay_bounds(&display));
            assert_eq!(projection.to_global(window), global, "display {id}");
            assert_eq!(projection.to_frame(global), window, "display {id}");
        }
    }

    #[test]
    fn positions_outside_the_window_map_onto_neighbouring_displays() {
        // A drag that starts on the external display and continues to the right
        // of its window lands on the primary display.
        let external = display(2);
        let projection = Projection::new(&external, overlay_bounds(&external));
        let global = projection.to_global(Point::new(2020.0, 450.0));
        assert_eq!(global, pt(100.0, 50.0));
        assert_eq!(
            layout().display_at(global).map(|d| d.id),
            Some(DisplayId(1))
        );
        // ...and above it, at negative window coordinates.
        assert_eq!(
            projection.to_global(Point::new(-80.0, -50.0)),
            pt(-2000.0, -450.0)
        );
    }

    #[test]
    fn a_scaled_or_offset_canvas_is_stretched_over_the_display() {
        // The harness shows the portrait display at a quarter size, and a canvas
        // need not start at the window origin.
        let portrait = display(3);
        let bounds = Rectangle::new(Point::new(10.0, 30.0), Size::new(200.0, 320.0));
        let projection = Projection::new(&portrait, bounds);
        assert_eq!(
            projection.to_global(Point::new(10.0, 30.0)),
            pt(1512.0, 100.0)
        );
        assert_eq!(
            projection.to_global(Point::new(60.0, 55.0)),
            pt(1712.0, 200.0)
        );
        assert_eq!(
            projection.rect_to_frame(&LogicalRect::new(1612.0, 500.0, 400.0, 80.0)),
            Rectangle::new(Point::new(25.0, 100.0), Size::new(100.0, 20.0))
        );
        assert_eq!(
            projection.size_to_logical(Size::new(10.0, 5.0)),
            LogicalSize::new(40.0, 20.0)
        );
    }

    #[test]
    fn a_rectangle_across_displays_projects_onto_each_canvas() {
        // From the external display into the primary: each canvas sees its slice,
        // extending past its edge.
        let rect = LogicalRect::new(-300.0, -100.0, 700.0, 400.0);
        let external = display(2);
        let on_external =
            Projection::new(&external, overlay_bounds(&external)).rect_to_frame(&rect);
        assert_eq!(
            on_external,
            Rectangle::new(Point::new(1620.0, 300.0), Size::new(700.0, 400.0))
        );
        let primary = display(1);
        let on_primary = Projection::new(&primary, overlay_bounds(&primary)).rect_to_frame(&rect);
        assert_eq!(
            on_primary,
            Rectangle::new(Point::new(-300.0, -100.0), Size::new(700.0, 400.0))
        );
    }

    #[test]
    fn dim_covers_everything_but_the_visible_part_of_the_hole() {
        let area = Rectangle::new(Point::ORIGIN, Size::new(100.0, 80.0));
        assert_eq!(dim_regions(area, None)[0], area);

        // A hole running off the right edge of the canvas.
        let hole = Rectangle::new(Point::new(60.0, 20.0), Size::new(100.0, 30.0));
        let regions = dim_regions(area, Some(hole));
        let covered: f32 = regions.iter().map(|r| r.width * r.height).sum();
        assert_eq!(covered, 100.0 * 80.0 - 40.0 * 30.0);
        assert_eq!(regions[3].width, 0.0, "nothing to dim right of the hole");
        for region in regions {
            assert!(
                region.intersection(&hole).is_none(),
                "{region:?} dims the hole"
            );
        }

        // A hole entirely on another display dims the whole canvas.
        let elsewhere = Rectangle::new(Point::new(-500.0, 0.0), Size::new(50.0, 50.0));
        assert_eq!(dim_regions(area, Some(elsewhere))[0], area);
    }
}
