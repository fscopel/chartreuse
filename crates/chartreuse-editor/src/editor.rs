//! The editor widget: one image being annotated, with its tools, style, zoom,
//! and pan.

use std::sync::Arc;

use chartreuse_core::color::Rgba8;
use chartreuse_core::image::Image;
use iced::widget::{column, image};
use iced::{keyboard, Element};

use crate::canvas::{self, Input, InputKind, View, Viewport, Zoom, ZOOM_STEP};
use crate::font;
use crate::model::{BlurMode, Command, Document, Rect, Shape, Style, StylePatch};
use crate::toolbar::toolbar;
use crate::tools::{Context, Pointer, ResizeInput, TextInput, Tool, ToolKind};

/// Canvas pixels of Cmd-scrolling that double (or halve) the zoom.
const SCROLL_PER_DOUBLING: f32 = 200.0;

/// The editor's messages. The canvas and the toolbar produce them; the app
/// routes them back to [`Editor::update`].
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// Input from the canvas.
    Canvas(Input),
    /// Switches to a tool, first finishing whatever the current one was doing.
    Tool(ToolKind),
    /// Sets the color for new annotations and restyles the selected
    /// annotations that have one (and the text being edited), as one undo
    /// step.
    Color(Rgba8),
    /// Sets the stroke width, like [`Message::Color`].
    StrokeWidth(f32),
    /// Sets the font size, like [`Message::Color`].
    FontSize(f32),
    /// Sets how blur regions obscure what is beneath them, like
    /// [`Message::Color`].
    BlurMode(BlurMode),
    /// Uncrops the document (one undo step), dropping any crop being edited.
    ClearCrop,
    /// Input from the resize tool's toolbar controls.
    Resize(ResizeInput),
    Undo,
    Redo,
    /// Deletes the selected annotations.
    Delete,
    Zoom(ZoomChange),
    /// The window's scale factor changed (or became known): device pixels
    /// per logical pixel. The canvas sends it when the window reports a
    /// change; not every platform reports the initial one, so the owner
    /// sends that when the window opens (`iced::window::scale_factor`).
    ScaleFactor(f32),
}

/// A zoom command, applied around the canvas's center.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZoomChange {
    /// Magnify by [`ZOOM_STEP`].
    In,
    /// Shrink by [`ZOOM_STEP`].
    Out,
    /// [`Zoom::Fit`].
    Fit,
    /// One canvas pixel per image pixel.
    ActualSize,
}

/// Something the editor asks its owner to do, returned by [`Editor::update`].
/// Before returning one, the editor [finishes](Editor::finish) any gesture or
/// text edit in progress, so [`Editor::document`] is complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Save the image to a file (Cmd+S).
    Save,
    /// Copy the image to the clipboard (Cmd+C).
    Copy,
    /// Close the window (Cmd+W).
    Close,
}

/// The state of one editor: the [`Document`], the active tool, the style for
/// new annotations, and the zoom and pan.
///
/// Keep it in the app state, show [`view`](Self::view), and feed the
/// messages the view produces to [`update`](Self::update).
#[derive(Debug)]
pub struct Editor {
    document: Document,
    /// The base image for the renderer, uploaded once per base image.
    image: image::Handle,
    /// The base image `image` shows, to tell when a resize (or its undo)
    /// replaces it.
    image_of: Arc<Image>,
    tool: Box<dyn Tool>,
    style: Style,
    view: View,
    /// The canvas's size as of its latest input.
    canvas: iced::Size,
    modifiers: keyboard::Modifiers,
    /// The pointer's latest canvas position while the button is down.
    pointer: Option<iced::Point>,
    /// Device pixels per canvas pixel: the window's scale factor, which
    /// highlighters are rasterized at.
    scale_factor: f32,
    /// The canvas's blur region pixels.
    rasters: canvas::Rasters,
}

impl Editor {
    /// An editor over `image` with no annotations, fitted to the window. Also
    /// loads the annotation font (see [`font::load`]).
    #[must_use]
    pub fn new(image: Image) -> Self {
        font::load();
        let canvas = iced::Size::new(image.width() as f32, image.height() as f32);
        let document = Document::new(image);
        let image_of = Arc::clone(document.shared_base());
        Self {
            tool: ToolKind::Select.create(&document),
            document,
            image: handle(&image_of),
            image_of,
            style: Style::default(),
            view: View::default(),
            canvas,
            modifiers: keyboard::Modifiers::default(),
            pointer: None,
            scale_factor: 1.0,
            rasters: canvas::Rasters::default(),
        }
    }

    /// The document being edited. A gesture still in progress (a drag) is not
    /// in it yet.
    #[must_use]
    pub const fn document(&self) -> &Document {
        &self.document
    }

    /// The active tool.
    #[must_use]
    pub fn tool(&self) -> ToolKind {
        self.tool.kind()
    }

    /// The style for new annotations.
    #[must_use]
    pub const fn style(&self) -> Style {
        self.style
    }

    #[must_use]
    pub const fn zoom(&self) -> Zoom {
        self.view.zoom()
    }

