//! The resize tool: resizes the image, and everything on it.

use chartreuse_core::geometry::PhysicalSize;
use iced::mouse::Interaction;

use super::{Context, Pointer, Preview, Tool, ToolKind};
use crate::model::{Document, Point, Rect, Size, Vector};

/// What the resize tool's width and height are given in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResizeUnit {
    /// Pixels of the resized image.
    Pixels,
    /// Percent of the image's size now.
    Percent,
}

impl ResizeUnit {
    /// Both units, in the order the toolbar offers them.
    pub const ALL: [Self; 2] = [Self::Pixels, Self::Percent];

    /// The name the toolbar shows.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pixels => "px",
            Self::Percent => "%",
        }
    }
}

/// Input to the resize tool from its toolbar controls.
#[derive(Debug, Clone, PartialEq)]
pub enum ResizeInput {
    /// The width field was edited.
    Width(String),
    /// The height field was edited.
    Height(String),
    /// A unit was chosen.
    Unit(ResizeUnit),
    /// Keep proportions was switched on or off.
    Proportional(bool),
    /// Apply the size.
    Apply,
}

/// The resize tool: a width and a height, in pixels or percent, for the image
/// an export produces, set in the toolbar's fields or by dragging handles
/// around the image. Applying it resizes the document ([`Document::resize`],
/// one undo step): the image is resampled and the annotations and crop scale
/// with it, so the canvas shows it at its new size at once.
///
/// - The fields start at the size an export produces now, in pixels.
/// - With Keep proportions on (as it starts), editing one side sets the
///   other to keep the image's aspect ratio. Switching it on sets the
///   height from the width.
/// - Switching units converts both fields.
/// - The canvas previews the size the fields give in the image's
///   [frame](Self::frame), stretching the image and its annotations to fill
///   it, with a handle at each corner and edge of it. Dragging a handle
///   moves that corner or edge, the opposite one staying put, and sets the
///   fields to the frame's size in whole pixels. With Keep proportions on, a
///   corner moves along the frame's diagonal and an edge scales the other
///   side too, about its middle. Neither side goes below 1 or above
///   [`Document::MAX_SIDE`] pixels.
/// - Apply (or Enter) applies the size, if both sides are valid (1 to
///   [`Document::MAX_SIDE`] pixels) and the document can take it, and is
///   done, so the editor returns to the select tool. Applying the size it
///   already has records nothing.
/// - Escape abandons a drag in progress; otherwise it leaves the size as it
///   was, and is done. So does switching tools.
///
/// Nothing is resampled until the size is applied: the preview is the image
/// as it is, drawn larger or smaller.
#[derive(Debug)]
pub struct ResizeTool {
    /// The size an export produces now, which the fields start at and
    /// percentages are of.
    original: PhysicalSize,
    /// The frame's top-left corner.
    corner: Point,
    unit: ResizeUnit,
    width: String,
    height: String,
    proportional: bool,
    gesture: Option<Gesture>,
    done: bool,
}

/// A drag of one of the frame's handles, with what it changes as it was
/// when the drag started.
#[derive(Debug, Clone, PartialEq)]
struct Gesture {
    grip: Grip,
    /// From the pointer to the handle when grabbed.
    grab: Vector,
    frame: Rect,
    width: String,
    height: String,
}

/// One of the frame's handles: where it sits along each axis, -1 on the
/// frame's left (or top) edge, 0 midway, 1 on its right (or bottom) edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Grip {
    x: i8,
    y: i8,
}

impl Grip {
    /// The corners and the edges' midpoints, clockwise from the top-left.
    const ALL: [Self; 8] = [
        Self::new(-1, -1),
        Self::new(0, -1),
        Self::new(1, -1),
        Self::new(1, 0),
        Self::new(1, 1),
        Self::new(0, 1),
        Self::new(-1, 1),
        Self::new(-1, 0),
    ];

    const fn new(x: i8, y: i8) -> Self {
        Self { x, y }
    }

