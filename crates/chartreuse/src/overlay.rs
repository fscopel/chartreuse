//! Selection overlay windows (`WindowKind::Overlay`), one per display, driven by
//! `chartreuse-overlay`. Owned by integration tasks I3 and I5.
//!
//! # Flow
//!
//! 1. The capture flow sends [`Message::Open`] with the frozen [`Snapshot`] and
//!    how its windows are selected ([`Windows`]). Its captures become image
//!    handles off the main thread ([`Message::Frozen`]), then one overlay
//!    window opens per captured display ([`setup::open`]), each drawing its
//!    display's capture under one shared [`Selector`] in global logical
//!    coordinates. The selector starts with a rectangle [`Selection`], drawn by
//!    [`RectangleOverlay`]; Space switches it to a [`WindowSelection`], drawn
//!    by [`WindowOverlay`], and back. It does not switch mid-drag, where
//!    windows cannot be listed, or where the platform's own interface selects
//!    windows: there Space hands the capture over to that interface. The
//!    window selection highlights the window under the pointer when Space was
//!    pressed over it, else nothing until the pointer moves: the platform
//!    traits do not expose the pointer's position yet (macOS could report it
//!    with `NSEvent.mouseLocation`, Windows with `GetCursorPos`).
//! 2. Every overlay's pointer, Escape and Space input goes to that selector
//!    ([`Message::Rectangle`], [`Message::Window`]). Keys reach the focused
//!    overlay only: the platform style activates the app and makes each
//!    overlay key as it is shown, and the primary display's overlay is focused
//!    as well ([`window::gain_focus`]), for backends whose style cannot.
//! 3. When the selection ends, every overlay closes and the capture flow hears
//!    how: `capture::Message::Selected` with the rectangle or
//!    `capture::Message::WindowSelected` with the window on commit,
//!    `capture::Message::PickOnPlatform` on handing over,
//!    `capture::Message::SelectionCancelled` on Escape. An overlay closed any
//!    other way (by the OS) mid-selection cancels too.
//!
//! There is one session at a time: the capture flow starts no capture while one
//! is in progress. A finished session lingers only until its windows have
//! closed, so they keep showing the capture meanwhile; a new session replaces it.

use chartreuse_core::display::{DisplayId, DisplayLayout};
use chartreuse_core::flavor;
use chartreuse_core::geometry::LogicalPoint;
use chartreuse_core::window::WindowInfo;
use chartreuse_overlay::rectangle::{self, RectangleOverlay, Selection};
use chartreuse_overlay::setup::{self, OverlayWindows, Styled};
use chartreuse_overlay::shared;
use chartreuse_overlay::window::{self as window_selection, WindowOverlay, WindowSelection};
use iced::widget::image::Handle;
use iced::widget::space;
use iced::{window, Element, Subscription, Task};

use crate::app::{App, Message as AppMessage};
use crate::capture::{self, Snapshot};
use crate::theme;
use crate::windows::WindowKind;

/// This feature's part of the app state ([`App::overlay`]).
#[derive(Debug, Default)]
pub struct State {
    session: Option<Session>,
}

impl State {
    /// The display an open overlay window covers.
    #[must_use]
    pub fn display(&self, window: window::Id) -> Option<DisplayId> {
        self.session.as_ref()?.windows.display(window)
    }
}

/// How a selection's windows are selected, once Space switches to them.
#[derive(Debug, Clone)]
pub enum Windows {
    /// Over the overlays, among these windows listed with the capture (front to
    /// back).
    Listed(Vec<WindowInfo>),
    /// In the platform's own capture interface, which the overlays hand the
    /// capture over to.
    Platform,
    /// Not at all: the windows could not be listed.
    Unavailable,
}

/// The selection shared by every display's overlay, in global logical
/// coordinates: a rectangle, or a window once Space switches to them.
#[derive(Debug, Clone)]
pub struct Selector {
    /// The gesture the overlays show.
    gesture: Gesture,
    /// How to select windows.
    windows: Windows,
}

/// The selection gesture a [`Selector`] is in.
#[derive(Debug, Clone)]
enum Gesture {
    /// Drag out a rectangle.
    Rectangle(Selection),
    /// Click a window.
    Window(WindowSelection),
}

impl Selector {
    /// A rectangle selection over `layout`, which Space switches to selecting
    /// `windows`.
    const fn new(layout: DisplayLayout, windows: Windows) -> Self {
        Self {
            gesture: Gesture::Rectangle(Selection::new(layout)),
            windows,
        }
    }

