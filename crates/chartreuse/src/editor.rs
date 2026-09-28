//! Editor windows (`WindowKind::Editor`): one per capture or opened image, each
//! showing its own [`Editor`] from `chartreuse-editor`. The editor widget's
//! wiring in [`view`] is owned by track 2E; the rest by integration task I4.
//!
//! # Opening
//!
//! [`Message::Open`] opens a new editor window over an image (a capture, or an
//! image from the clipboard or a file). The window is titled with the time it
//! opened and sized by [`window_size`]: big enough for the image at one screen
//! point per pixel, within 80% of the primary display. The app is a menu bar
//! app with no Dock icon, so the window is focused explicitly to bring it
//! forward. Once it is open, the editor is told the window's scale factor
//! (which not every platform reports by itself), so it draws highlighters at
//! the display's resolution.
//!
//! # The status bar
//!
//! The bar along the bottom of the window shows the size, in pixels, of the
//! image an export produces ([`Document::export_size`]: the crop's, if the
//! image is cropped), and the buttons that export it:
//!
//! | Button          | Shortcut  | Does                                          |
//! |-----------------|-----------|-----------------------------------------------|
//! | Copy            | Cmd+C     | copies the image to the clipboard             |
//! | Save…           | Cmd+S     | asks where to save the image, then saves it   |
//! | Copy & Close    |           | copies, then closes the window                |
//! | Save & Close…   |           | saves, then closes the window                 |
//!
//! (Ctrl instead of Cmd on Windows and Linux.) Each finishes the gesture or
//! text edit in progress ([`Editor::finish`]) and flattens the annotations into
//! the image off the main thread ([`flatten`]); `export` then saves or copies it
//! (see its docs for where files go). A close-after button closes the window
//! only once the export succeeds: a cancelled save dialog or a failure (reported
//! to the user) leaves it open.
//!
//! Closing an editor window discards it, annotations included, without asking:
//! v1 has no unsaved-changes confirmation.
//!
//! [`Document::export_size`]: chartreuse_editor::model::Document::export_size

use std::collections::HashMap;
use std::sync::Arc;

use chartreuse_core::flavor;
use chartreuse_core::geometry::{LogicalSize, PhysicalSize};
use chartreuse_core::image::Image;
use chartreuse_editor::canvas::MARGIN;
use chartreuse_editor::flatten::flatten;
use chartreuse_editor::{Editor, Event};
use chrono::NaiveDateTime;
use iced::widget::{button, column, container, row, space, text, tooltip};
use iced::{window, Alignment, Element, Length, Size, Subscription, Task};

use crate::alert::{self, Notice};
use crate::app::{App, Message as AppMessage};
use crate::export::{self, Request, Target};
use crate::windows::WindowKind;

/// The smallest editor window.
const MIN_SIZE: Size = Size::new(720.0, 480.0);

/// The largest share of the primary display's width and height an editor
/// window opens at.
const MAX_SHARE: f32 = 0.8;

/// The largest size an editor window opens at when the primary display is
/// unknown.
const FALLBACK_MAX_SIZE: Size = Size::new(1280.0, 800.0);

/// Room around the image in an editor window: the canvas margin on every side,
/// plus about the toolbar above it and the export buttons below.
const CHROME: Size = Size::new(2.0 * MARGIN, 2.0 * MARGIN + 100.0);

/// This feature's part of the app state ([`App::editor`]).
#[derive(Debug, Default)]
pub struct State {
    windows: HashMap<window::Id, Session>,
}

impl State {
    /// The editor in `window`, if it is an open editor window.
    #[must_use]
    pub fn get(&self, window: window::Id) -> Option<&Editor> {
        self.windows.get(&window).map(|session| &session.editor)
    }
}

/// One editor window.
#[derive(Debug)]
struct Session {
    editor: Editor,
    /// The local time the window opened: when its image was taken, for the
    /// saved file's name.
    opened: NaiveDateTime,
    title: String,
}

/// An export button or shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    pub target: Target,
    /// Close the window once the export succeeds.
    pub close: bool,
}

/// This feature's messages ([`AppMessage::Editor`]).
#[derive(Debug, Clone)]
pub enum Message {
    /// Open a new editor window over the image.
    Open(Arc<Image>),
    /// A message from the editor widget in a window.
    Widget(window::Id, chartreuse_editor::Message),
    /// Export the image in a window: an export button.
    Export(window::Id, Action),
    /// An image was flattened for export to the target, or failed to be.
    Flattened(Target, Result<Request, chartreuse_core::Error>),
}