    /// Handles a message from [`view`](Self::view).
    pub fn update(&mut self, message: Message) -> Option<Event> {
        let event = match message {
            Message::Canvas(input) => self.canvas_input(input),
            Message::Tool(kind) => {
                self.set_tool(kind);
                None
            }
            Message::Color(color) => {
                self.restyle(StylePatch {
                    color: Some(color),
                    ..StylePatch::default()
                });
                None
            }
            Message::StrokeWidth(width) => {
                self.restyle(StylePatch {
                    stroke_width: Some(width),
                    ..StylePatch::default()
                });
                None
            }
            Message::FontSize(size) => {
                self.restyle(StylePatch {
                    font_size: Some(size),
                    ..StylePatch::default()
                });
                None
            }
            Message::BlurMode(mode) => {
                self.restyle(StylePatch {
                    blur: Some(mode),
                    ..StylePatch::default()
                });
                None
            }
            Message::ClearCrop => {
                self.clear_crop();
                None
            }
            Message::Resize(input) => {
                self.with_tool(|tool, cx| tool.resize_input(input, cx));
                None
            }
            Message::Undo => {
                self.undo();
                None
            }
            Message::Redo => {
                self.redo();
                None
            }
            Message::Delete => {
                self.delete_selection();
                None
            }
            Message::Zoom(change) => {
                self.zoom_by(change);
                None
            }
            Message::ScaleFactor(scale_factor) => {
                if scale_factor.is_finite() && scale_factor > 0.0 {
                    self.scale_factor = scale_factor;
                }
                None
            }
        };
        self.measure_text();
        self.sync_image();
        event
    }

    /// Completes whatever the active tool is doing (a drag, a text edit) as
    /// if the user had finished it, so [`document`](Self::document) holds
    /// everything the user sees. Call it before exporting.
    pub fn finish(&mut self) {
        self.with_tool(|tool, cx| tool.finish(cx));
        self.measure_text();
    }