    /// The displays the selection spans.
    const fn layout(&self) -> &DisplayLayout {
        match &self.gesture {
            Gesture::Rectangle(selection) => selection.layout(),
            Gesture::Window(selection) => selection.layout(),
        }
    }

    /// Feeds rectangle input to a rectangle selection, returning what to tell
    /// the capture flow if it ended the selection.
    fn rectangle(&mut self, input: rectangle::Input) -> Option<capture::Message> {
        if let rectangle::Input::Space(pointer) = input {
            return self.switch_to_windows(pointer);
        }
        let Gesture::Rectangle(selection) = &mut self.gesture else {
            return None;
        };
        selection.apply(input).map(|outcome| match outcome {
            rectangle::Outcome::Commit(rect) => capture::Message::Selected(rect),
            rectangle::Outcome::Cancel => capture::Message::SelectionCancelled,
        })
    }

    /// Feeds window input to a window selection, returning what to tell the
    /// capture flow if it ended the selection.
    fn window(&mut self, input: window_selection::Input) -> Option<capture::Message> {
        let Gesture::Window(selection) = &mut self.gesture else {
            return None;
        };
        if let window_selection::Input::Space(_) = input {
            if selection.phase() == window_selection::Phase::Selecting {
                tracing::debug!("switched to rectangle selection");
                self.gesture = Gesture::Rectangle(Selection::new(selection.layout().clone()));
            }
            return None;
        }
        selection.apply(input).map(|outcome| match outcome {
            window_selection::Outcome::Commit(id) => capture::Message::WindowSelected(id),
            window_selection::Outcome::Cancel => capture::Message::SelectionCancelled,
        })
    }

    /// Switches an idle rectangle selection (not mid-drag, not ended) to
    /// selecting windows, hovering the one under `pointer`. Returns what to
    /// tell the capture flow if that hands the capture over.
    fn switch_to_windows(&mut self, pointer: Option<LogicalPoint>) -> Option<capture::Message> {
        let Gesture::Rectangle(selection) = &mut self.gesture else {
            return None;
        };
        if selection.phase() != rectangle::Phase::Idle {
            return None;
        }
        match &self.windows {
            Windows::Listed(windows) => {
                tracing::debug!("switched to window selection");
                let mut windows = WindowSelection::new(selection.layout().clone(), windows.clone());
                if let Some(pointer) = pointer {
                    windows.move_to(pointer);
                }
                self.gesture = Gesture::Window(windows);
                None
            }
            Windows::Platform => {
                // Ended here, so that closing the overlays cancels nothing.
                selection.escape();
                Some(capture::Message::PickOnPlatform)
            }
            Windows::Unavailable => {
                tracing::debug!("no windows to select; staying with the rectangle");
                None
            }
        }
    }

    /// Cancels the selection if it has not ended yet, returning what to tell
    /// the capture flow if so.
    fn cancel(&mut self) -> Option<capture::Message> {
        let cancelled = match &mut self.gesture {
            Gesture::Rectangle(selection) => selection.escape().is_some(),
            Gesture::Window(selection) => selection.escape().is_some(),
        };
        cancelled.then_some(capture::Message::SelectionCancelled)
    }
}

/// One selection across every display's overlay window.
#[derive(Debug)]
struct Session {
    /// The selection, over the snapshot's layout.
    selector: Selector,
    /// Each display's frozen capture, in the layout's display order.
    images: Vec<Handle>,
    /// The overlay windows still open.
    windows: OverlayWindows,
}

impl Session {
    /// The overlay for `window`, if it is one of this session's.
    fn view(&self, window: window::Id) -> Option<Element<'_, AppMessage>> {
        let display = self.windows.display(window)?;
        let (info, image) = self
            .selector
            .layout()
            .displays()
            .iter()
            .zip(&self.images)
            .find(|(info, _)| info.id == display)?;
        let accent = theme::to_iced(flavor::ACCENT);
        Some(match &self.selector.gesture {
            Gesture::Rectangle(selection) => {
                RectangleOverlay::new(selection, info, image, accent, |input| {
                    AppMessage::Overlay(Message::Rectangle(input))
                })
                .view()
            }
            Gesture::Window(selection) => {
                WindowOverlay::new(selection, info, image, accent, |input| {
                    AppMessage::Overlay(Message::Window(input))
                })
                .view()
            }
        })
    }
}

