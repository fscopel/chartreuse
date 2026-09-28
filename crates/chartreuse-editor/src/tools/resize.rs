//! The resize tool: resizes the image, and everything on it.

use chartreuse_core::geometry::PhysicalSize;
use iced::mouse::Interaction;

use super::{Context, Pointer, Preview, Tool, ToolKind};
use crate::model::{Document, Point};

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
/// an export produces. Applying it resizes the document
/// ([`Document::resize`], one undo step): the image is resampled and the
/// annotations and crop scale with it, so the canvas shows it at its new
/// size at once.
///
/// - The fields start at the size an export produces now, in pixels.
/// - With Keep proportions on (as it starts), editing one side sets the
///   other to keep the image's aspect ratio. Switching it on sets the
///   height from the width.
/// - Switching units converts both fields.
/// - Apply (or Enter) applies the size, if both sides are valid (1 to
///   [`Document::MAX_SIDE`] pixels) and the document can take it, and is
///   done, so the editor returns to the select tool. Applying the size it
///   already has records nothing.
/// - Escape, or switching tools, leaves the size as it was.
#[derive(Debug)]
pub struct ResizeTool {
    /// The size an export produces now, which the fields start at and
    /// percentages are of.
    original: PhysicalSize,
    unit: ResizeUnit,
    width: String,
    height: String,
    proportional: bool,
    done: bool,
}

impl ResizeTool {
    /// The resize tool for `document`, showing the size it exports at.
    #[must_use]
    pub fn new(document: &Document) -> Self {
        let original = document.export_size();
        Self {
            original,
            unit: ResizeUnit::Pixels,
            width: original.width.to_string(),
            height: original.height.to_string(),
            proportional: true,
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
        let Some(target) = self.target() else {
            return;
        };
        if target == self.original || cx.document.resize(target) {
            self.done = true;
        }
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

    fn pointer(&mut self, _pointer: Pointer, _cx: &mut Context<'_>) {}

    fn escape(&mut self, _cx: &mut Context<'_>) -> bool {
        self.done = true;
        true
    }

    fn confirm(&mut self, cx: &mut Context<'_>) {
        self.apply(cx);
    }

    fn finish(&mut self, _cx: &mut Context<'_>) {}

    fn is_active(&self) -> bool {
        false
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn preview(&self) -> Preview<'_> {
        Preview::None
    }

    fn as_resize(&self) -> Option<&ResizeTool> {
        Some(self)
    }

    fn resize_input(&mut self, input: ResizeInput, cx: &mut Context<'_>) {
        self.input(input, cx);
    }

    fn cursor(&self, _document: &Document, _at: Point, _pixel: f32) -> Interaction {
        Interaction::Idle
    }
}

#[cfg(test)]
mod tests {
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
            crate::model::Size::new(200.0, 150.0),
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
}