    /// The editor's widgets: the toolbar above the canvas.
    pub fn view(&self) -> Element<'_, Message> {
        column![toolbar(self), canvas::view(self)].into()
    }

    pub(crate) const fn image(&self) -> &image::Handle {
        &self.image
    }

    pub(crate) fn active_tool(&self) -> &dyn Tool {
        self.tool.as_ref()
    }

    pub(crate) const fn canvas_size(&self) -> iced::Size {
        self.canvas
    }

    pub(crate) const fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    pub(crate) const fn rasters(&self) -> &canvas::Rasters {
        &self.rasters
    }

    /// The mapping for a canvas of `size`.
    pub(crate) fn viewport(&self, size: iced::Size) -> Viewport {
        self.view.viewport(size, self.area())
    }

    /// The part of the document the canvas shows: the whole image while
    /// cropping, otherwise [`Document::cropped_bounds`].
    pub(crate) fn area(&self) -> Rect {
        if self.tool.kind() == ToolKind::Crop {
            self.document.bounds()
        } else {
            self.document.cropped_bounds()
        }
    }

    fn canvas_input(&mut self, input: Input) -> Option<Event> {
        self.canvas = input.size;
        match input.kind {
            InputKind::Resized => {}
            InputKind::Press { position, clicks } => {
                self.pointer_at(position, |at| Pointer::Press { at, clicks });
            }
            InputKind::Move { position } => self.pointer_at(position, |at| Pointer::Move { at }),
            InputKind::Release { position } => {
                self.pointer_at(position, |at| Pointer::Release { at });
                self.pointer = None;
            }
            InputKind::Scroll { position, delta } => {
                let area = self.area();
                if self.modifiers.command() {
                    let factor = (delta.y / SCROLL_PER_DOUBLING).exp2();
                    self.view.zoom_by(factor, position, self.canvas, area);
                } else {
                    self.view.pan_by(delta, self.canvas, area);
                }
            }
            InputKind::Key {
                key,
                modifiers,
                text,
            } => return self.key(&key, modifiers, text.as_deref()),
            InputKind::Modifiers(modifiers) => {
                self.modifiers = modifiers;
                // Let a drag in progress pick up Shift without waiting for the
                // pointer to move.
                if let Some(position) = self.pointer
                    && self.tool.is_active()
                {
                    self.pointer_at(position, |at| Pointer::Move { at });
                }
            }
        }
        None
    }

    fn set_tool(&mut self, kind: ToolKind) {
        if kind != self.tool.kind() {
            self.finish();
            self.tool = kind.create(&self.document);
        }
    }

    /// Starts the active tool afresh, so it picks up a document changed
    /// under it (by undo, say).
    fn restart_tool(&mut self) {
        self.tool = self.tool.kind().create(&self.document);
    }

    /// Uncrops the document as one undo step. A crop being edited is dropped
    /// rather than applied first.
    fn clear_crop(&mut self) {
        if self.tool.kind() != ToolKind::Crop {
            self.finish();
        }
        self.document.apply(Command::SetCrop(None));
        self.restart_tool();
    }

    /// Changes the style for new annotations, the open text edit's style, and
    /// (as one undo step) the selected annotations' styles.
    fn restyle(&mut self, patch: StylePatch) {
        self.style = self.style.patched(&patch);
        if let Some(edit) = self.tool.text_edit() {
            edit.restyle(&patch);
        }
        let ids = self.document.selection().iter().copied().collect();
        self.document.apply(Command::Restyle { ids, patch });
    }

    fn undo(&mut self) {
        self.finish();
        self.document.undo();
        self.restart_tool();
    }

    fn redo(&mut self) {
        self.finish();
        self.document.redo();
        self.restart_tool();
    }

    fn zoom_by(&mut self, change: ZoomChange) {
        let center = iced::Point::new(self.canvas.width / 2.0, self.canvas.height / 2.0);
        let area = self.area();
        match change {
            ZoomChange::In => self.view.zoom_by(ZOOM_STEP, center, self.canvas, area),
            ZoomChange::Out => self
                .view
                .zoom_by(1.0 / ZOOM_STEP, center, self.canvas, area),
            ZoomChange::Fit => self.view.zoom_to(Zoom::Fit, center, self.canvas, area),
            ZoomChange::ActualSize => {
                self.view
                    .zoom_to(Zoom::Scale(1.0), center, self.canvas, area);
            }
        }
    }

    /// Handles a key press:
    ///
    /// - Cmd+Z undoes and Cmd+Shift+Z redoes; Cmd+S, Cmd+C, and Cmd+W ask the
    ///   owner to save, copy, or close; Cmd+0 fits the image, Cmd+1 shows it
    ///   at actual size, and Cmd+= and Cmd+- zoom in and out. These work
    ///   while typing. (Cmd is Ctrl on Windows and Linux.)
    /// - While a text edit is open, other keys type (see [`Self::type_key`]).
    /// - Otherwise Delete or Backspace deletes the selection, Escape abandons
    ///   the gesture in progress or, if there is none, clears the selection,
    ///   Enter [confirms](Tool::confirm) the tool's job (applies a crop or a
    ///   resize), and a tool's [hotkey](ToolKind::hotkey) switches to it.
    ///
    /// Keys typed into a focused toolbar field (the resize tool's) reach
    /// the field only: the canvas never sees them.
    fn key(
        &mut self,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
        text: Option<&str>,
    ) -> Option<Event> {
        use keyboard::key::Named;
        use keyboard::Key;

        if modifiers.command() {
            let Key::Character(c) = key.as_ref() else {
                return None;
            };
            match c {
                "z" if modifiers.shift() => self.redo(),
                "z" => self.undo(),
                "s" | "c" | "w" => {
                    self.finish();
                    return Some(match c {
                        "s" => Event::Save,
                        "c" => Event::Copy,
                        _ => Event::Close,
                    });
                }
                "0" => self.zoom_by(ZoomChange::Fit),
                "1" => self.zoom_by(ZoomChange::ActualSize),
                "=" | "+" => self.zoom_by(ZoomChange::In),
                "-" => self.zoom_by(ZoomChange::Out),
                _ => {}
            }
            return None;
        }
        if self.tool.text_edit().is_some() {
            self.type_key(key, modifiers, text);
            return None;
        }
        match key.as_ref() {
            Key::Named(Named::Delete | Named::Backspace) => self.delete_selection(),
            Key::Named(Named::Escape) => {
                if !self.with_tool(|tool, cx| tool.escape(cx)) {
                    self.document.clear_selection();
                }
            }
            Key::Named(Named::Enter) => self.with_tool(|tool, cx| tool.confirm(cx)),
            Key::Character(c) if !modifiers.control() && !modifiers.alt() => {
                if let Some(kind) = ToolKind::from_hotkey(c) {
                    self.set_tool(kind);
                }
            }
            _ => {}
        }
        None
    }

    /// Typing into the open text edit.
    fn type_key(
        &mut self,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
        text: Option<&str>,
    ) {
        use keyboard::key::Named;
        use keyboard::Key;

        let input = match key.as_ref() {
            Key::Named(Named::Escape) => {
                self.with_tool(|tool, cx| tool.escape(cx));
                return;
            }
            Key::Named(Named::Enter) => TextInput::Newline,
            Key::Named(Named::Backspace) => TextInput::Backspace,
            _ if modifiers.command() || modifiers.control() => return,
            _ => match text {
                Some(text) => TextInput::Insert(text.to_owned()),
                None => return,
            },
        };
        if let Some(edit) = self.tool.text_edit() {
            edit.input(input);
        }
    }

    /// Deletes the selected annotations as one undo step.
    fn delete_selection(&mut self) {
        self.finish();
        let ids = self.document.selection().iter().copied().collect();
        self.document.apply(Command::Delete { ids });
    }

    /// Uploads the base image afresh if a resize (or an undo or redo of
    /// one) replaced it, and forgets the blur region pixels computed from
    /// the old one.
    fn sync_image(&mut self) {
        let base = self.document.shared_base();
        if !Arc::ptr_eq(base, &self.image_of) {
            self.image_of = Arc::clone(base);
            self.image = handle(&self.image_of);
            self.rasters = canvas::Rasters::default();
        }
    }

    /// Reports the laid-out size of every text annotation that has none (new,
    /// edited, restyled, or restored by undo) to the model, so hit-testing and
    /// bounds use the real layout.
    fn measure_text(&mut self) {
        let unmeasured: Vec<_> = self
            .document
            .annotations()
            .iter()
            .filter_map(|annotation| match &annotation.shape {
                Shape::Text(text) if text.measured().is_none() => Some((
                    annotation.id(),
                    font::measure(&text.content, annotation.style.font_size),
                )),
                _ => None,
            })
            .collect();
        for (id, size) in unmeasured {
            self.document.set_text_size(id, size);
        }
    }

    /// Sends the tool a pointer event at canvas `position`.
    fn pointer_at(
        &mut self,
        position: iced::Point,
        event: impl FnOnce(crate::model::Point) -> Pointer,
    ) {
        self.pointer = Some(position);
        let viewport = self.viewport(self.canvas);
        let pointer = event(viewport.to_document(position));
        self.with_tool(|tool, cx| tool.pointer(pointer, cx));
    }

    /// Runs `f` on the active tool with a context for the current state,
    /// then returns to the select tool if the tool is done.
    fn with_tool<T>(&mut self, f: impl FnOnce(&mut dyn Tool, &mut Context<'_>) -> T) -> T {
        let pixel = self.viewport(self.canvas).to_document_length(1.0);
        let mut cx = Context {
            document: &mut self.document,
            style: self.style,
            pixel,
            shift: self.modifiers.shift(),
        };
        let result = f(self.tool.as_mut(), &mut cx);
        if self.tool.is_done() {
            self.tool = ToolKind::Select.create(&self.document);
        }
        result
    }
}

