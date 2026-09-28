//! Overlay window setup: one borderless iced window per display, placed with the
//! display model and styled with
//! [`OverlayWindowStyle`](chartreuse_platform::OverlayWindowStyle).
//!
//! [`open`] opens every overlay through a caller-supplied window opener (the
//! app's `WindowRegistry`, or plain `iced::window::open`), remembers which
//! display each window covers ([`OverlayWindows`]), and returns a task that
//! styles and then shows each window as it opens, reporting a [`Styled`] per
//! window:
//!
//! ```ignore
//! let (overlays, task) = setup::open(&layout, &app.platform.overlay_style, |settings| {
//!     app.windows.open(WindowKind::Overlay, settings)
//! });
//! app.overlay.windows = overlays;
//! task.map(|styled| AppMessage::Overlay(overlay::Message::Styled(styled)))
//! ```
//!
//! # Placement
//!
//! `iced::window::Position::Specific` is a logical position in the global
//! desktop, which on macOS is exactly Chartreuse's logical space: iced hands it to
//! winit as a `LogicalPosition` for the window's outer top-left corner, and winit
//! flips it against the height of the main display (the one with the menu bar,
//! whose top-left corner is the origin) into AppKit's bottom-up screen
//! coordinates, the inverse of the display model's flip. Negative coordinates
//! (displays above or left of the primary) pass through unchanged. So an overlay
//! is placed at its display's logical origin and sized to its logical size, with
//! no conversion beyond `f64` to `f32` ([`position`], [`size`]). Borderless
//! windows have no frame, so outer and inner geometry coincide.
//!
//! This relies on the app's iced scale factor being 1 (the default): iced scales
//! a new window's size, but not its position, by that factor.
//!
//! # Showing
//!
//! Overlays open hidden, so they never appear as ordinary windows below the
//! menu bar, and are shown once the platform style has been applied: the style
//! raises them above the menu bar and Dock and, on macOS, activates the app and
//! makes the window key so Escape reaches it. A window whose style fails is
//! still shown, at iced's always-on-top level, and the failure is reported in
//! its [`Styled`] for the caller to log.

use std::sync::Arc;

use chartreuse_core::display::{DisplayId, DisplayInfo, DisplayLayout};
use chartreuse_core::geometry::LogicalRect;
use chartreuse_core::Result;
use chartreuse_platform::{NativeWindow, OverlayWindowStyle};
use iced::{window, Point, Size, Task};

/// The overlay windows that are open, and the display each one covers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlayWindows {
    windows: Vec<(window::Id, DisplayId)>,
}

impl OverlayWindows {
    /// The display that `window` covers, if it is an overlay.
    #[must_use]
    pub fn display(&self, window: window::Id) -> Option<DisplayId> {
        self.windows
            .iter()
            .find(|(id, _)| *id == window)
            .map(|(_, display)| *display)
    }

    /// The overlay window covering `display`.
    #[must_use]
    pub fn window(&self, display: DisplayId) -> Option<window::Id> {
        self.windows
            .iter()
            .find(|(_, id)| *id == display)
            .map(|(window, _)| *window)
    }

    /// Every overlay window with its display, in the layout's display order.
    pub fn iter(&self) -> impl Iterator<Item = (window::Id, DisplayId)> + '_ {
        self.windows.iter().copied()
    }

    /// Forgets a closed overlay window, returning the display it covered.
    pub fn remove(&mut self, window: window::Id) -> Option<DisplayId> {
        let index = self.windows.iter().position(|(id, _)| *id == window)?;
        Some(self.windows.remove(index).1)
    }

    /// Closes every overlay window. Each close is reported like any other
    /// (`iced::window::close_events`), so remove the windows as they close.
    pub fn close_all<T: Send + 'static>(&self) -> Task<T> {
        Task::batch(self.windows.iter().map(|(id, _)| window::close(*id)))
    }

    /// The number of overlay windows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

/// The outcome of styling one overlay window. The window is shown either way.
#[derive(Debug, Clone)]
pub struct Styled {
    pub window: window::Id,
    pub result: Result<()>,
}

/// Opens one overlay window per display of `layout` with `open_window`, which
/// opens a window with the given settings and returns its id and the task that
/// opens it (`WindowRegistry::open` for `WindowKind::Overlay` in the app;
/// `iced::window::open` also fits). Returns the windows and a task that styles
/// each with `style` and shows it once it has opened, yielding one [`Styled`]
/// per window.
pub fn open(
    layout: &DisplayLayout,
    style: &Arc<dyn OverlayWindowStyle>,
    mut open_window: impl FnMut(window::Settings) -> (window::Id, Task<window::Id>),
) -> (OverlayWindows, Task<Styled>) {
    let mut windows = Vec::with_capacity(layout.displays().len());
    let mut tasks = Vec::with_capacity(layout.displays().len());
    for display in layout.displays() {
        let (id, opened) = open_window(settings(display));
        windows.push((id, display.id));
        let style = Arc::clone(style);
        let display = display.clone();
        tasks.push(opened.then(move |id| apply_style(id, Arc::clone(&style), display.clone())));
    }
    (OverlayWindows { windows }, Task::batch(tasks))
}

