//! The capture flow: captures every display first, then hands the result on.
//! Owned by integration tasks I2, I3, and I5.
//!
//! # Flow
//!
//! 1. [`Message::Start`] (from the status item menu or a hotkey) passes the
//!    Screen Recording gate ([`permission::ensure_screen_recording`]), then
//!    captures every display through the platform
//!    [`Capture`](chartreuse_platform::Capture) trait, off the main thread. A
//!    selection capture lists the windows
//!    ([`WindowList`](chartreuse_platform::WindowList)) at the same time, so
//!    they match what the overlays show.
//! 2. The captures arrive frozen, with the [`DisplayLayout`] they were taken
//!    from, as a [`Snapshot`] in the [`Scene`] of [`Message::Captured`].
//! 3. By mode:
//!    - **Display**: the snapshot is composited onto the whole desktop
//!      ([`Snapshot::desktop`], at the largest scale factor, off the main
//!      thread).
//!    - **Selection**: the snapshot is kept while the selection overlays show
//!      it (`overlay::Message::Open`), selecting a rectangle first; Space
//!      switches to selecting a window and back. A committed rectangle
//!      ([`Message::Selected`]) is cropped at the largest scale factor of the
//!      displays it covers ([`rectangle::output_grid`], then
//!      [`Snapshot::composite`] off the main thread). A committed window
//!      ([`Message::WindowSelected`]) is captured directly, not cropped from
//!      the snapshot: whole even where other windows covered it, with its
//!      shadow and rounded corners. A cancelled selection
//!      ([`Message::SelectionCancelled`]) discards the snapshot. A window list
//!      that fails leaves rectangle selection only.
//!
//!    Where Chartreuse cannot select by itself, the platform's own capture
//!    interface picks instead ([`Capture::capture_interactively`], Wayland's
//!    Screenshot portal): for the whole selection where full-screen overlays
//!    could cover only one of several displays, and for window selection on a
//!    platform that offers one, since it offers one because it cannot list
//!    windows. There, Space in the overlays hands the capture over to it
//!    ([`Message::PickOnPlatform`]). The picked image goes straight to step 4;
//!    cancelling ends the capture.
//! 4. The image ([`Message::Finished`]) is handed on as the settings'
//!    post-capture behavior ([`App::config`], [`AfterCapture`]) says: it opens
//!    in a new editor window (`editor::Message::Open`, the default), is copied
//!    to the clipboard, or is saved into the save directory and copied
//!    (`export::Target::SaveToDirectory` and `export::Target::Copy`, which
//!    report their own failures).
//!
//! [`Capture::capture_interactively`]: chartreuse_platform::Capture::capture_interactively
//!
//! One capture runs at a time, selection included: a `Start` while one is in
//! progress is ignored.
//!
//! # Failures
//!
//! A capture that fails with `Error::PermissionDenied(Permission::ScreenRecording)`
//! (which the platform also reports for captures that come back blank) opens the
//! permission guidance ([`permission::show_guidance`]). Any other failure,
//! such as a selected window that closed before it could be captured, is
//! reported with [`alert::report_error`].

use std::sync::Arc;

use chartreuse_config::AfterCapture;
use chartreuse_core::capture::CaptureMode;
use chartreuse_core::display::{DisplayLayout, PixelGrid};
use chartreuse_core::geometry::LogicalRect;
use chartreuse_core::image::Image;
use chartreuse_core::permission::Permission;
use chartreuse_core::window::WindowId;
use chartreuse_core::{Error, Result};
use chartreuse_imaging::Composite;
use chartreuse_overlay::rectangle;
use chartreuse_platform::{DisplayCapture, OverlayPlacement};
use iced::futures::future::{self, BoxFuture, FutureExt};
use iced::{Subscription, Task};

use crate::alert::{self, Notice};
use crate::app::{App, Message as AppMessage};
use crate::export::{self, Request, Target};
use crate::overlay::Windows;
use crate::{editor, overlay, permission};

/// This feature's part of the app state ([`App::capture`]).
#[derive(Debug, Default)]
pub struct State {
    /// The mode of the capture in progress, from `Start` until its image is
    /// handed on, it fails, or its selection is cancelled.
    in_progress: Option<CaptureMode>,
    /// The capture being selected from while the overlays show it.
    selecting: Option<Snapshot>,
}

impl State {
    /// The mode of the capture in progress, if any.
    #[must_use]
    pub const fn in_progress(&self) -> Option<CaptureMode> {
        self.in_progress
    }
}

/// Every display's contents at one moment, with the layout they were taken in.
///
/// Cloning is cheap: the captures are shared.
#[derive(Debug, Clone)]
pub struct Snapshot {
    layout: DisplayLayout,
    captures: Arc<[DisplayCapture]>,
}

impl Snapshot {
    /// Freezes `captures`, validating their displays as a [`DisplayLayout`].
    ///
    /// # Errors
    ///
    /// [`Error::Platform`] if the captured displays do not form a valid layout
    /// (none, no single primary, or a repeated id).
    pub fn new(captures: Vec<DisplayCapture>) -> Result<Self> {
        let layout = DisplayLayout::new(
            captures
                .iter()
                .map(|capture| capture.display.clone())
                .collect(),
        )?;
        Ok(Self {
            layout,
            captures: captures.into(),
        })
    }