    /// Where the handle is on `frame`.
    fn on(self, frame: Rect) -> Point {
        let center = frame.center();
        Point::new(
            center.x + f32::from(self.x) * frame.width() / 2.0,
            center.y + f32::from(self.y) * frame.height() / 2.0,
        )
    }

    /// The cursor that shows which way the handle drags.
    const fn cursor(self) -> Interaction {
        match (self.x, self.y) {
            (0, _) => Interaction::ResizingVertically,
            (_, 0) => Interaction::ResizingHorizontally,
            (x, y) if x == y => Interaction::ResizingDiagonallyDown,
            _ => Interaction::ResizingDiagonallyUp,
        }
    }
}

/// Where the resize tool's handles are on `frame`: its corners and the
/// middles of its edges.
#[must_use]
pub fn resize_handles(frame: Rect) -> [Point; 8] {
    Grip::ALL.map(|grip| grip.on(frame))
}

impl ResizeTool {
    /// The resize tool for `document`, showing the size it exports at.
    #[must_use]
    pub fn new(document: &Document) -> Self {
        let original = document.export_size();
        Self {
            original,
            corner: document.cropped_bounds().min(),
            unit: ResizeUnit::Pixels,
            width: original.width.to_string(),
            height: original.height.to_string(),
            proportional: true,
            gesture: None,
            done: false,
        }
    }

    #[must_use]
    pub const fn unit(&self) -> ResizeUnit {
        self.unit
    }

    /// The width field's text.
    #[must_use]
    pub fn width(&self) -> &str {
        &self.width
    }

    /// The height field's text.
    #[must_use]
    pub fn height(&self) -> &str {
        &self.height
    }

    /// Whether Keep proportions is on.
    #[must_use]
    pub const fn proportional(&self) -> bool {
        self.proportional
    }

    /// The size the fields give, or `None` if either side is not a number
    /// that comes to 1 to [`Document::MAX_SIDE`] pixels.
    #[must_use]
    pub fn target(&self) -> Option<PhysicalSize> {
        let side = |text: &str, original: u32| {
            let pixels = self.pixels(text, original)?.round();
            (1.0..=Document::MAX_SIDE as f32)
                .contains(&pixels)
                .then_some(pixels as u32)
        };
        Some(PhysicalSize::new(
            side(&self.width, self.original.width)?,
            side(&self.height, self.original.height)?,
        ))
    }

    /// Where the canvas shows the image (or its crop) resized, in document
    /// coordinates (the image's as it is now): the [`target`](Self::target)
    /// size, or the size now while the fields give none, from the top-left
    /// corner the image has now or a drag left it at.
    #[must_use]
    pub fn frame(&self) -> Rect {
        let size = self.target().unwrap_or(self.original);
        Rect::new(
            self.corner,
            Size::new(size.width as f32, size.height as f32),
        )
    }

    /// Handles input from the toolbar.
    pub fn input(&mut self, input: ResizeInput, cx: &mut Context<'_>) {
        match input {
            ResizeInput::Width(text) => {
                self.width = text;
                self.keep_proportions(Side::Width);
            }
            ResizeInput::Height(text) => {
                self.height = text;
                self.keep_proportions(Side::Height);
            }
            ResizeInput::Unit(unit) => {
                let convert = |text: &str, original: u32| {
                    self.pixels(text, original)
                        .map(|pixels| field(unit, pixels, original))
                };
                let width = convert(&self.width, self.original.width);
                let height = convert(&self.height, self.original.height);
                if let Some(width) = width {
                    self.width = width;
                }
                if let Some(height) = height {
                    self.height = height;
                }
                self.unit = unit;
                self.keep_proportions(Side::Width);
            }
            ResizeInput::Proportional(on) => {
                self.proportional = on;
                self.keep_proportions(Side::Width);
            }
            ResizeInput::Apply => self.apply(cx),
        }
    }

    /// Resizes to the size, if it is valid and the document takes it, and is
    /// done.
    fn apply(&mut self, cx: &mut Context<'_>) {
        self.gesture = None;
        let Some(target) = self.target() else {
            return;
        };
        if target == self.original || cx.document.resize(target) {
            self.done = true;
        }
    }

