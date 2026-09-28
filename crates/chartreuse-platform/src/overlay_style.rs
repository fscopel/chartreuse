//! Native styling for selection overlay windows.

use chartreuse_core::display::DisplayInfo;
use chartreuse_core::error::Error;
use chartreuse_core::Result;
use raw_window_handle::{DisplayHandle, HasDisplayHandle, HasWindowHandle, WindowHandle};

/// Borrowed native handles of one iced window.
#[derive(Debug, Clone, Copy)]
pub struct NativeWindow<'a> {
    pub window: WindowHandle<'a>,
    pub display: DisplayHandle<'a>,
}

impl<'a> NativeWindow<'a> {
    /// Borrows the handles of anything that has them, such as the `&dyn Window`
    /// that `iced::window::run` passes to its callback.
    pub fn from_window<W>(window: &'a W) -> Result<Self>
    where
        W: HasWindowHandle + HasDisplayHandle + ?Sized,
    {
        let unavailable = |e| Error::Platform(format!("native window handle unavailable: {e}"));
        Ok(Self {
            window: window.window_handle().map_err(unavailable)?,
            display: window.display_handle().map_err(unavailable)?,
        })
    }
}

/// Turns an ordinary iced window into a selection overlay: above every other
/// window including the menu bar, Dock, and taskbar; present on the active Space
/// and over full-screen apps; and without a taskbar entry.
///
/// `Send + Sync` because the app calls it from inside the `Send` callback of
/// `iced::window::run`, which executes on the main thread.
pub trait OverlayWindowStyle: Send + Sync {
    /// How overlay windows are placed on this platform. Every platform but
    /// Wayland places them over their displays (the default).
    fn placement(&self) -> OverlayPlacement {
        OverlayPlacement::OverDisplay
    }

    /// Applies the overlay style to the window that covers `display`, and
    /// corrects its placement where iced's logical position can miss the
    /// display (Windows with mixed DPI). **Main thread only.**
    fn apply(&self, window: NativeWindow<'_>, display: &DisplayInfo) -> Result<()>;
}

/// How the app places overlay windows ([`OverlayWindowStyle::placement`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayPlacement {
    /// A window over the display's bounds, which iced places at the display's
    /// logical position and the style may correct.
    OverDisplay,
    /// A full-screen window, on the output the compositor chooses: Wayland
    /// lets clients neither position windows nor, through winit, pick the
    /// output of a full-screen one. So overlays can cover a desktop of one
    /// display only.
    Fullscreen,
}