    /// The displays as they were when captured.
    #[must_use]
    pub const fn layout(&self) -> &DisplayLayout {
        &self.layout
    }

    /// One capture per display, in the layout's order.
    #[must_use]
    pub fn captures(&self) -> &[DisplayCapture] {
        &self.captures
    }

    /// The captures rendered onto `grid`, e.g. [`DisplayLayout::capture_grid`] of
    /// a selection. CPU-heavy for large grids: run it off the main thread.
    ///
    /// # Errors
    ///
    /// As [`chartreuse_imaging::composite_at`].
    pub fn composite(&self, grid: PixelGrid) -> Result<Composite> {
        chartreuse_imaging::composite_at(
            self.captures
                .iter()
                .map(|capture| (&capture.display, &capture.image)),
            grid,
        )
    }

    /// The whole desktop: [`Self::composite`] on [`DisplayLayout::desktop_grid`].
    ///
    /// # Errors
    ///
    /// As [`chartreuse_imaging::composite_at`].
    pub fn desktop(&self) -> Result<Composite> {
        self.composite(self.layout.desktop_grid())
    }
}

/// What a capture took, by mode, before the mode's next step.
#[derive(Debug, Clone)]
pub enum Scene {
    /// Every display, for a display capture.
    Display(Snapshot),
    /// Every display, to select a rectangle or a window from, with how its
    /// windows are selected.
    Selection(Snapshot, Windows),
}

/// This feature's messages ([`AppMessage::Capture`]).
#[derive(Debug, Clone)]
pub enum Message {
    /// Start a capture: from the status item menu or a hotkey.
    Start(CaptureMode),
    /// The displays were captured for the capture in progress.
    Captured(Result<Scene>),
    /// The user committed this rectangle (global logical coordinates) over the
    /// capture being selected from.
    Selected(LogicalRect),
    /// The user selected this window.
    WindowSelected(WindowId),
    /// The user switched to window selection where the platform's own capture
    /// interface selects windows: it takes the capture over.
    PickOnPlatform,
    /// The user cancelled the selection.
    SelectionCancelled,
    /// The image of the capture in progress is ready.
    Finished(Result<Arc<Image>>),
}

pub fn boot(_app: &mut App) -> Task<AppMessage> {
    Task::none()
}

pub fn update(app: &mut App, message: Message) -> Task<AppMessage> {
    match message {
        Message::Start(mode) => start(app, mode),
        Message::Captured(Ok(scene)) => captured(app, scene),
        Message::Selected(rect) => selected(app, &rect),
        Message::WindowSelected(window) => window_selected(app, window),
        Message::PickOnPlatform => pick_on_platform(app),
        Message::SelectionCancelled => {
            tracing::info!("selection cancelled");
            app.capture.selecting = None;
            app.capture.in_progress = None;
            Task::none()
        }
        Message::Captured(Err(error)) | Message::Finished(Err(error)) => {
            app.capture.in_progress = None;
            failed(app, &error)
        }
        Message::Finished(Ok(image)) => {
            let mode = app.capture.in_progress.take();
            tracing::info!(
                mode = ?mode,
                width = image.size().width,
                height = image.size().height,
                after = ?app.config.after_capture,
                "captured"
            );
            hand_on(app.config.after_capture, image)
        }
    }
}

pub fn subscription(_app: &App) -> Subscription<AppMessage> {
    Subscription::none()
}

/// Hands a finished capture on as `after` says.
fn hand_on(after: AfterCapture, image: Arc<Image>) -> Task<AppMessage> {
    let export = |target, image| {
        Task::done(AppMessage::Export(export::Message::Export(
            target,
            Request {
                image,
                taken: chrono::Local::now().naive_local(),
                then_close: None,
                source: None,
            },
        )))
    };
    match after {
        AfterCapture::OpenEditor => Task::done(AppMessage::Editor(editor::Message::Open(image))),
        AfterCapture::Copy => export(Target::Copy, image),
        AfterCapture::SaveAndCopy => Task::batch([
            export(Target::Copy, Arc::clone(&image)),
            export(Target::SaveToDirectory, image),
        ]),
    }
}

fn start(app: &mut App, mode: CaptureMode) -> Task<AppMessage> {
    if let Some(running) = app.capture.in_progress {
        tracing::info!(%mode, %running, "a capture is already in progress; ignoring");
        return Task::none();
    }
    if let Err(guidance) = permission::ensure_screen_recording(app) {
        return guidance;
    }
    app.capture.in_progress = Some(mode);
    let windows = match mode {
        CaptureMode::Display => None,
        // Unpolled, the platform's own capture interface has not opened yet.
        CaptureMode::Selection => match app.platform.capture.capture_interactively() {
            Some(picker) if overlays_miss_displays(app) => {
                tracing::debug!(%mode, "the platform's capture interface picks what to capture");
                return platform_pick(picker);
            }
            // Such a platform cannot list windows: its interface selects them.
            Some(_) => Some(future::ready(Windows::Platform).boxed()),
            None => Some(list_windows(app)),
        },
    };
    let displays = app
        .platform
        .capture
        .capture_displays()
        .map(|captures| captures.and_then(Snapshot::new));
    let scene = match windows {
        None => displays
            .map(|snapshot| snapshot.map(Scene::Display))
            .boxed(),
        Some(windows) => future::join(displays, windows)
            .map(|(snapshot, windows)| snapshot.map(|snapshot| Scene::Selection(snapshot, windows)))
            .boxed(),
    };
    Task::perform(scene, |scene| AppMessage::Capture(Message::Captured(scene)))
}

