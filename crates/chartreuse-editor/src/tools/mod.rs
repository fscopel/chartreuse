//! The tool framework and one module per annotation tool. The Stage 3 tools
//! add their own modules here.
//!
//! # How a tool works
//!
//! The [`Editor`](crate::Editor) owns one active [`Tool`] and feeds it
//! [`Pointer`] events already converted to document coordinates, together
//! with a [`Context`]: the document to edit, the style for new annotations,
//! and the current zoom (so screen-space slop such as [`HIT_TOLERANCE`] can be
//! converted to document units). A tool is a small state machine:
//!
//! - It keeps an in-progress gesture (a drag) to itself and describes it with
//!   [`Tool::preview`], which the canvas draws on top of the document.
//! - It commits a finished gesture to the document as exactly one undo step
//!   (one [`Document::add`] or [`Document::apply`]).
//! - [`Tool::escape`] abandons the gesture (the Escape key) and
//!   [`Tool::finish`] completes it early (switching tools).
//!
//! Tools are pure logic over the model: they never touch the renderer, so they
//! are tested by feeding them events.
//!
//! # Adding a tool
//!
//! Add a module with the tool type, a [`ToolKind`] variant, and an arm in
//! [`ToolKind::create`]. Tools that draw a shape by dragging from one corner
//! or end to the other only need a [`DragShape`] (see `line.rs`), and tools
//! that draw along the pointer's path only a [`FreehandShape`] (see
//! `pen.rs`).

mod arrow;
mod blur;
mod drag;
mod ellipse;
mod freehand;
mod handles;
mod highlighter;
mod line;
mod pen;
mod rectangle;
mod select;
mod step;
mod text;

use std::fmt;

use iced::mouse::Interaction;

use crate::model::{AnnotationId, Document, Point, Shape, Style, Vector};

pub use arrow::ArrowTool;
pub use blur::BlurTool;
pub use drag::{DragShape, DragTool};
pub use ellipse::EllipseTool;
pub use freehand::{FreehandShape, FreehandTool};
pub use handles::{handle_at, handles, reshaped, Handle, HANDLE_REACH, HANDLE_SIZE};
pub use highlighter::HighlighterTool;
pub use line::LineTool;
pub use pen::PenTool;
pub use rectangle::RectangleTool;
pub use select::SelectTool;
pub use step::StepTool;
pub use text::{TextEdit, TextInput, TextTarget, TextTool};

/// How far from an annotation's drawn area a click still hits it, in canvas
/// (screen) pixels.
pub const HIT_TOLERANCE: f32 = 4.0;

/// How far the pointer must move from where it was pressed before a press
/// becomes a drag, in canvas pixels. Shorter drags are clicks and draw
/// nothing.
pub const DRAG_THRESHOLD: f32 = 3.0;

/// The kinds of tool, one per toolbar button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Select,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Pen,
    Highlighter,
    Text,
    Step,
    Blur,
}

impl ToolKind {
    /// Every kind, in toolbar order.
    pub const ALL: [Self; 10] = [
        Self::Select,
        Self::Line,
        Self::Arrow,
        Self::Rectangle,
        Self::Ellipse,
        Self::Pen,
        Self::Highlighter,
        Self::Text,
        Self::Step,
        Self::Blur,
    ];