    /// Grabs the handle nearest `at`, if one is within reach.
    fn press(&mut self, at: Point, cx: &Context<'_>) {
        let frame = self.frame();
        let reach = cx.handle_reach();
        self.gesture = Grip::ALL
            .into_iter()
            .map(|grip| (grip, grip.on(frame)))
            .map(|(grip, handle)| (grip, handle, handle.distance(at)))
            .filter(|&(_, _, distance)| distance <= reach)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(grip, handle, _)| Gesture {
                grip,
                grab: handle - at,
                frame,
                width: self.width.clone(),
                height: self.height.clone(),
            });
    }

    /// Moves the grabbed handle to follow the pointer at `at`.
    fn drag_to(&mut self, at: Point) {
        let Some(gesture) = &self.gesture else {
            return;
        };
        let (grip, from) = (gesture.grip, gesture.frame);
        let (original_width, original_height) =
            (self.original.width as f32, self.original.height as f32);
        if original_width == 0.0 || original_height == 0.0 {
            return;
        }
        let to = at + gesture.grab;
        // A side the handle is on follows the pointer; the opposite side
        // stays put.
        let length = |grip: i8, to: f32, min: f32, max: f32, length: f32| match grip {
            1 => to - min,
            -1 => max - to,
            _ => length,
        };
        let width = length(grip.x, to.x, from.min().x, from.max().x, from.width());
        let height = length(grip.y, to.y, from.min().y, from.max().y, from.height());
        let max = Document::MAX_SIDE as f32;
        let (width, height) = if self.proportional {
            let factor = match (grip.x, grip.y) {
                (0, _) => height / original_height,
                (_, 0) => width / original_width,
                // The pointer projected onto the diagonal through the
                // handle.
                _ => {
                    (width * original_width + height * original_height)
                        / (original_width * original_width + original_height * original_height)
                }
            };
            let factor = factor.clamp(
                (1.0 / original_width).max(1.0 / original_height),
                (max / original_width).min(max / original_height),
            );
            (factor * original_width, factor * original_height)
        } else {
            (width.clamp(1.0, max), height.clamp(1.0, max))
        };
        self.width = field(self.unit, width.round(), self.original.width);
        self.height = field(self.unit, height.round(), self.original.height);

        // The fields' size, placed so the opposite side stays put: centered
        // across where the handle is midway along it.
        let size = self.frame().size();
        let place = |grip: i8, min: f32, max: f32, center: f32, length: f32| match grip {
            1 => min,
            -1 => max - length,
            _ => center - length / 2.0,
        };
        let center = from.center();
        self.corner = Point::new(
            place(grip.x, from.min().x, from.max().x, center.x, size.width),
            place(grip.y, from.min().y, from.max().y, center.y, size.height),
        );
    }

    /// With Keep proportions on, sets the side other than `edited` to match
    /// it.
    fn keep_proportions(&mut self, edited: Side) {
        if !self.proportional {
            return;
        }
        match edited {
            Side::Width => {
                if let Some(height) = self.proportional_height() {
                    self.height = height;
                }
            }
            Side::Height => {
                if let Some(width) =
                    self.matching(&self.height, self.original.height, self.original.width)
                {
                    self.width = width;
                }
            }
        }
    }

    /// The height field that keeps the width's proportions, if the width is
    /// a number.
    fn proportional_height(&self) -> Option<String> {
        self.matching(&self.width, self.original.width, self.original.height)
    }

    /// The field for the side of `other` pixels that matches `text` on the
    /// side of `original` pixels.
    fn matching(&self, text: &str, original: u32, other: u32) -> Option<String> {
        if original == 0 {
            return None;
        }
        let factor = self.pixels(text, original)? / original as f32;
        Some(field(self.unit, factor * other as f32, other))
    }

    /// `text` in the current unit, as pixels of a side `original` pixels
    /// long: `None` unless it is a finite, positive number.
    fn pixels(&self, text: &str, original: u32) -> Option<f32> {
        let value: f32 = text.trim().parse().ok()?;
        if !value.is_finite() || value <= 0.0 {
            return None;
        }
        Some(match self.unit {
            ResizeUnit::Pixels => value,
            ResizeUnit::Percent => value / 100.0 * original as f32,
        })
    }
}