/// Styles the open window `id`, which covers `display`, with `style` on the
/// main thread, then shows it.
pub fn apply_style(
    id: window::Id,
    style: Arc<dyn OverlayWindowStyle>,
    display: DisplayInfo,
) -> Task<Styled> {
    window::run(id, move |window| {
        NativeWindow::from_window(window).and_then(|native| style.apply(native, &display))
    })
    .then(move |result| {
        window::set_mode(id, window::Mode::Windowed)
            .chain(Task::done(Styled { window: id, result }))
    })
}

/// The settings of the overlay window for `display`: borderless, fixed, placed
/// over the display's logical bounds, always on top (the platform style raises
/// it further), hidden until styled, and not quitting the app when closed.
///
/// Opaque: overlays draw the frozen capture over the whole window, so there is
/// nothing behind them to show through.
#[must_use]
pub fn settings(display: &DisplayInfo) -> window::Settings {
    let bounds = &display.logical_bounds;
    window::Settings {
        size: size(bounds),
        position: position(bounds),
        visible: false,
        resizable: false,
        closeable: false,
        minimizable: false,
        decorations: false,
        transparent: false,
        level: window::Level::AlwaysOnTop,
        exit_on_close_request: false,
        platform_specific: platform_specific(),
        ..window::Settings::default()
    }
}

/// The iced window position that puts a window's top-left corner at the
/// top-left corner of `bounds` (global logical coordinates).
#[must_use]
pub fn position(bounds: &LogicalRect) -> window::Position {
    window::Position::Specific(Point::new(bounds.origin.x as f32, bounds.origin.y as f32))
}

/// The iced window size of `bounds` (logical points).
#[must_use]
pub fn size(bounds: &LogicalRect) -> Size {
    Size::new(bounds.size.width as f32, bounds.size.height as f32)
}

/// No taskbar button on Windows. (macOS has no per-window equivalent: the Dock
/// icon belongs to the app, which runs as an accessory.)
#[cfg(target_os = "windows")]
fn platform_specific() -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific {
        skip_taskbar: true,
        ..window::settings::PlatformSpecific::default()
    }
}

#[cfg(not(target_os = "windows"))]
fn platform_specific() -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific::default()
}

#[cfg(test)]
mod tests {
    use chartreuse_platform::fake::{self, Fake};

    use super::*;

    fn layout() -> DisplayLayout {
        DisplayLayout::new(fake::default_displays()).unwrap()
    }

    /// The logical rectangle an overlay window with `settings` covers.
    fn covered(settings: &window::Settings) -> LogicalRect {
        let window::Position::Specific(origin) = settings.position else {
            panic!(
                "overlays have a specific position, not {:?}",
                settings.position
            );
        };
        LogicalRect::new(
            f64::from(origin.x),
            f64::from(origin.y),
            f64::from(settings.size.width),
            f64::from(settings.size.height),
        )
    }

    #[test]
    fn each_overlay_covers_exactly_its_display_including_negative_origins() {
        for display in layout().displays() {
            assert_eq!(
                covered(&settings(display)),
                display.logical_bounds,
                "{}",
                display.name
            );
        }
        // The fake desktop has a display left of and above the primary.
        assert!(layout()
            .displays()
            .iter()
            .any(|d| d.logical_bounds.origin.x < 0.0 && d.logical_bounds.origin.y < 0.0));
    }

    #[test]
    fn overlay_settings_start_hidden() {
        assert!(!settings(&layout().displays()[0]).visible);
    }
    #[test]
    fn open_opens_one_window_per_display_and_maps_it_back() {
        let layout = layout();
        let style = Fake::new().platform().overlay_style;
        let mut opened = Vec::new();
        let (windows, _task) = open(&layout, &style, |settings| {
            opened.push(covered(&settings));
            window::open(settings)
        });

        let bounds: Vec<_> = layout.displays().iter().map(|d| d.logical_bounds).collect();
        assert_eq!(opened, bounds, "one window per display, in layout order");
        assert_eq!(windows.len(), layout.displays().len());
        for ((window, display), info) in windows.iter().zip(layout.displays()) {
            assert_eq!(display, info.id);
            assert_eq!(windows.display(window), Some(info.id));
            assert_eq!(windows.window(info.id), Some(window));
        }
    }

    #[test]
    fn removing_a_window_forgets_only_that_one() {
        let layout = layout();
        let style = Fake::new().platform().overlay_style;
        let (mut windows, _task) = open(&layout, &style, window::open);
        let (first, display) = windows.iter().next().unwrap();

        assert_eq!(windows.remove(first), Some(display));
        assert_eq!(windows.remove(first), None, "a window closes only once");
        assert_eq!(windows.display(first), None);
        assert_eq!(windows.window(display), None);
        assert_eq!(windows.len(), layout.displays().len() - 1);
    }
}