/// The windows to select from, listed now; unavailable if they cannot be.
fn list_windows(app: &App) -> BoxFuture<'static, Windows> {
    app.platform
        .window_list
        .windows()
        .map(|windows| {
            windows.map_or_else(
                |error| {
                    tracing::warn!(%error, "could not list the windows to select from");
                    Windows::Unavailable
                },
                Windows::Listed,
            )
        })
        .boxed()
}

/// Whether the overlays would leave displays uncovered: full-screen overlays,
/// on a desktop of several.
fn overlays_miss_displays(app: &App) -> bool {
    app.platform.overlay_style.placement() == OverlayPlacement::Fullscreen
        && app
            .platform
            .displays
            .displays()
            .is_ok_and(|displays| displays.len() > 1)
}

/// Waits for the user's pick in the platform's own capture interface.
fn platform_pick(picked: BoxFuture<'static, Result<Option<Image>>>) -> Task<AppMessage> {
    Task::perform(picked, |picked| {
        AppMessage::Capture(match picked {
            Ok(Some(image)) => Message::Finished(Ok(Arc::new(image))),
            Ok(None) => Message::SelectionCancelled,
            Err(error) => Message::Finished(Err(error)),
        })
    })
}

fn captured(app: &mut App, scene: Scene) -> Task<AppMessage> {
    match scene {
        Scene::Display(snapshot) => {
            tracing::debug!(displays = snapshot.captures().len(), "displays captured");
            composite(move || snapshot.desktop())
        }
        Scene::Selection(snapshot, windows) => {
            tracing::debug!(
                displays = snapshot.captures().len(),
                "displays captured to select from"
            );
            app.capture.selecting = Some(snapshot.clone());
            Task::done(AppMessage::Overlay(overlay::Message::Open(
                snapshot, windows,
            )))
        }
    }
}

/// Crops the committed `rect` out of the capture being selected from.
fn selected(app: &mut App, rect: &LogicalRect) -> Task<AppMessage> {
    let Some(snapshot) = app.capture.selecting.take() else {
        return Task::none();
    };
    match rectangle::output_grid(snapshot.layout(), rect) {
        Some(grid) => composite(move || snapshot.composite(grid)),
        None => {
            // Committed selections always cover a display.
            tracing::warn!(?rect, "the selection covers no display; nothing captured");
            app.capture.in_progress = None;
            Task::none()
        }
    }
}

/// Captures the committed `window` directly, off the main thread.
fn window_selected(app: &mut App, window: WindowId) -> Task<AppMessage> {
    if app.capture.selecting.take().is_none() {
        return Task::none();
    }
    let capture = app.platform.capture.capture_window(window);
    Task::perform(capture.map(|image| image.map(Arc::new)), |image| {
        AppMessage::Capture(Message::Finished(image))
    })
}

/// Hands the capture being selected from over to the platform's own capture
/// interface.
fn pick_on_platform(app: &mut App) -> Task<AppMessage> {
    if app.capture.selecting.take().is_none() {
        return Task::none();
    }
    if let Some(picker) = app.platform.capture.capture_interactively() {
        tracing::debug!("the platform's capture interface selects the window");
        platform_pick(picker)
    } else {
        // The overlays hand over only where there is an interface.
        tracing::warn!("no platform capture interface to select the window");
        app.capture.in_progress = None;
        Task::none()
    }
}

/// Runs `render` off the main thread, then reports its image as
/// [`Message::Finished`].
fn composite(render: impl FnOnce() -> Result<Composite> + Send + 'static) -> Task<AppMessage> {
    Task::perform(
        async move { render().map(|composite| Arc::new(composite.image)) },
        |image| AppMessage::Capture(Message::Finished(image)),
    )
}