/// `image` for the renderer.
fn handle(image: &Image) -> image::Handle {
    image::Handle::from_rgba(image.width(), image.height(), image.pixels().to_vec())
}

#[cfg(test)]
pub(crate) mod testing {
    //! Driving an [`Editor`] the way its canvas does.

    use chartreuse_core::color::Rgba8;
    use chartreuse_core::geometry::PhysicalSize;
    use chartreuse_core::image::Image;
    use iced::{Point, Vector};

    use super::*;

    /// A 400 × 300 image in a canvas with exactly [`canvas::MARGIN`] around
    /// it, so the fitted view is 1:1 and document point `(x, y)` is at canvas
    /// point `(x + 16, y + 16)`.
    pub const CANVAS: iced::Size = iced::Size::new(432.0, 332.0);

    pub fn editor() -> Editor {
        Editor::new(Image::filled(
            PhysicalSize::new(400, 300),
            Rgba8::from_rgb_hex(0xffffff),
        ))
    }

    /// An [`editor`] with the line tool.
    pub fn line_editor() -> Editor {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Line));
        editor
    }

    /// The canvas point over document point `(x, y)` in the fitted view.
    pub fn at(x: f32, y: f32) -> Point {
        Point::new(x + canvas::MARGIN, y + canvas::MARGIN)
    }

    pub fn input(editor: &mut Editor, kind: InputKind) -> Option<Event> {
        editor.update(Message::Canvas(Input { size: CANVAS, kind }))
    }

    pub fn press(editor: &mut Editor, position: Point, clicks: u8) {
        input(editor, InputKind::Press { position, clicks });
    }

    pub fn drag(editor: &mut Editor, from: Point, to: Point) {
        press(editor, from, 1);
        input(
            editor,
            InputKind::Move {
                position: from + (to - from) * 0.5,
            },
        );
        input(editor, InputKind::Move { position: to });
        input(editor, InputKind::Release { position: to });
    }

    pub fn click(editor: &mut Editor, position: Point) {
        press(editor, position, 1);
        input(editor, InputKind::Release { position });
    }

    pub fn scroll(editor: &mut Editor, position: Point, delta: Vector) {
        input(editor, InputKind::Scroll { position, delta });
    }

    pub fn modifiers(editor: &mut Editor, modifiers: keyboard::Modifiers) {
        input(editor, InputKind::Modifiers(modifiers));
    }

    /// A key press with `modifiers`; `text` is what the key types.
    pub fn key_with(
        editor: &mut Editor,
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        text: Option<&str>,
    ) -> Option<Event> {
        input(
            editor,
            InputKind::Key {
                key,
                modifiers,
                text: text.map(Into::into),
            },
        )
    }

    /// A named key (Enter, Escape, ...) without modifiers.
    pub fn named(editor: &mut Editor, named: keyboard::key::Named) -> Option<Event> {
        key_with(
            editor,
            keyboard::Key::Named(named),
            keyboard::Modifiers::default(),
            None,
        )
    }

    /// Types `text` one character at a time.
    pub fn type_text(editor: &mut Editor, text: &str) {
        for c in text.chars() {
            let s = c.to_string();
            key_with(
                editor,
                keyboard::Key::Character(s.as_str().into()),
                keyboard::Modifiers::default(),
                Some(&s),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use iced::Vector;

    use super::testing::*;
    use super::*;
    use crate::model::{Arrow, Line, Point, Rect, Rectangle, Shape, Text};
    use keyboard::key::Named;

    fn only_text(editor: &Editor) -> &Text {
        match editor.document().annotations() {
            [annotation] => match &annotation.shape {
                Shape::Text(text) => text,
                other => panic!("not text: {other:?}"),
            },
            other => panic!("expected one annotation, got {other:?}"),
        }
    }

    #[test]
    fn a_drag_on_the_canvas_adds_one_annotation_in_document_coordinates() {
        let mut editor = line_editor();
        drag(&mut editor, at(10.0, 20.0), at(110.0, 70.0));
        let [line] = editor.document().annotations() else {
            panic!("expected one annotation");
        };
        assert_eq!(
            line.shape,
            Shape::Line(Line {
                start: Point::new(10.0, 20.0),
                end: Point::new(110.0, 70.0),
            })
        );
        assert!(editor.document().can_undo());
    }

    #[test]
    fn zoom_and_pan_change_where_drags_land_and_the_drag_threshold() {
        let mut editor = line_editor();
        modifiers(&mut editor, keyboard::Modifiers::COMMAND);
        // Doubling the zoom around document point (0, 0) keeps it in place.
        scroll(
            &mut editor,
            at(0.0, 0.0),
            Vector::new(0.0, SCROLL_PER_DOUBLING),
        );
        modifiers(&mut editor, keyboard::Modifiers::default());
        assert_eq!(editor.zoom(), Zoom::Scale(2.0));
        scroll(&mut editor, at(0.0, 0.0), Vector::new(-40.0, -10.0));

        // Canvas (16, 16) is now document (20, 5); 1 document unit is 2 pixels.
        let origin = Point::new(20.0, 5.0);
        drag(&mut editor, at(0.0, 0.0), at(100.0, 0.0));
        let [line] = editor.document().annotations() else {
            panic!("expected one annotation");
        };
        assert_eq!(
            line.shape,
            Shape::Line(Line {
                start: origin,
                end: Point::new(70.0, 5.0),
            })
        );

        // 2 canvas pixels (under the 3-pixel threshold) is 1 document unit.
        drag(&mut editor, at(0.0, 50.0), at(2.0, 50.0));
        assert_eq!(editor.document().annotations().len(), 1);
    }

    #[test]
    fn shift_pressed_mid_drag_constrains_without_moving_the_pointer() {
        let mut editor = line_editor();
        press(&mut editor, at(0.0, 0.0), 1);
        input(
            &mut editor,
            InputKind::Move {
                position: at(100.0, 10.0),
            },
        );
        modifiers(&mut editor, keyboard::Modifiers::SHIFT);
        input(
            &mut editor,
            InputKind::Release {
                position: at(100.0, 10.0),
            },
        );
        let [line] = editor.document().annotations() else {
            panic!("expected one annotation");
        };
        assert_eq!(
            line.shape,
            Shape::Line(Line {
                start: Point::new(0.0, 0.0),
                end: Point::new(100.0, 0.0),
            })
        );
    }

    #[test]
    fn each_drag_tool_draws_its_shape() {
        let mut editor = editor();
        let (a, b) = (Point::new(10.0, 10.0), Point::new(60.0, 40.0));
        let (c, d) = (Point::new(200.0, 150.0), Point::new(100.0, 100.0));
        editor.update(Message::Tool(ToolKind::Arrow));
        drag(&mut editor, at(a.x, a.y), at(b.x, b.y));
        editor.update(Message::Tool(ToolKind::Rectangle));
        drag(&mut editor, at(c.x, c.y), at(d.x, d.y));
        let shapes: Vec<_> = editor
            .document()
            .annotations()
            .iter()
            .map(|annotation| annotation.shape.clone())
            .collect();
        assert_eq!(
            shapes,
            [
                Shape::Arrow(Arrow { start: a, end: b }),
                Shape::Rectangle(Rectangle {
                    rect: Rect::from_corners(c, d),
                }),
            ]
        );
    }

    #[test]
    fn a_drag_tool_drags_the_handles_of_the_shape_it_just_drew() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Arrow));
        drag(&mut editor, at(10.0, 10.0), at(60.0, 40.0));
        drag(&mut editor, at(61.0, 41.0), at(90.0, 40.0));
        let [arrow] = editor.document().annotations() else {
            panic!("expected one annotation");
        };
        assert_eq!(
            arrow.shape,
            Shape::Arrow(Arrow {
                start: Point::new(10.0, 10.0),
                end: Point::new(89.0, 39.0),
            })
        );
        assert!(editor.document.undo(), "the reshape");
        assert!(editor.document.undo(), "the add");
        assert!(!editor.document().can_undo());
    }

    #[test]
    fn select_move_and_delete_undo_end_to_end() {
        let mut editor = line_editor();
        drag(&mut editor, at(10.0, 50.0), at(110.0, 50.0));
        let id = editor.document().annotations()[0].id();
        editor.update(Message::Tool(ToolKind::Select));
        click(&mut editor, at(200.0, 200.0));
        assert!(editor.document().selection().is_empty());

        // Drag the line (not a handle) down by 30 and delete it.
        drag(&mut editor, at(60.0, 51.0), at(60.0, 81.0));
        assert!(editor.document().is_selected(id));
        named(&mut editor, Named::Delete);
        assert!(editor.document().annotations().is_empty());

        let line_y = |editor: &Editor| match &editor.document().get(id).unwrap().shape {
            Shape::Line(line) => line.start.y,
            other => panic!("not a line: {other:?}"),
        };
        assert!(editor.document.undo());
        assert_eq!(line_y(&editor), 80.0, "restored where it was moved");
        assert!(editor.document().is_selected(id));
        assert!(editor.document.undo());
        assert_eq!(line_y(&editor), 50.0, "the move was one step");
        assert!(editor.document.undo());
        assert!(editor.document().annotations().is_empty());
        assert!(!editor.document().can_undo());

        // Backspace deletes too; with nothing selected it records nothing.
        assert!(editor.document.redo());
        editor.document.set_selection([id]);
        named(&mut editor, Named::Backspace);
        assert!(editor.document().annotations().is_empty());
        named(&mut editor, Named::Backspace);
        assert!(editor.document.undo());
        assert_eq!(editor.document().annotations().len(), 1);
    }

    #[test]
    fn double_clicking_text_with_select_edits_it_in_place() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Text));
        click(&mut editor, at(20.0, 100.0));
        type_text(&mut editor, "Hi");
        editor.update(Message::Tool(ToolKind::Select));
        let text = only_text(&editor);
        let inside = text.position + crate::model::Vector::new(5.0, 5.0);
        press(&mut editor, at(inside.x, inside.y), 1);
        input(
            &mut editor,
            InputKind::Release {
                position: at(inside.x, inside.y),
            },
        );
        press(&mut editor, at(inside.x, inside.y), 2);
        input(
            &mut editor,
            InputKind::Release {
                position: at(inside.x, inside.y),
            },
        );
        named(&mut editor, Named::Backspace);
        type_text(&mut editor, "ello");
        named(&mut editor, Named::Escape);

        assert_eq!(only_text(&editor).content, "Hello");
        assert!(only_text(&editor).measured().is_some(), "re-measured");
        assert!(editor.document.undo());
        assert_eq!(only_text(&editor).content, "Hi");
    }

    #[test]
    fn typing_into_the_text_tool_commits_one_measured_annotation() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Text));
        click(&mut editor, at(20.0, 100.0));
        type_text(&mut editor, "Hi");
        named(&mut editor, Named::Enter);
        type_text(&mut editor, "thee");
        named(&mut editor, Named::Backspace);
        assert!(editor.document().annotations().is_empty(), "still editing");
        named(&mut editor, Named::Escape);

        let text = only_text(&editor);
        assert_eq!(text.content, "Hi\nthe");
        let size = text.measured().expect("the editor measured it");
        assert_eq!(size, font::measure("Hi\nthe", Style::default().font_size));
        assert!(editor.document.undo());
        assert!(editor.document().annotations().is_empty());
    }

    #[test]
    fn unbound_chords_do_not_type_and_blank_text_is_discarded() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Text));
        click(&mut editor, at(20.0, 100.0));
        chord(&mut editor, "k", keyboard::Modifiers::COMMAND);
        assert!(
            editor
                .tool
                .text_edit()
                .is_some_and(|edit| edit.content().is_empty()),
            "Cmd+K neither typed nor closed the edit"
        );
        type_text(&mut editor, " ");
        named(&mut editor, Named::Escape);
        assert!(editor.document().annotations().is_empty());
        assert!(!editor.document().can_undo());
    }

    #[test]
    fn switching_tools_commits_the_open_text_edit() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Text));
        click(&mut editor, at(20.0, 100.0));
        type_text(&mut editor, "Note");
        editor.update(Message::Tool(ToolKind::Line));
        assert_eq!(only_text(&editor).content, "Note");
        assert_eq!(editor.tool(), ToolKind::Line);
        // Typing no longer goes anywhere.
        type_text(&mut editor, "x");
        assert_eq!(only_text(&editor).content, "Note");
    }

    /// A key press typing `c` with `modifiers`.
    fn chord(editor: &mut Editor, c: &str, modifiers: keyboard::Modifiers) -> Option<Event> {
        key_with(
            editor,
            keyboard::Key::Character(c.into()),
            modifiers,
            Some(c),
        )
    }

    const COMMAND_SHIFT: keyboard::Modifiers =
        keyboard::Modifiers::COMMAND.union(keyboard::Modifiers::SHIFT);

    #[test]
    fn undo_and_redo_shortcuts_round_trip_an_edit() {
        let mut editor = line_editor();
        drag(&mut editor, at(10.0, 10.0), at(90.0, 10.0));
        chord(&mut editor, "z", keyboard::Modifiers::COMMAND);
        assert!(editor.document().annotations().is_empty());
        chord(&mut editor, "z", COMMAND_SHIFT);
        assert_eq!(editor.document().annotations().len(), 1);
        editor.update(Message::Undo);
        assert!(editor.document().annotations().is_empty());
        editor.update(Message::Redo);
        assert_eq!(editor.document().annotations().len(), 1);
    }

    #[test]
    fn undo_while_typing_commits_the_text_then_undoes_it() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Text));
        click(&mut editor, at(20.0, 100.0));
        type_text(&mut editor, "Hi");
        chord(&mut editor, "z", keyboard::Modifiers::COMMAND);
        assert!(editor.document().annotations().is_empty());
        assert!(editor.tool.text_edit().is_none(), "the edit is closed");
        chord(&mut editor, "z", COMMAND_SHIFT);
        assert_eq!(only_text(&editor).content, "Hi");
    }

    #[test]
    fn hotkeys_switch_tools_except_while_typing() {
        let mut editor = editor();
        for (key, kind) in [
            ("l", ToolKind::Line),
            ("A", ToolKind::Arrow),
            ("r", ToolKind::Rectangle),
            ("B", ToolKind::Blur),
            ("t", ToolKind::Text),
        ] {
            chord(&mut editor, key, keyboard::Modifiers::default());
            assert_eq!(editor.tool(), kind, "{key}");
        }
        click(&mut editor, at(20.0, 100.0));
        type_text(&mut editor, "v");
        assert_eq!(editor.tool(), ToolKind::Text, "typed, not a hotkey");
        named(&mut editor, Named::Escape);
        assert_eq!(only_text(&editor).content, "v");
        chord(&mut editor, "v", keyboard::Modifiers::default());
        assert_eq!(editor.tool(), ToolKind::Select);
    }

    #[test]
    fn escape_abandons_a_drag_and_otherwise_clears_the_selection() {
        let mut editor = line_editor();
        press(&mut editor, at(10.0, 10.0), 1);
        input(
            &mut editor,
            InputKind::Move {
                position: at(90.0, 10.0),
            },
        );
        named(&mut editor, Named::Escape);
        input(
            &mut editor,
            InputKind::Release {
                position: at(90.0, 10.0),
            },
        );
        assert!(editor.document().annotations().is_empty());
        assert!(!editor.document().can_undo());

        drag(&mut editor, at(10.0, 10.0), at(90.0, 10.0));
        assert_eq!(editor.document().selection().len(), 1);
        named(&mut editor, Named::Escape);
        assert!(editor.document().selection().is_empty());
        assert_eq!(editor.document().annotations().len(), 1);
    }

    #[test]
    fn save_and_copy_are_events_after_finishing_the_open_edit() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Text));
        click(&mut editor, at(20.0, 100.0));
        type_text(&mut editor, "Hi");
        let save = chord(&mut editor, "s", keyboard::Modifiers::COMMAND);
        assert_eq!(save, Some(Event::Save));
        assert_eq!(only_text(&editor).content, "Hi");
        let copy = chord(&mut editor, "c", keyboard::Modifiers::COMMAND);
        assert_eq!(copy, Some(Event::Copy));
        assert_eq!(
            chord(&mut editor, "c", keyboard::Modifiers::default()),
            None
        );
    }

    #[test]
    fn zoom_shortcuts_and_messages() {
        let mut editor = editor();
        input(&mut editor, InputKind::Resized);
        assert_eq!(editor.zoom(), Zoom::Fit);
        chord(&mut editor, "=", keyboard::Modifiers::COMMAND);
        assert_eq!(editor.zoom(), Zoom::Scale(ZOOM_STEP), "fit was 1:1");
        chord(&mut editor, "-", keyboard::Modifiers::COMMAND);
        assert_eq!(editor.zoom(), Zoom::Scale(1.0));
        chord(&mut editor, "0", keyboard::Modifiers::COMMAND);
        assert_eq!(editor.zoom(), Zoom::Fit);
        editor.update(Message::Zoom(ZoomChange::ActualSize));
        assert_eq!(editor.zoom(), Zoom::Scale(1.0));
    }

    const BLUE: Rgba8 = Rgba8::from_rgb_hex(0x00_7a_ff);

    #[test]
    fn style_changes_apply_to_new_annotations_and_restyle_the_selection() {
        let mut editor = line_editor();
        drag(&mut editor, at(10.0, 10.0), at(90.0, 10.0));
        editor.update(Message::Color(BLUE));
        editor.update(Message::StrokeWidth(12.0));
        let annotation = &editor.document().annotations()[0];
        assert_eq!(annotation.style.color, BLUE);
        assert_eq!(annotation.style.stroke_width, 12.0);
        assert_eq!(editor.style().color, BLUE);

        assert!(editor.document.undo(), "each change is one step");
        assert_eq!(
            editor.document().annotations()[0].style.stroke_width,
            Style::default().stroke_width
        );
        assert_eq!(editor.document().annotations()[0].style.color, BLUE);
        assert!(editor.document.undo());
        assert_eq!(
            editor.document().annotations()[0].style.color,
            Style::DEFAULT_COLOR
        );

        // With nothing selected only the style for new annotations changes.
        editor.document.clear_selection();
        editor.update(Message::Color(Style::DEFAULT_COLOR));
        assert!(editor.document().can_redo(), "nothing recorded");
        drag(&mut editor, at(10.0, 100.0), at(90.0, 100.0));
        let newest = editor.document().annotations().last().unwrap();
        assert_eq!(newest.style.color, Style::DEFAULT_COLOR);
        assert_eq!(newest.style.stroke_width, 12.0);
    }

    fn crop_rect(ax: f32, ay: f32, bx: f32, by: f32) -> Rect {
        Rect::from_corners(Point::new(ax, ay), Point::new(bx, by))
    }

    #[test]
    fn enter_applies_a_crop_and_the_canvas_then_shows_just_the_crop() {
        let mut editor = editor();
        chord(&mut editor, "c", keyboard::Modifiers::default());
        assert_eq!(editor.tool(), ToolKind::Crop);
        drag(&mut editor, at(100.0, 50.0), at(300.0, 250.0));
        assert_eq!(editor.document().crop(), None, "still editing");
        assert_eq!(editor.area(), editor.document().bounds(), "the whole image");

        named(&mut editor, Named::Enter);
        let crop = crop_rect(100.0, 50.0, 300.0, 250.0);
        assert_eq!(editor.document().crop(), Some(crop));
        assert_eq!(editor.tool(), ToolKind::Select, "done");
        assert_eq!(editor.area(), crop);

        // The fitted view now centers the 200 × 200 crop at 1:1: document
        // (100, 50) is at the canvas's (116, 66), and drawing lands there.
        let viewport = editor.viewport(CANVAS);
        assert_eq!(
            viewport.to_canvas(crop.min()),
            iced::Point::new(116.0, 66.0)
        );
        editor.update(Message::Tool(ToolKind::Line));
        drag(
            &mut editor,
            iced::Point::new(116.0, 66.0),
            iced::Point::new(216.0, 66.0),
        );
        assert_eq!(
            editor.document().annotations()[0].shape,
            Shape::Line(Line {
                start: Point::new(100.0, 50.0),
                end: Point::new(200.0, 50.0),
            })
        );
    }

    #[test]
    fn escape_cancels_a_crop_edit_and_undo_restarts_the_crop_tool() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Crop));
        drag(&mut editor, at(10.0, 10.0), at(110.0, 110.0));
        named(&mut editor, Named::Escape);
        assert_eq!(editor.tool(), ToolKind::Select);
        assert_eq!(editor.document().crop(), None);
        assert!(!editor.document().can_undo());

        // Switching tools applies the crop being edited.
        editor.update(Message::Tool(ToolKind::Crop));
        drag(&mut editor, at(10.0, 10.0), at(110.0, 110.0));
        editor.update(Message::Tool(ToolKind::Arrow));
        assert_eq!(
            editor.document().crop(),
            Some(crop_rect(10.0, 10.0, 110.0, 110.0))
        );

        // Undo while cropping applies the edit, undoes it, and the crop tool
        // shows the crop from before.
        editor.update(Message::Tool(ToolKind::Crop));
        drag(&mut editor, at(50.0, 50.0), at(250.0, 200.0));
        editor.update(Message::Undo);
        assert_eq!(
            editor.document().crop(),
            Some(crop_rect(10.0, 10.0, 110.0, 110.0))
        );
        assert_eq!(
            editor.active_tool().preview(),
            crate::tools::Preview::Crop(editor.document().crop())
        );
    }

    #[test]
    fn clearing_the_crop_is_one_undo_step_that_drops_an_edit() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Crop));
        drag(&mut editor, at(10.0, 10.0), at(110.0, 110.0));
        named(&mut editor, Named::Enter);
        editor.update(Message::Tool(ToolKind::Crop));
        drag(&mut editor, at(150.0, 150.0), at(250.0, 250.0));
        editor.update(Message::ClearCrop);
        assert_eq!(editor.document().crop(), None);
        assert_eq!(editor.tool(), ToolKind::Crop);
        assert_eq!(
            editor.active_tool().preview(),
            crate::tools::Preview::Crop(None)
        );
        editor.update(Message::Undo);
        assert_eq!(
            editor.document().crop(),
            Some(crop_rect(10.0, 10.0, 110.0, 110.0))
        );
    }

    #[test]
    fn outside_the_crop_the_pointer_selects_and_grabs_nothing() {
        let mut editor = editor();
        // A line from inside the crop-to-be out past its left edge.
        editor.update(Message::Tool(ToolKind::Line));
        drag(&mut editor, at(150.0, 150.0), at(20.0, 150.0));
        let line = editor.document().annotations()[0].clone();
        editor.update(Message::Tool(ToolKind::Crop));
        drag(&mut editor, at(100.0, 50.0), at(300.0, 250.0));
        named(&mut editor, Named::Enter);
        // The 200 × 200 crop fits at 1:1 where it was, so `at` still maps
        // document points; left of x = 100 is the backdrop.

        click(&mut editor, at(60.0, 150.0));
        assert!(editor.document().selection().is_empty(), "the hidden part");
        click(&mut editor, at(130.0, 150.0));
        assert!(editor.document().is_selected(line.id()), "the visible part");

        // From the backdrop, the hidden end's handle cannot be dragged.
        drag(&mut editor, at(20.0, 150.0), at(40.0, 170.0));
        assert_eq!(editor.document().annotations(), [line]);
        assert!(editor.document().selection().is_empty());
    }

    #[test]
    fn the_blur_mode_applies_to_new_regions_and_restyles_the_selected_one() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Blur));
        drag(&mut editor, at(10.0, 10.0), at(90.0, 60.0));
        let mode =
            |editor: &Editor, index: usize| editor.document().annotations()[index].style.blur;
        assert_eq!(mode(&editor, 0), BlurMode::Pixelate, "pixelates by default");

        editor.update(Message::BlurMode(BlurMode::Gaussian));
        assert_eq!(mode(&editor, 0), BlurMode::Gaussian);
        drag(&mut editor, at(200.0, 10.0), at(290.0, 60.0));
        assert_eq!(mode(&editor, 1), BlurMode::Gaussian);
        assert!(editor.document.undo());
        assert!(editor.document.undo(), "the restyle");
        assert_eq!(mode(&editor, 0), BlurMode::Pixelate);
    }

    #[test]
    fn style_changes_while_typing_apply_to_the_text_being_edited() {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Text));
        click(&mut editor, at(20.0, 100.0));
        editor.update(Message::FontSize(48.0));
        editor.update(Message::Color(BLUE));
        type_text(&mut editor, "Big");
        named(&mut editor, Named::Escape);
        let style = editor.document().annotations()[0].style;
        assert_eq!((style.font_size, style.color), (48.0, BLUE));
        assert_eq!(
            only_text(&editor).measured(),
            Some(font::measure("Big", 48.0))
        );
    }
}