/// A side of the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Width,
    Height,
}

/// The field text for `pixels` of a side `original` pixels long, in `unit`:
/// whole pixels, or percent to two decimals, without trailing zeros.
fn field(unit: ResizeUnit, pixels: f32, original: u32) -> String {
    match unit {
        ResizeUnit::Pixels => format!("{}", pixels.round()),
        ResizeUnit::Percent if original == 0 => String::new(),
        ResizeUnit::Percent => {
            let percent = pixels / original as f32 * 100.0;
            format!("{}", (percent * 100.0).round() / 100.0)
        }
    }
}

impl Tool for ResizeTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Resize
    }

    fn pointer(&mut self, pointer: Pointer, cx: &mut Context<'_>) {
        match pointer {
            Pointer::Press { at, .. } => self.press(at, cx),
            Pointer::Move { at } => self.drag_to(at),
            Pointer::Release { at } => {
                self.drag_to(at);
                self.gesture = None;
            }
        }
    }

    fn escape(&mut self, _cx: &mut Context<'_>) -> bool {
        match self.gesture.take() {
            Some(gesture) => {
                self.width = gesture.width;
                self.height = gesture.height;
                self.corner = gesture.frame.min();
            }
            None => self.done = true,
        }
        true
    }

    fn confirm(&mut self, cx: &mut Context<'_>) {
        self.apply(cx);
    }

    fn finish(&mut self, _cx: &mut Context<'_>) {
        self.gesture = None;
    }

    fn is_active(&self) -> bool {
        self.gesture.is_some()
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn preview(&self) -> Preview<'_> {
        Preview::Resize(self.frame())
    }

    fn as_resize(&self) -> Option<&ResizeTool> {
        Some(self)
    }

    fn resize_input(&mut self, input: ResizeInput, cx: &mut Context<'_>) {
        self.input(input, cx);
    }

    fn cursor(&self, _document: &Document, at: Point, pixel: f32) -> Interaction {
        if let Some(gesture) = &self.gesture {
            return gesture.grip.cursor();
        }
        let frame = self.frame();
        Grip::ALL
            .into_iter()
            .map(|grip| (grip, grip.on(frame).distance(at)))
            .filter(|&(_, distance)| distance <= super::HANDLE_REACH * pixel)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(Interaction::Idle, |(grip, _)| grip.cursor())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::drag::testing::document;
    use super::*;
    use crate::model::Style;

    fn cx(document: &mut Document) -> Context<'_> {
        Context {
            document,
            style: Style::default(),
            pixel: 1.0,
            shift: false,
        }
    }

    fn send(tool: &mut ResizeTool, document: &mut Document, input: ResizeInput) {
        tool.input(input, &mut cx(document));
    }

    fn fields(tool: &ResizeTool) -> (&str, &str) {
        (tool.width(), tool.height())
    }

    fn rect(ax: f32, ay: f32, bx: f32, by: f32) -> Rect {
        Rect::from_corners(Point::new(ax, ay), Point::new(bx, by))
    }

    /// Presses at `from`, moves to `to`, and (if `release`) lets go there.
    fn drag(tool: &mut ResizeTool, document: &mut Document, from: Point, to: Point, release: bool) {
        let mut cx = cx(document);
        tool.pointer(
            Pointer::Press {
                at: from,
                clicks: 1,
            },
            &mut cx,
        );
        tool.pointer(Pointer::Move { at: to }, &mut cx);
        if release {
            tool.pointer(Pointer::Release { at: to }, &mut cx);
        }
    }

    #[test]
    fn keeping_proportions_sets_the_other_side_in_either_unit() {
        // The image is 400 × 300.
        let mut document = document();
        let mut tool = ResizeTool::new(&document);
        assert_eq!(fields(&tool), ("400", "300"));
        assert!(tool.proportional());

        send(&mut tool, &mut document, ResizeInput::Width("123".into()));
        assert_eq!(fields(&tool), ("123", "92"), "92.25 rounded");
        send(
            &mut tool,
            &mut document,
            ResizeInput::Unit(ResizeUnit::Percent),
        );
        assert_eq!(fields(&tool), ("30.75", "30.75"));
        send(&mut tool, &mut document, ResizeInput::Height("50".into()));
        assert_eq!(fields(&tool), ("50", "50"));
        assert_eq!(tool.target(), Some(PhysicalSize::new(200, 150)));

        send(&mut tool, &mut document, ResizeInput::Apply);
        assert!(tool.is_done());
        assert_eq!(document.export_size(), PhysicalSize::new(200, 150));
        assert_eq!(
            document.bounds().size(),
            Size::new(200.0, 150.0),
            "the image itself is resized"
        );
    }

    #[test]
    fn without_proportions_each_side_is_its_own() {
        let mut document = document();
        let mut tool = ResizeTool::new(&document);
        send(&mut tool, &mut document, ResizeInput::Proportional(false));
        send(&mut tool, &mut document, ResizeInput::Width("123".into()));
        send(&mut tool, &mut document, ResizeInput::Height("45".into()));
        assert_eq!(fields(&tool), ("123", "45"));
        send(&mut tool, &mut document, ResizeInput::Apply);
        assert_eq!(document.export_size(), PhysicalSize::new(123, 45));

        // Opened again, the tool starts from the new size.
        let mut tool = ResizeTool::new(&document);
        assert_eq!(fields(&tool), ("123", "45"));
        send(
            &mut tool,
            &mut document,
            ResizeInput::Unit(ResizeUnit::Percent),
        );
        assert_eq!(fields(&tool), ("100", "100"));

        assert!(document.undo());
        assert_eq!(document.export_size(), PhysicalSize::new(400, 300));
    }

    #[test]
    fn invalid_sizes_apply_nothing() {
        let mut document = document();
        let mut tool = ResizeTool::new(&document);
        for width in ["0", "-5", "abc", "", "16385"] {
            send(&mut tool, &mut document, ResizeInput::Width(width.into()));
            assert_eq!(tool.target(), None, "{width:?}");
            assert_eq!(tool.frame(), document.bounds(), "previews the size now");
            send(&mut tool, &mut document, ResizeInput::Apply);
            assert!(!tool.is_done());
        }
        assert!(!document.can_undo());

        // Nor does the size an export already has, or Escape.
        send(&mut tool, &mut document, ResizeInput::Width("400".into()));
        send(&mut tool, &mut document, ResizeInput::Apply);
        assert!(tool.is_done());
        assert!(!document.can_undo());

        let mut tool = ResizeTool::new(&document);
        send(&mut tool, &mut document, ResizeInput::Width("10".into()));
        assert!(tool.escape(&mut cx(&mut document)));
        assert!(tool.is_done());
        assert_eq!(document.export_size(), PhysicalSize::new(400, 300));
    }

    #[test]
    fn dragging_a_corner_keeps_proportions_and_the_opposite_corner() {
        // The image is 400 × 300; its frame starts on it.
        let mut document = document();
        let base = Arc::clone(document.shared_base());
        let mut tool = ResizeTool::new(&document);
        assert_eq!(tool.preview(), Preview::Resize(document.bounds()));
        let p = Point::new;

        // Grabbed a little off the bottom-right corner, which keeps the
        // offset; the pointer is projected onto the diagonal: (200 × 400 +
        // 100 × 300) / (400² + 300²) = 0.44.
        drag(
            &mut tool,
            &mut document,
            p(403.0, 302.0),
            p(203.0, 102.0),
            false,
        );
        assert!(tool.is_active());
        assert_eq!(fields(&tool), ("176", "132"));
        assert_eq!(tool.frame(), rect(0.0, 0.0, 176.0, 132.0));

        // The top-left corner, past the bottom-right one: the smallest size
        // that keeps proportions, at the bottom-right corner.
        tool.pointer(
            Pointer::Release {
                at: p(203.0, 102.0),
            },
            &mut cx(&mut document),
        );
        drag(&mut tool, &mut document, p(0.0, 0.0), p(500.0, 500.0), true);
        assert!(!tool.is_active());
        assert_eq!(fields(&tool), ("1", "1"), "1.33 × 1 rounded");
        assert_eq!(tool.frame(), rect(175.0, 131.0, 176.0, 132.0));

        // In percent, the fields give the same whole pixels.
        let mut tool = ResizeTool::new(&document);
        send(
            &mut tool,
            &mut document,
            ResizeInput::Unit(ResizeUnit::Percent),
        );
        drag(
            &mut tool,
            &mut document,
            p(0.0, 300.0),
            p(-400.0, 600.0),
            true,
        );
        assert_eq!(fields(&tool), ("200", "200"));
        assert_eq!(tool.frame(), rect(-400.0, 0.0, 400.0, 600.0));

        // Nothing is resampled or recorded until it is applied.
        assert!(Arc::ptr_eq(document.shared_base(), &base));
        assert!(!document.can_undo());
        tool.confirm(&mut cx(&mut document));
        assert!(tool.is_done());
        assert_eq!(document.export_size(), PhysicalSize::new(800, 600));
    }

    #[test]
    fn dragging_an_edge_scales_the_other_side_about_its_middle_or_not_at_all() {
        let mut document = document();
        let mut tool = ResizeTool::new(&document);
        let p = Point::new;

        // The right edge, to 200 wide: 150 high, about the edge's middle.
        drag(
            &mut tool,
            &mut document,
            p(400.0, 150.0),
            p(200.0, 0.0),
            true,
        );
        assert_eq!(fields(&tool), ("200", "150"));
        assert_eq!(tool.frame(), rect(0.0, 75.0, 200.0, 225.0));

        // Without proportions, the top edge moves alone, and stops a pixel
        // short of the bottom one.
        send(&mut tool, &mut document, ResizeInput::Proportional(false));
        drag(
            &mut tool,
            &mut document,
            p(100.0, 75.0),
            p(100.0, 25.4),
            true,
        );
        assert_eq!(fields(&tool), ("200", "200"), "199.6 rounded");
        assert_eq!(tool.frame(), rect(0.0, 25.0, 200.0, 225.0));
        drag(
            &mut tool,
            &mut document,
            p(100.0, 25.0),
            p(100.0, 900.0),
            true,
        );
        assert_eq!(fields(&tool), ("200", "1"));
        assert_eq!(tool.frame(), rect(0.0, 224.0, 200.0, 225.0));

        // Pressing away from every handle drags nothing.
        drag(
            &mut tool,
            &mut document,
            p(100.0, 100.0),
            p(10.0, 10.0),
            false,
        );
        assert!(!tool.is_active());
        assert_eq!(fields(&tool), ("200", "1"));
    }

    #[test]
    fn escape_abandons_a_drag_then_the_resize() {
        let mut document = document();
        let mut tool = ResizeTool::new(&document);
        let p = Point::new;
        drag(
            &mut tool,
            &mut document,
            p(400.0, 300.0),
            p(200.0, 150.0),
            true,
        );
        drag(
            &mut tool,
            &mut document,
            p(0.0, 0.0),
            p(-100.0, -100.0),
            false,
        );
        assert_ne!(fields(&tool), ("200", "150"));

        assert!(tool.escape(&mut cx(&mut document)));
        assert!(!tool.is_active());
        assert!(!tool.is_done());
        assert_eq!(fields(&tool), ("200", "150"), "drag undone");
        assert_eq!(tool.frame(), rect(0.0, 0.0, 200.0, 150.0));

        assert!(tool.escape(&mut cx(&mut document)));
        assert!(tool.is_done());
        assert_eq!(document.export_size(), PhysicalSize::new(400, 300));
    }
}