/// The size of a new editor window for an image of `image` pixels on a
/// primary display of `display` points: room for the image at one point per
/// pixel, the toolbar, and the export buttons, but at most 80% of the display
/// in each dimension (1280 × 800 points if it is unknown), and never smaller
/// than 720 × 480 points.
#[must_use]
pub fn window_size(image: PhysicalSize, display: Option<LogicalSize>) -> Size {
    let max = display.map_or(FALLBACK_MAX_SIZE, |display| {
        Size::new(
            display.width as f32 * MAX_SHARE,
            display.height as f32 * MAX_SHARE,
        )
    });
    let wanted = Size::new(
        image.width as f32 + CHROME.width,
        image.height as f32 + CHROME.height,
    );
    Size::new(
        wanted.width.min(max.width).max(MIN_SIZE.width),
        wanted.height.min(max.height).max(MIN_SIZE.height),
    )
}

pub fn boot(_app: &mut App) -> Task<AppMessage> {
    Task::none()
}

pub fn update(app: &mut App, message: Message) -> Task<AppMessage> {
    match message {
        Message::Open(image) => open(app, Arc::unwrap_or_clone(image)),
        Message::Widget(window, message) => {
            let Some(session) = app.editor.windows.get_mut(&window) else {
                return Task::none();
            };
            let target = match session.editor.update(message) {
                Some(Event::Save) => Target::Save,
                Some(Event::Copy) => Target::Copy,
                None => return Task::none(),
            };
            export(
                app,
                window,
                Action {
                    target,
                    close: false,
                },
            )
        }
        Message::Export(window, action) => export(app, window, action),
        Message::Flattened(target, Ok(request)) => {
            Task::done(AppMessage::Export(export::Message::Export(target, request)))
        }
        Message::Flattened(target, Err(error)) => {
            alert::report_error(app, Notice::from_error(target.failure(), &error))
        }
    }
}

pub fn subscription(_app: &App) -> Subscription<AppMessage> {
    Subscription::none()
}

/// The title of editor window `window`: the app's name and when it opened.
pub fn title(app: &App, window: window::Id) -> String {
    app.editor.windows.get(&window).map_or_else(
        || WindowKind::Editor.title(),
        |session| session.title.clone(),
    )
}

/// The editor window's contents: its [`Editor`] widget above the status bar.
pub fn view(app: &App, window: window::Id) -> Element<'_, AppMessage> {
    match app.editor.get(window) {
        Some(editor) => column![
            container(
                editor
                    .view()
                    .map(move |message| AppMessage::Editor(Message::Widget(window, message)))
            )
            .height(Length::Fill),
            status_bar(editor, window),
        ]
        .into(),
        None => space().into(),
    }
}

pub fn window_closed(app: &mut App, window: window::Id) -> Task<AppMessage> {
    app.editor.windows.remove(&window);
    Task::none()
}

/// Opens a new editor window over `image`, focuses it, and tells the editor
/// its scale factor.
fn open(app: &mut App, image: Image) -> Task<AppMessage> {
    let opened = chrono::Local::now().naive_local();
    let size = window_size(image.size(), primary_display(app));
    let (id, open) = app.windows.open(
        WindowKind::Editor,
        window::Settings {
            size,
            min_size: Some(MIN_SIZE),
            position: window::Position::Centered,
            ..window::Settings::default()
        },
    );
    tracing::info!(
        ?id,
        width = image.width(),
        height = image.height(),
        "opening an editor"
    );
    let title = format!(
        "{} — {}",
        flavor::DISPLAY_NAME,
        opened.format("%Y-%m-%d %H:%M:%S")
    );
    app.editor.windows.insert(
        id,
        Session {
            editor: Editor::new(image),
            opened,
            title,
        },
    );
    // Not every platform reports a new window's scale factor (the editor
    // hears of later changes itself), so ask for it.
    let scale_factor = window::scale_factor(id).map(move |scale_factor| {
        AppMessage::Editor(Message::Widget(
            id,
            chartreuse_editor::Message::ScaleFactor(scale_factor),
        ))
    });
    // The app has no Dock icon, so bring the window forward explicitly.
    open.discard()
        .chain(window::gain_focus(id))
        .chain(scale_factor)
}

/// The primary display's size in points, if the displays can be listed.
fn primary_display(app: &App) -> Option<LogicalSize> {
    match app.platform.displays.displays() {
        Ok(displays) => displays
            .into_iter()
            .find(|display| display.is_primary)
            .map(|display| display.logical_bounds.size),
        Err(error) => {
            tracing::warn!(%error, "could not list the displays to size the editor");
            None
        }
    }
}