fn failed(app: &mut App, error: &Error) -> Task<AppMessage> {
    match error {
        Error::PermissionDenied(Permission::ScreenRecording) => {
            tracing::info!("capture withheld for lack of Screen Recording permission");
            permission::show_guidance(app)
        }
        error => alert::report_error(app, Notice::from_error("Capture failed", error)),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;

    use chartreuse_config::SaveDirectory;
    use chartreuse_core::color::Rgba8;
    use chartreuse_core::display::{DisplayId, DisplayInfo};
    use chartreuse_core::geometry::{LogicalPoint, LogicalRect, PhysicalSize, ScaleFactor};
    use chartreuse_core::permission::PermissionStatus;
    use chartreuse_core::window::{WindowId, WindowInfo, WindowOwner};
    use chartreuse_overlay::rectangle::Input;
    use chartreuse_overlay::window::Input as WindowInput;
    use chartreuse_platform::fake::{self, Fake};
    use chartreuse_platform::{Capture, MenuAction, NativeWindow, OverlayWindowStyle, WindowList};
    use futures::executor::block_on;
    use futures::future::{self, BoxFuture, FutureExt};
    use futures::StreamExt;
    use iced::advanced::subscription::into_recipes;

    use super::*;
    use crate::windows::WindowKind;
    use crate::{hotkeys, tray};

    /// A small desktop, to keep compositing fast: a 2× primary display with its
    /// top-left at the global origin, and a 1× display up and to the left of it.
    /// Together they span (-4, -2) to (8, 6) in logical points, leaving a gap
    /// below the 1× display.
    ///
    /// Two windows: [`FRONT`] on the primary display, in front of [`BACK`],
    /// which spans both displays.
    fn small_desktop() -> Fake {
        let display = |id, bounds: LogicalRect, scale: f64, is_primary| {
            let scale_factor = ScaleFactor::new(scale).unwrap();
            DisplayInfo {
                id: DisplayId(id),
                name: format!("Display {id}"),
                logical_bounds: bounds,
                pixel_size: bounds.size.to_physical(scale_factor),
                scale_factor,
                is_primary,
            }
        };
        Fake::with_world(
            vec![
                display(1, LogicalRect::new(0.0, 0.0, 8.0, 6.0), 2.0, true),
                display(2, LogicalRect::new(-4.0, -2.0, 4.0, 4.0), 1.0, false),
            ],
            vec![
                window(FRONT, 0, LogicalRect::new(1.0, 1.0, 4.0, 3.0)),
                window(BACK, 1, LogicalRect::new(-3.0, -1.0, 6.0, 4.0)),
            ],
        )
    }

    const FRONT: WindowId = WindowId(7);
    const BACK: WindowId = WindowId(8);

    fn window(id: WindowId, z_order: u32, bounds: LogicalRect) -> WindowInfo {
        WindowInfo {
            id,
            title: Some(format!("Window {}", id.0)),
            owner: WindowOwner {
                name: "Test".into(),
                pid: None,
            },
            bounds,
            z_order,
        }
    }

    fn start(app: &mut App, mode: CaptureMode) -> Vec<AppMessage> {
        app.settle(AppMessage::Capture(Message::Start(mode)))
    }

    fn windows(app: &App, kind: WindowKind) -> usize {
        app.windows.of_kind(kind).count()
    }

    /// The images of the open editor windows, in no particular order.
    fn editor_images(app: &App) -> Vec<Image> {
        app.windows
            .of_kind(WindowKind::Editor)
            .map(|window| app.editor.get(window).unwrap().document().base().clone())
            .collect()
    }

    /// The image of the one open editor window.
    fn edited(app: &App) -> Image {
        let mut images = editor_images(app);
        assert_eq!(images.len(), 1, "one editor window");
        images.pop().unwrap()
    }

    /// A capture backend that always answers `result`.
    struct Scripted(Result<Vec<DisplayCapture>>);

    impl Capture for Scripted {
        fn capture_displays(&self) -> BoxFuture<'static, Result<Vec<DisplayCapture>>> {
            future::ready(self.0.clone()).boxed()
        }

        fn capture_window(&self, _window: WindowId) -> BoxFuture<'static, Result<Image>> {
            future::ready(Err(Error::Unsupported("window capture"))).boxed()
        }
    }

    /// The fake's display captures, with window captures that fail with the
    /// error.
    struct WindowFails(Fake, Error);

    impl Capture for WindowFails {
        fn capture_displays(&self) -> BoxFuture<'static, Result<Vec<DisplayCapture>>> {
            self.0.capture_displays()
        }

        fn capture_window(&self, _window: WindowId) -> BoxFuture<'static, Result<Image>> {
            future::ready(Err(self.1.clone())).boxed()
        }
    }

    /// A window list that always fails with the error.
    struct ListFails(Error);

    impl WindowList for ListFails {
        fn windows(&self) -> BoxFuture<'static, Result<Vec<WindowInfo>>> {
            future::ready(Err(self.0.clone())).boxed()
        }
    }

    #[test]
    fn a_display_capture_composites_the_desktop_and_opens_it_in_an_editor() {
        let (mut app, _default_desktop) = App::for_test();
        let fake = small_desktop();
        app.platform = fake.platform();
        let _ = start(&mut app, CaptureMode::Display);

        // The desktop spans 12 × 8 logical points, composited at the largest
        // scale factor, 2×.
        let desktop = edited(&app);
        assert_eq!(desktop.size(), PhysicalSize::new(24, 16));

        // The 2× primary display lands unscaled with its top-left, the global
        // origin, at pixel (4, 2) × 2.
        let captures = block_on(fake.platform().capture.capture_displays()).unwrap();
        let primary = captures.iter().find(|c| c.display.is_primary).unwrap();
        for (x, y) in [(0, 0), (5, 3), (15, 11)] {
            assert_eq!(
                desktop.pixel(8 + x, 4 + y),
                primary.image.pixel(x, y),
                "primary pixel ({x}, {y})"
            );
        }
        // Logical (-1, 4), below the 1× display and left of the primary, lies
        // on no display.
        let gap = desktop.pixel((4 - 1) * 2, (2 + 4) * 2).unwrap();
        assert_eq!(gap.a, 0, "gaps between displays are transparent");

        assert_eq!(fake.clipboard(), None, "nothing is copied until asked");
        assert_eq!(app.capture.in_progress(), None);
        assert_eq!(windows(&app, WindowKind::Alert), 0);
    }

    /// An app on [`small_desktop`] whose captures are handed on as `after`
    /// says.
    fn small_app(after: AfterCapture) -> (App, Fake) {
        let (mut app, _default_desktop) = App::for_test();
        let fake = small_desktop();
        app.platform = fake.platform();
        app.config.after_capture = after;
        (app, fake)
    }

    #[test]
    fn copy_after_capture_copies_the_capture_without_an_editor() {
        let (mut app, fake) = small_app(AfterCapture::Copy);
        let _ = start(&mut app, CaptureMode::Display);

        let copied = fake.clipboard().expect("the capture was copied");
        assert_eq!(
            copied.size(),
            PhysicalSize::new(24, 16),
            "the whole desktop"
        );
        assert!(editor_images(&app).is_empty());
        assert_eq!(app.capture.in_progress(), None);
        assert_eq!(windows(&app, WindowKind::Alert), 0);
    }

    #[test]
    fn save_and_copy_after_capture_saves_into_the_save_directory_and_copies() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("Captures");
        let (mut app, fake) = small_app(AfterCapture::SaveAndCopy);
        app.config.save_directory = Some(SaveDirectory::new(&directory).unwrap());
        app.config.file_name = "Capture".parse().unwrap();

        let _ = start(&mut app, CaptureMode::Display);
        let copied = fake.clipboard().expect("the capture was copied");
        assert_eq!(copied.size(), PhysicalSize::new(24, 16));
        let saved = chartreuse_imaging::decode_file(&directory.join("Capture.png")).unwrap();
        assert_eq!(saved, copied);

        // The next capture does not replace the first.
        let _ = start(&mut app, CaptureMode::Display);
        let mut names: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["Capture (2).png", "Capture.png"]);
        assert!(editor_images(&app).is_empty());
        assert!(fake.save_requests().is_empty(), "no save dialog");
        assert_eq!(windows(&app, WindowKind::Alert), 0);
    }

    /// The first message `subscription` delivers.
    fn next_message(subscription: Subscription<AppMessage>) -> AppMessage {
        let mut recipes = into_recipes(subscription);
        assert_eq!(recipes.len(), 1);
        let events = recipes
            .pop()
            .unwrap()
            .stream(futures::stream::empty().boxed());
        block_on(events.into_future()).0.expect("a message")
    }

    #[test]
    fn the_menu_and_the_display_hotkey_each_open_the_desktop_in_an_editor() {
        let (mut app, _default_desktop) = App::for_test();
        let fake = small_desktop();
        app.platform = fake.platform();
        let _ = app.settle(AppMessage::Tray(tray::Message::Install));
        let _ = app.settle(AppMessage::Hotkeys(hotkeys::Message::Register));
        let desktop = PhysicalSize::new(24, 16);

        assert!(fake.choose_menu_action(MenuAction::Capture(CaptureMode::Display)));
        let chosen = next_message(tray::subscription(&app));
        let _ = app.settle(chosen);
        assert_eq!(edited(&app).size(), desktop);

        assert!(fake.press_hotkey(app.config.hotkeys.get(CaptureMode::Display)));
        let pressed = next_message(hotkeys::subscription(&app));
        let _ = app.settle(pressed);
        let sizes: Vec<_> = editor_images(&app).iter().map(Image::size).collect();
        assert_eq!(
            sizes,
            [desktop, desktop],
            "each capture opens its own editor"
        );
        assert_eq!(windows(&app, WindowKind::Alert), 0);
    }

    #[test]
    fn a_denied_permission_opens_the_guidance_and_captures_nothing() {
        let (mut app, fake) = App::for_test();
        fake.set_screen_recording(PermissionStatus::Denied);

        for mode in CaptureMode::ALL {
            let handled = start(&mut app, mode);
            assert!(
                !handled
                    .iter()
                    .any(|message| matches!(message, AppMessage::Capture(Message::Captured(..)))),
                "{mode}: {handled:?}"
            );
            assert_eq!(windows(&app, WindowKind::Permission), 1, "{mode}");
            assert_eq!(windows(&app, WindowKind::Overlay), 0, "{mode}");
            assert_eq!(app.capture.in_progress(), None, "{mode}");
        }
        assert!(editor_images(&app).is_empty());
    }

    #[test]
    fn a_capture_withheld_by_the_os_opens_the_guidance() {
        // The status says granted, but the capture itself is refused (or came
        // back blank, which the platform reports the same way).
        let (mut app, _fake) = App::for_test();
        app.platform.capture = Box::new(Scripted(Err(Error::PermissionDenied(
            Permission::ScreenRecording,
        ))));

        let _ = start(&mut app, CaptureMode::Display);
        assert_eq!(windows(&app, WindowKind::Permission), 1);
        assert_eq!(windows(&app, WindowKind::Alert), 0);
        assert!(editor_images(&app).is_empty());
    }

    #[test]
    fn capture_errors_are_reported_and_the_next_capture_can_start() {
        let (mut app, _fake) = App::for_test();
        app.platform.capture = Box::new(Scripted(Err(Error::Platform(
            "SCShareableContent failed".into(),
        ))));
        let _ = start(&mut app, CaptureMode::Display);
        // No displays at all do not form a layout.
        app.platform.capture = Box::new(Scripted(Ok(Vec::new())));
        let _ = start(&mut app, CaptureMode::Display);

        assert_eq!(windows(&app, WindowKind::Alert), 2);
        assert_eq!(windows(&app, WindowKind::Permission), 0);
        assert!(editor_images(&app).is_empty());
        assert_eq!(app.capture.in_progress(), None);
    }

    #[test]
    fn a_start_during_a_capture_is_ignored() {
        let (mut app, _fake) = App::for_test();
        let first = app.update(AppMessage::Capture(Message::Start(CaptureMode::Display)));
        assert!(iced_runtime::task::into_stream(first).is_some());
        assert_eq!(app.capture.in_progress(), Some(CaptureMode::Display));

        let second = app.update(AppMessage::Capture(Message::Start(CaptureMode::Display)));
        assert!(iced_runtime::task::into_stream(second).is_none());
    }

    fn overlays(app: &App) -> Vec<iced::window::Id> {
        app.windows.of_kind(WindowKind::Overlay).collect()
    }

    fn select(app: &mut App, input: Input) {
        let _ = app.settle(AppMessage::Overlay(overlay::Message::Rectangle(input)));
    }

    fn pick(app: &mut App, input: WindowInput) {
        let _ = app.settle(AppMessage::Overlay(overlay::Message::Window(input)));
    }

    /// Starts a selection capture of the small desktop.
    fn start_selection() -> (App, Fake) {
        let (mut app, _default_desktop) = App::for_test();
        let fake = small_desktop();
        app.platform = fake.platform();
        let _ = start(&mut app, CaptureMode::Selection);
        (app, fake)
    }

    /// Starts a selection capture of the small desktop and switches it to
    /// selecting windows.
    fn start_window_selection() -> (App, Fake) {
        let (mut app, fake) = start_selection();
        select(&mut app, Input::Space(None));
        (app, fake)
    }

    /// The overlays' displays.
    fn covered(app: &App) -> HashSet<DisplayId> {
        overlays(app)
            .into_iter()
            .map(|window| app.overlay.display(window).expect("an overlay's display"))
            .collect()
    }

    #[test]
    fn a_selection_capture_opens_one_overlay_per_display() {
        let (mut app, _fake) = start_selection();

        assert_eq!(overlays(&app).len(), 2);
        assert_eq!(covered(&app), HashSet::from([DisplayId(1), DisplayId(2)]));
        assert_eq!(app.capture.in_progress(), Some(CaptureMode::Selection));
        assert!(
            editor_images(&app).is_empty(),
            "nothing opens before a commit"
        );

        // One selection at a time.
        let _ = start(&mut app, CaptureMode::Selection);
        let _ = start(&mut app, CaptureMode::Display);
        assert_eq!(overlays(&app).len(), 2);
        assert!(editor_images(&app).is_empty());
    }

    #[test]
    fn a_committed_rectangle_is_cropped_at_the_largest_scale_and_opened_in_an_editor() {
        let (mut app, fake) = start_selection();

        // From (-3, -1) on the 1× display to (2, 2) on the 2× primary: 5 × 3
        // logical points, output at 2×.
        let (from, to) = (LogicalPoint::new(-3.0, -1.0), LogicalPoint::new(2.0, 2.0));
        select(&mut app, Input::Press(from));
        select(&mut app, Input::Move(to));
        select(&mut app, Input::Release(to));

        assert!(overlays(&app).is_empty(), "the overlays closed");
        let image = edited(&app);
        assert_eq!(image.size(), PhysicalSize::new(10, 6));

        let captures = block_on(fake.platform().capture.capture_displays()).unwrap();
        let capture = |id| &captures.iter().find(|c| c.display.id == id).unwrap().image;
        // The primary's top-left, the global origin, is 3 × 1 points into the
        // selection, and lands unscaled.
        for (x, y) in [(0, 0), (1, 1), (3, 3)] {
            assert_eq!(
                image.pixel(6 + x, 2 + y),
                capture(DisplayId(1)).pixel(x, y),
                "primary pixel ({x}, {y})"
            );
        }
        // The selection's top-left is pixel (1, 1) of the 1× display, doubled.
        for (x, y) in [(0, 0), (1, 1)] {
            assert_eq!(image.pixel(x, y), capture(DisplayId(2)).pixel(1, 1));
        }
        // (1, -0.5) is above the primary and right of the 1× display.
        assert_eq!(image.pixel(8, 1).unwrap().a, 0, "gaps are transparent");
        assert_eq!(app.capture.in_progress(), None);
        assert_eq!(windows(&app, WindowKind::Alert), 0);
    }

    #[test]
    fn escape_closes_the_overlays_and_opens_nothing() {
        let (mut app, _fake) = start_selection();
        select(&mut app, Input::Press(LogicalPoint::new(1.0, 1.0)));
        select(&mut app, Input::Move(LogicalPoint::new(6.0, 5.0)));
        select(&mut app, Input::Escape);

        assert!(overlays(&app).is_empty());
        assert_eq!(app.capture.in_progress(), None);
        assert!(editor_images(&app).is_empty());

        // Input after the session ends goes nowhere.
        select(&mut app, Input::Release(LogicalPoint::new(6.0, 5.0)));
        assert!(editor_images(&app).is_empty());

        let _ = start(&mut app, CaptureMode::Selection);
        assert_eq!(overlays(&app).len(), 2, "a new capture can start");
    }

    #[test]
    fn after_space_a_clicked_window_is_captured_directly_and_opened_in_an_editor() {
        let (mut app, fake) = start_window_selection();
        // Rectangle input no longer selects.
        select(&mut app, Input::Press(LogicalPoint::new(1.0, 1.0)));
        select(&mut app, Input::Move(LogicalPoint::new(6.0, 5.0)));
        select(&mut app, Input::Release(LogicalPoint::new(6.0, 5.0)));
        assert_eq!(overlays(&app).len(), 2);

        // (6, 5) is on the primary display but on no window.
        pick(&mut app, WindowInput::Move(LogicalPoint::new(6.0, 5.0)));
        pick(&mut app, WindowInput::Click(LogicalPoint::new(6.0, 5.0)));
        assert_eq!(overlays(&app).len(), 2, "a click on no window is ignored");
        assert!(editor_images(&app).is_empty());

        // (2, 2) is on the front window, which covers the back one there.
        pick(&mut app, WindowInput::Move(LogicalPoint::new(2.0, 2.0)));
        pick(&mut app, WindowInput::Click(LogicalPoint::new(2.0, 2.0)));

        assert!(overlays(&app).is_empty(), "the overlays closed");
        let image = edited(&app);
        let direct = block_on(fake.capture_window(FRONT)).unwrap();
        assert_eq!(image, direct, "the window's own capture, not a crop");
        assert_eq!(image.size(), PhysicalSize::new(8, 6));
        assert_eq!(app.capture.in_progress(), None);
        assert_eq!(windows(&app, WindowKind::Alert), 0);
    }

    #[test]
    fn space_again_goes_back_to_selecting_a_rectangle() {
        let (mut app, _fake) = start_window_selection();
        pick(&mut app, WindowInput::Space(None));
        // Window input no longer selects.
        pick(&mut app, WindowInput::Click(LogicalPoint::new(2.0, 2.0)));
        assert_eq!(overlays(&app).len(), 2);

        let (from, to) = (LogicalPoint::new(1.0, 1.0), LogicalPoint::new(4.0, 3.0));
        select(&mut app, Input::Press(from));
        select(&mut app, Input::Move(to));
        select(&mut app, Input::Release(to));
        assert!(overlays(&app).is_empty());
        assert_eq!(edited(&app).size(), PhysicalSize::new(6, 4), "a 2× crop");
    }

    #[test]
    fn space_mid_drag_keeps_the_rectangle() {
        let (mut app, _fake) = start_selection();
        let (from, to) = (LogicalPoint::new(1.0, 1.0), LogicalPoint::new(4.0, 3.0));
        select(&mut app, Input::Press(from));
        select(&mut app, Input::Move(to));
        select(&mut app, Input::Space(Some(to)));
        select(&mut app, Input::Release(to));

        assert!(overlays(&app).is_empty());
        assert_eq!(edited(&app).size(), PhysicalSize::new(6, 4));
    }

    #[test]
    fn escape_cancels_a_window_selection() {
        let (mut app, _fake) = start_window_selection();
        pick(&mut app, WindowInput::Move(LogicalPoint::new(2.0, 2.0)));
        pick(&mut app, WindowInput::Escape);

        assert!(overlays(&app).is_empty());
        assert_eq!(app.capture.in_progress(), None);

        // Input after the session ends goes nowhere.
        pick(&mut app, WindowInput::Click(LogicalPoint::new(2.0, 2.0)));
        assert!(editor_images(&app).is_empty());

        let _ = start(&mut app, CaptureMode::Selection);
        assert_eq!(overlays(&app).len(), 2, "a new capture can start");
    }

    #[test]
    fn a_window_capture_withheld_by_the_os_opens_the_guidance() {
        let (mut app, fake) = start_window_selection();
        app.platform.capture = Box::new(WindowFails(
            fake.clone(),
            Error::PermissionDenied(Permission::ScreenRecording),
        ));
        pick(&mut app, WindowInput::Click(LogicalPoint::new(2.0, 2.0)));

        assert!(overlays(&app).is_empty());
        assert_eq!(windows(&app, WindowKind::Permission), 1);
        assert_eq!(windows(&app, WindowKind::Alert), 0);
        assert_eq!(app.capture.in_progress(), None);
        assert!(editor_images(&app).is_empty());
    }

    #[test]
    fn a_window_that_cannot_be_captured_is_reported() {
        // E.g. the window closed while the overlays were up.
        let (mut app, fake) = start_window_selection();
        app.platform.capture = Box::new(WindowFails(
            fake.clone(),
            Error::Platform("no window with id 7".into()),
        ));
        pick(&mut app, WindowInput::Click(LogicalPoint::new(2.0, 2.0)));

        assert!(overlays(&app).is_empty());
        assert_eq!(windows(&app, WindowKind::Alert), 1);
        assert_eq!(windows(&app, WindowKind::Permission), 0);
        assert_eq!(app.capture.in_progress(), None);
        assert!(editor_images(&app).is_empty());
    }

    #[test]
    fn a_window_list_that_fails_leaves_only_rectangle_selection() {
        // The displays capture fine, but listing their windows is refused.
        let (mut app, _default_desktop) = App::for_test();
        let fake = small_desktop();
        app.platform = fake.platform();
        app.platform.window_list = Box::new(ListFails(Error::Platform(
            "SCShareableContent failed".into(),
        )));
        let _ = start(&mut app, CaptureMode::Selection);
        assert_eq!(overlays(&app).len(), 2);
        assert_eq!(windows(&app, WindowKind::Alert), 0);

        // Space does not switch to windows: a click selects nothing.
        select(&mut app, Input::Space(None));
        pick(&mut app, WindowInput::Click(LogicalPoint::new(2.0, 2.0)));
        assert_eq!(overlays(&app).len(), 2);

        let (from, to) = (LogicalPoint::new(1.0, 1.0), LogicalPoint::new(4.0, 3.0));
        select(&mut app, Input::Press(from));
        select(&mut app, Input::Move(to));
        select(&mut app, Input::Release(to));
        assert_eq!(edited(&app).size(), PhysicalSize::new(6, 4));
        assert_eq!(app.capture.in_progress(), None);
    }

    #[test]
    fn an_overlay_closed_from_outside_cancels_the_selection() {
        for (start, gesture) in [
            (start_selection as fn() -> (App, Fake), "rectangle"),
            (start_window_selection, "window"),
        ] {
            let (mut app, _fake) = start();
            let closed = overlays(&app)[0];
            let _ = app.settle(AppMessage::WindowClosed(closed));

            assert!(
                overlays(&app).is_empty(),
                "{gesture}: the other overlays closed too"
            );
            assert_eq!(app.capture.in_progress(), None, "{gesture}");
            assert!(editor_images(&app).is_empty(), "{gesture}");
        }
    }

    /// The fake's captures, plus a platform capture interface (as on
    /// Wayland) in which the user picks the image, or cancels with `None`.
    struct Picks(Fake, Option<Image>);

    impl Capture for Picks {
        fn capture_displays(&self) -> BoxFuture<'static, Result<Vec<DisplayCapture>>> {
            self.0.capture_displays()
        }

        fn capture_window(&self, window: WindowId) -> BoxFuture<'static, Result<Image>> {
            self.0.capture_window(window)
        }

        fn capture_interactively(&self) -> Option<BoxFuture<'static, Result<Option<Image>>>> {
            Some(future::ready(Ok(self.1.clone())).boxed())
        }
    }

    /// Full-screen overlays, as on Wayland.
    struct FullScreen;

    impl OverlayWindowStyle for FullScreen {
        fn placement(&self) -> OverlayPlacement {
            OverlayPlacement::Fullscreen
        }

        fn apply(&self, _window: NativeWindow<'_>, _display: &DisplayInfo) -> Result<()> {
            Ok(())
        }
    }

    fn picked() -> Image {
        Image::filled(PhysicalSize::new(3, 2), Rgba8::new(1, 2, 3, 255))
    }

    /// An app on `fake` whose platform picks `picked` interactively. Such a
    /// platform cannot list windows.
    fn picking_app(fake: &Fake, picked: Option<Image>) -> App {
        let (mut app, _default_desktop) = App::for_test();
        app.platform = fake.platform();
        app.platform.capture = Box::new(Picks(fake.clone(), picked));
        app.platform.window_list = Box::new(ListFails(Error::Unsupported("window lists")));
        app
    }

    #[test]
    fn where_the_platform_picks_windows_space_hands_the_capture_over() {
        let fake = small_desktop();
        let mut app = picking_app(&fake, Some(picked()));
        let _ = start(&mut app, CaptureMode::Selection);
        assert_eq!(overlays(&app).len(), 2, "rectangles are selected as usual");

        let handled = app.settle(AppMessage::Overlay(overlay::Message::Rectangle(
            Input::Space(None),
        )));
        assert!(overlays(&app).is_empty(), "the overlays closed");
        assert!(
            !handled
                .iter()
                .any(|message| matches!(message, AppMessage::Capture(Message::SelectionCancelled))),
            "closing the overlays does not cancel: {handled:?}"
        );
        assert_eq!(edited(&app), picked());
        assert_eq!(windows(&app, WindowKind::Alert), 0);
        assert_eq!(app.capture.in_progress(), None);
    }

    #[test]
    fn a_cancelled_platform_pick_ends_the_capture_quietly() {
        let fake = small_desktop();
        let mut app = picking_app(&fake, None);
        let _ = start(&mut app, CaptureMode::Selection);
        select(&mut app, Input::Space(None));

        assert!(overlays(&app).is_empty());
        assert!(editor_images(&app).is_empty());
        assert_eq!(windows(&app, WindowKind::Alert), 0);
        assert_eq!(app.capture.in_progress(), None, "a new capture can start");
    }

    #[test]
    fn selections_are_picked_by_the_platform_only_where_full_screen_overlays_miss_displays() {
        // Two displays under full-screen overlays: the platform picks.
        let two = small_desktop();
        let mut app = picking_app(&two, Some(picked()));
        app.platform.overlay_style = Arc::new(FullScreen);
        let _ = start(&mut app, CaptureMode::Selection);
        assert!(overlays(&app).is_empty());
        assert_eq!(edited(&app), picked());

        // One display: its full-screen overlay covers it.
        let primary = fake::default_displays()
            .into_iter()
            .find(|display| display.is_primary)
            .unwrap();
        let one = Fake::with_world(vec![primary.clone()], Vec::new());
        let mut app = picking_app(&one, Some(picked()));
        app.platform.overlay_style = Arc::new(FullScreen);
        let _ = start(&mut app, CaptureMode::Selection);
        assert_eq!(covered(&app), HashSet::from([primary.id]));
        assert!(editor_images(&app).is_empty());

        // Overlays over each display cover them all.
        let mut app = picking_app(&two, Some(picked()));
        let _ = start(&mut app, CaptureMode::Selection);
        assert_eq!(covered(&app), HashSet::from([DisplayId(1), DisplayId(2)]));
        assert!(editor_images(&app).is_empty());
    }
}