/// This feature's messages ([`AppMessage::Overlay`]).
#[derive(Debug, Clone)]
pub enum Message {
    /// Open the selection overlays over a capture, selecting its windows as
    /// `Windows` says.
    Open(Snapshot, Windows),
    /// A capture's images are ready to draw, one per display in the selector's
    /// layout order: open its overlays.
    Frozen(Selector, Vec<Handle>),
    /// An overlay window was styled and shown.
    Styled(Styled),
    /// Pointer, Escape or Space input from a rectangle overlay.
    Rectangle(rectangle::Input),
    /// Pointer, Escape or Space input from a window overlay.
    Window(window_selection::Input),
}

pub fn boot(_app: &mut App) -> Task<AppMessage> {
    Task::none()
}

pub fn update(app: &mut App, message: Message) -> Task<AppMessage> {
    match message {
        Message::Open(snapshot, windows) => {
            freeze(Selector::new(snapshot.layout().clone(), windows), snapshot)
        }
        Message::Frozen(selector, images) => open(app, selector, images),
        Message::Styled(styled) => shown(app, styled),
        Message::Rectangle(input) => select(app, |selector| selector.rectangle(input)),
        Message::Window(input) => select(app, |selector| selector.window(input)),
    }
}

pub fn subscription(_app: &App) -> Subscription<AppMessage> {
    Subscription::none()
}

pub fn view(app: &App, window: window::Id) -> Element<'_, AppMessage> {
    app.overlay
        .session
        .as_ref()
        .and_then(|session| session.view(window))
        .unwrap_or_else(|| space().into())
}

pub fn window_closed(app: &mut App, window: window::Id) -> Task<AppMessage> {
    let Some(session) = &mut app.overlay.session else {
        return Task::none();
    };
    if session.windows.remove(window).is_none() {
        // One of a replaced session's windows.
        return Task::none();
    }
    // Closed from outside while still selecting: give up on the selection.
    let cancelled = session.selector.cancel().map(|reply| end(session, reply));
    if session.windows.is_empty() {
        app.overlay.session = None;
    }
    cancelled.unwrap_or_else(Task::none)
}

/// Makes image handles of `snapshot`'s captures off the main thread, then
/// reports them with `selector` as [`Message::Frozen`]. Each handle takes its
/// display's pixels by value, so every capture is copied: the capture flow
/// keeps the snapshot for cropping.
fn freeze(selector: Selector, snapshot: Snapshot) -> Task<AppMessage> {
    Task::perform(
        async move {
            snapshot
                .captures()
                .iter()
                .map(|capture| shared::frozen_image(capture.image.clone()))
                .collect()
        },
        move |images| AppMessage::Overlay(Message::Frozen(selector, images)),
    )
}

fn open(app: &mut App, selector: Selector, images: Vec<Handle>) -> Task<AppMessage> {
    let (windows, styled) =
        setup::open(selector.layout(), &app.platform.overlay_style, |settings| {
            app.windows.open(WindowKind::Overlay, settings)
        });
    tracing::debug!(overlays = windows.len(), "opened the overlays");
    let session = Session {
        selector,
        images,
        windows,
    };
    // A replaced session has ended and its windows are already closing.
    app.overlay.session = Some(session);
    styled.map(|styled| AppMessage::Overlay(Message::Styled(styled)))
}

/// An overlay was styled and shown: focus it if it covers the primary display.
fn shown(app: &App, Styled { window, result }: Styled) -> Task<AppMessage> {
    if let Err(error) = result {
        tracing::warn!(?window, %error, "could not style an overlay window");
    }
    let Some(session) = &app.overlay.session else {
        return Task::none();
    };
    let primary = session.selector.layout().primary().id;
    if session.windows.display(window) == Some(primary) {
        window::gain_focus(window)
    } else {
        Task::none()
    }
}

/// Feeds input to the session's selector with `apply`, ending the session if
/// the selection ended.
fn select(
    app: &mut App,
    apply: impl FnOnce(&mut Selector) -> Option<capture::Message>,
) -> Task<AppMessage> {
    let Some(session) = &mut app.overlay.session else {
        return Task::none();
    };
    apply(&mut session.selector).map_or_else(Task::none, |reply| end(session, reply))
}

/// Closes every overlay and tells the capture flow how the selection ended.
fn end(session: &Session, reply: capture::Message) -> Task<AppMessage> {
    Task::batch([
        session.windows.close_all(),
        Task::done(AppMessage::Capture(reply)),
    ])
}