/// Finishes the editing in progress in `window` and flattens its document off
/// the main thread, for export as `action` asks.
fn export(app: &mut App, window: window::Id, action: Action) -> Task<AppMessage> {
    let Some(session) = app.editor.windows.get_mut(&window) else {
        return Task::none();
    };
    session.editor.finish();
    let document = session.editor.document().clone();
    let taken = session.opened;
    let then_close = action.close.then_some(window);
    Task::perform(
        async move {
            flatten(&document).map(|image| Request {
                image: Arc::new(image),
                taken,
                then_close,
            })
        },
        move |request| AppMessage::Editor(Message::Flattened(action.target, request)),
    )
}

/// The bar along the bottom of editor window `window`: the size of the image
/// an export of `editor` produces, then the export buttons.
fn status_bar<'a>(editor: &Editor, window: window::Id) -> Element<'a, AppMessage> {
    let action = |label: &'a str, target, close| {
        button(text(label))
            .style(if close {
                button::primary
            } else {
                button::secondary
            })
            .on_press(AppMessage::Editor(Message::Export(
                window,
                Action { target, close },
            )))
    };
    let shortcut = |key: char| {
        let hint = if cfg!(target_os = "macos") {
            format!("⌘{key}")
        } else {
            format!("Ctrl+{key}")
        };
        text(hint)
    };
    let size = editor.document().export_size();
    row![
        text(format!("{} × {} px", size.width, size.height)),
        space().width(Length::Fill),
        tooltip(
            action("Copy", Target::Copy, false),
            shortcut('C'),
            tooltip::Position::Top
        ),
        tooltip(
            action("Save…", Target::Save, false),
            shortcut('S'),
            tooltip::Position::Top
        ),
        action("Copy & Close", Target::Copy, true),
        action("Save & Close…", Target::Save, true),
    ]
    .spacing(8)
    .padding(8)
    .align_y(Alignment::Center)
    .into()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chartreuse_config::SaveDirectory;
    use chartreuse_core::color::Rgba8;
    use chartreuse_core::Error;
    use chartreuse_editor::canvas::{Input, InputKind};
    use chartreuse_editor::model::Style;
    use chartreuse_editor::tools::ToolKind;
    use chartreuse_platform::fake::Fake;
    use iced::{keyboard, Point};

    use super::*;

    /// The image editors open over in these tests.
    const IMAGE: PhysicalSize = PhysicalSize::new(24, 16);

    fn white() -> Image {
        Image::filled(IMAGE, Rgba8::WHITE)
    }

    fn editors(app: &App) -> Vec<window::Id> {
        app.windows.of_kind(WindowKind::Editor).collect()
    }

    fn alerts(app: &App) -> usize {
        app.windows.of_kind(WindowKind::Alert).count()
    }

    /// Opens an editor over `image` and returns its window.
    fn open_editor(app: &mut App, image: Image) -> window::Id {
        let before = editors(app);
        let _ = app.settle(AppMessage::Editor(Message::Open(Arc::new(image))));
        let opened: Vec<_> = editors(app)
            .into_iter()
            .filter(|window| !before.contains(window))
            .collect();
        assert_eq!(opened.len(), 1, "one new editor window");
        opened[0]
    }

    /// A test app whose saves go to a fresh temporary directory, with an
    /// editor open over [`white`].
    fn app_with_editor() -> (App, Fake, tempfile::TempDir, window::Id) {
        let (mut app, fake) = App::for_test();
        let saves = tempfile::tempdir().unwrap();
        app.config.save_directory = Some(SaveDirectory::new(saves.path()).unwrap());
        let window = open_editor(&mut app, white());
        (app, fake, saves, window)
    }

    fn widget(app: &mut App, window: window::Id, message: chartreuse_editor::Message) {
        let _ = app.settle(AppMessage::Editor(Message::Widget(window, message)));
    }

    /// Canvas input over an [`IMAGE`]-sized canvas plus the margin, where the
    /// fitted view is 1:1 and document point `(x, y)` is at canvas point
    /// `(x + MARGIN, y + MARGIN)`.
    fn canvas(app: &mut App, window: window::Id, kind: InputKind) {
        let size = Size::new(
            IMAGE.width as f32 + 2.0 * MARGIN,
            IMAGE.height as f32 + 2.0 * MARGIN,
        );
        widget(
            app,
            window,
            chartreuse_editor::Message::Canvas(Input { size, kind }),
        );
    }

    fn at(x: f32, y: f32) -> Point {
        Point::new(x + MARGIN, y + MARGIN)
    }

    /// Draws a rectangle from (2, 2) to (21, 13) with the default style, the
    /// way the canvas and toolbar would.
    fn draw_rectangle(app: &mut App, window: window::Id) {
        widget(
            app,
            window,
            chartreuse_editor::Message::Tool(ToolKind::Rectangle),
        );
        let (from, to) = (at(2.0, 2.0), at(21.0, 13.0));
        canvas(
            app,
            window,
            InputKind::Press {
                position: from,
                clicks: 1,
            },
        );
        canvas(
            app,
            window,
            InputKind::Move {
                position: at(10.0, 8.0),
            },
        );
        canvas(app, window, InputKind::Move { position: to });
        canvas(app, window, InputKind::Release { position: to });
    }

    /// A Cmd (Ctrl) shortcut in the editor.
    fn command(app: &mut App, window: window::Id, key: &str) {
        canvas(
            app,
            window,
            InputKind::Key {
                key: keyboard::Key::Character(key.into()),
                modifiers: keyboard::Modifiers::COMMAND,
                text: None,
            },
        );
    }

    fn export(app: &mut App, window: window::Id, target: Target, close: bool) {
        let _ = app.settle(AppMessage::Editor(Message::Export(
            window,
            Action { target, close },
        )));
    }

    /// What the editor in `window` exports now.
    fn flattened(app: &App, window: window::Id) -> Image {
        flatten(app.editor.get(window).unwrap().document()).unwrap()
    }

    /// Asserts that `image` is [`white`] with the rectangle from
    /// [`draw_rectangle`] drawn in.
    fn assert_annotated(image: &Image) {
        assert_eq!(image.size(), IMAGE);
        assert_eq!(image.pixel(10, 2), Some(Style::DEFAULT_COLOR), "top edge");
        assert_eq!(image.pixel(21, 8), Some(Style::DEFAULT_COLOR), "right edge");
        assert_eq!(image.pixel(10, 8), Some(Rgba8::WHITE), "inside");
    }

    #[test]
    fn each_image_opens_in_its_own_editor_window() {
        let (mut app, _fake) = App::for_test();
        let red = Image::filled(PhysicalSize::new(3, 2), Rgba8::from_rgb_hex(0xff0000));
        let first = open_editor(&mut app, white());
        let second = open_editor(&mut app, red.clone());

        assert_ne!(first, second);
        assert_eq!(app.editor.get(first).unwrap().document().base(), &white());
        assert_eq!(app.editor.get(second).unwrap().document().base(), &red);
        assert!(app.title(first).starts_with(flavor::DISPLAY_NAME));
    }

    #[test]
    fn windows_fit_the_image_within_most_of_the_primary_display() {
        let display = Some(LogicalSize::new(1500.0, 1000.0));
        let size = |width, height| window_size(PhysicalSize::new(width, height), display);

        assert_eq!(size(10, 10), MIN_SIZE, "small images get the minimum");
        assert_eq!(
            size(900, 600),
            Size::new(900.0 + CHROME.width, 600.0 + CHROME.height),
            "room for the image at one point per pixel"
        );
        assert_eq!(
            size(4000, 3000),
            Size::new(1200.0, 800.0),
            "80% of the display"
        );
        assert_eq!(size(4000, 10), Size::new(1200.0, MIN_SIZE.height));
        assert_eq!(
            window_size(PhysicalSize::new(4000, 3000), None),
            FALLBACK_MAX_SIZE
        );
        assert_eq!(
            window_size(
                PhysicalSize::new(4000, 3000),
                Some(LogicalSize::new(640.0, 400.0))
            ),
            MIN_SIZE,
            "the minimum wins on a tiny display"
        );
    }

    #[test]
    fn save_writes_the_annotated_image_where_the_user_chose() {
        let (mut app, fake, saves, window) = app_with_editor();
        let chosen = saves.path().join("chosen.png");
        fake.set_save_answer(Some(chosen.clone()));
        draw_rectangle(&mut app, window);

        export(&mut app, window, Target::Save, false);

        let saved = chartreuse_imaging::decode_file(&chosen).unwrap();
        assert_annotated(&saved);
        assert_eq!(saved, flattened(&app, window));
        assert_eq!(editors(&app), [window], "the window stays open");
        assert_eq!(alerts(&app), 0);
    }

    #[test]
    fn copy_puts_the_annotated_image_on_the_clipboard() {
        let (mut app, fake, _saves, window) = app_with_editor();
        draw_rectangle(&mut app, window);

        export(&mut app, window, Target::Copy, false);

        let copied = fake.clipboard().expect("the image was copied");
        assert_annotated(&copied);
        assert_eq!(editors(&app), [window], "the window stays open");
    }

    #[test]
    fn the_keyboard_shortcuts_save_and_copy() {
        let (mut app, fake, saves, window) = app_with_editor();
        let chosen = saves.path().join("keyboard.png");
        fake.set_save_answer(Some(chosen.clone()));
        draw_rectangle(&mut app, window);

        command(&mut app, window, "c");
        assert_annotated(&fake.clipboard().expect("Cmd+C copied"));
        command(&mut app, window, "s");
        assert_annotated(&chartreuse_imaging::decode_file(&chosen).unwrap());
        assert_eq!(editors(&app), [window], "shortcuts never close");
    }

    #[test]
    fn a_text_edit_in_progress_is_exported() {
        let (mut app, fake, _saves, window) = app_with_editor();
        widget(
            &mut app,
            window,
            chartreuse_editor::Message::Tool(ToolKind::Text),
        );
        canvas(
            &mut app,
            window,
            InputKind::Press {
                position: at(2.0, 2.0),
                clicks: 1,
            },
        );
        canvas(
            &mut app,
            window,
            InputKind::Release {
                position: at(2.0, 2.0),
            },
        );
        canvas(
            &mut app,
            window,
            InputKind::Key {
                key: keyboard::Key::Character("W".into()),
                modifiers: keyboard::Modifiers::default(),
                text: Some("W".into()),
            },
        );

        export(&mut app, window, Target::Copy, false);
        let copied = fake.clipboard().unwrap();
        assert_ne!(copied, white(), "the typed text is in the export");
    }

    #[test]
    fn close_after_closes_the_window_once_the_export_succeeds() {
        let (mut app, fake, saves, copied) = app_with_editor();
        draw_rectangle(&mut app, copied);
        export(&mut app, copied, Target::Copy, true);
        assert_annotated(&fake.clipboard().unwrap());
        assert!(editors(&app).is_empty());
        assert!(app.editor.get(copied).is_none());

        let saved = open_editor(&mut app, white());
        let chosen = saves.path().join("closed.png");
        fake.set_save_answer(Some(chosen.clone()));
        export(&mut app, saved, Target::Save, true);
        assert_eq!(chartreuse_imaging::decode_file(&chosen).unwrap(), white());
        assert!(editors(&app).is_empty());
        assert_eq!(alerts(&app), 0);
    }

    #[test]
    fn a_cancelled_or_failed_export_leaves_the_window_open() {
        struct Broken;
        impl chartreuse_platform::Clipboard for Broken {
            fn write_image(&self, _image: &Image) -> chartreuse_core::Result<()> {
                Err(Error::Platform("the pasteboard refused".into()))
            }
            fn read_image(&self) -> chartreuse_core::Result<Option<Image>> {
                Ok(None)
            }
        }

        let (mut app, fake, saves, window) = app_with_editor();

        fake.set_save_answer(None);
        export(&mut app, window, Target::Save, true);
        assert_eq!(editors(&app), [window], "cancelled");
        assert_eq!(alerts(&app), 0);

        let in_the_way = saves.path().join("in the way");
        fs::write(&in_the_way, b"").unwrap();
        fake.set_save_answer(Some(in_the_way.join("inside.png")));
        export(&mut app, window, Target::Save, true);
        assert_eq!(editors(&app), [window], "the save failed");
        assert_eq!(alerts(&app), 1);

        app.platform.clipboard = Box::new(Broken);
        export(&mut app, window, Target::Copy, true);
        assert_eq!(editors(&app), [window], "the copy failed");
        assert_eq!(alerts(&app), 2);
    }

    #[test]
    fn widget_messages_reach_the_editor_of_their_window_only() {
        let (mut app, _fake) = App::for_test();
        let first = open_editor(&mut app, white());
        let second = open_editor(&mut app, white());

        widget(
            &mut app,
            first,
            chartreuse_editor::Message::Tool(ToolKind::Text),
        );
        widget(
            &mut app,
            window::Id::unique(),
            chartreuse_editor::Message::Undo,
        );
        assert_eq!(app.editor.get(first).unwrap().tool(), ToolKind::Text);
        assert_eq!(app.editor.get(second).unwrap().tool(), ToolKind::Select);
    }

    #[test]
    fn closing_a_window_drops_its_editor() {
        let (mut app, _fake) = App::for_test();
        let window = open_editor(&mut app, white());
        let _ = app.settle(AppMessage::WindowClosed(window));
        assert!(app.editor.get(window).is_none());
        assert!(editors(&app).is_empty());
    }
}