    /// The name shown in the toolbar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Line => "Line",
            Self::Arrow => "Arrow",
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Pen => "Pen",
            Self::Highlighter => "Highlighter",
            Self::Text => "Text",
            Self::Step => "Step",
            Self::Blur => "Blur",
        }
    }

    /// The key that switches to this tool (lowercase).
    #[must_use]
    pub const fn hotkey(self) -> char {
        match self {
            Self::Select => 'v',
            Self::Line => 'l',
            Self::Arrow => 'a',
            Self::Rectangle => 'r',
            Self::Ellipse => 'e',
            Self::Pen => 'p',
            Self::Highlighter => 'h',
            Self::Text => 't',
            Self::Step => 'n',
            Self::Blur => 'b',
        }
    }

    /// The tool whose [hotkey](Self::hotkey) `key` is, in either case.
    #[must_use]
    pub fn from_hotkey(key: &str) -> Option<Self> {
        let mut chars = key.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return None;
        };
        Self::ALL
            .into_iter()
            .find(|kind| kind.hotkey() == c.to_ascii_lowercase())
    }

    /// A new tool of this kind, with nothing in progress.
    #[must_use]
    pub fn create(self) -> Box<dyn Tool> {
        match self {
            Self::Select => Box::<SelectTool>::default(),
            Self::Line => Box::<LineTool>::default(),
            Self::Arrow => Box::<ArrowTool>::default(),
            Self::Rectangle => Box::<RectangleTool>::default(),
            Self::Ellipse => Box::<EllipseTool>::default(),
            Self::Pen => Box::<PenTool>::default(),
            Self::Highlighter => Box::<HighlighterTool>::default(),
            Self::Text => Box::<TextTool>::default(),
            Self::Step => Box::<StepTool>::default(),
            Self::Blur => Box::<BlurTool>::default(),
        }
    }
}

/// A pointer (primary mouse button) event, in document coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pointer {
    /// The button went down. `clicks` counts consecutive clicks at about the
    /// same place: 1 for a single click, 2 for a double click, and so on.
    Press { at: Point, clicks: u8 },
    /// The pointer moved while the button was down.
    Move { at: Point },
    /// The button came up.
    Release { at: Point },
}

/// What a tool works with while handling an event.
#[derive(Debug)]
pub struct Context<'a> {
    pub document: &'a mut Document,
    /// The style for new annotations.
    pub style: Style,
    /// The size of one canvas pixel in document units at the current zoom.
    pub pixel: f32,
    /// Whether Shift is held (constrains shapes).
    pub shift: bool,
}

impl Context<'_> {
    /// [`HIT_TOLERANCE`] in document units.
    #[must_use]
    pub fn tolerance(&self) -> f32 {
        HIT_TOLERANCE * self.pixel
    }

    /// [`DRAG_THRESHOLD`] in document units.
    #[must_use]
    pub fn drag_threshold(&self) -> f32 {
        DRAG_THRESHOLD * self.pixel
    }

    /// [`HANDLE_REACH`] in document units.
    #[must_use]
    pub fn handle_reach(&self) -> f32 {
        HANDLE_REACH * self.pixel
    }
}

/// What the canvas draws for a tool's in-progress gesture.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview<'a> {
    /// Nothing in progress.
    None,
    /// A new annotation being drawn, in the editor's current style.
    New(Shape),
    /// Text being edited, drawn with a caret at its end. An existing text
    /// annotation being edited is hidden meanwhile.
    Text(&'a TextEdit),
    /// Annotations drawn moved by a vector (a move in progress).
    Moved(&'a [AnnotationId], Vector),
    /// An annotation drawn with a different shape (a handle drag in
    /// progress).
    Reshaped(AnnotationId, &'a Shape),
}

/// An annotation tool: a state machine turning pointer events into commands
/// (see the [module docs](self)).
pub trait Tool: fmt::Debug {
    fn kind(&self) -> ToolKind;

    /// Handles a pointer event.
    fn pointer(&mut self, pointer: Pointer, cx: &mut Context<'_>);

    /// Handles the Escape key: abandons a drag in progress, leaving the
    /// document untouched, or commits an open text edit. Returns whether
    /// anything was in progress.
    fn escape(&mut self, cx: &mut Context<'_>) -> bool;

    /// Completes the gesture in progress as if the user had finished it, for
    /// example before switching tools.
    fn finish(&mut self, cx: &mut Context<'_>);

    /// Whether a gesture or text edit is in progress.
    fn is_active(&self) -> bool;

    /// The in-progress gesture, for the canvas to draw.
    fn preview(&self) -> Preview<'_>;

    /// The open text edit, if any. While there is one, the editor sends typing
    /// to it instead of treating keys as shortcuts.
    fn text_edit(&mut self) -> Option<&mut TextEdit> {
        None
    }

    /// The mouse cursor over document point `at`.
    fn cursor(&self, document: &Document, at: Point, pixel: f32) -> Interaction;
}
