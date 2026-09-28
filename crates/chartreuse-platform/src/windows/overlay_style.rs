//! Windows: overlay window styling.
//!
//! [`WindowsOverlayStyle::apply`] gives the iced window's `HWND`:
//!
//! - `WS_EX_TOPMOST`, applied with `SetWindowPos(HWND_TOPMOST)`, so it stays above
//!   other windows and the taskbar;
//! - `WS_EX_TOOLWINDOW` (and not `WS_EX_APPWINDOW`), so it has no taskbar button
//!   and is left out of Alt+Tab. The taskbar only notices the change when the
//!   window is shown, so a visible window is hidden and shown again around it;
//! - display affinity `WDA_EXCLUDEFROMCAPTURE` (Windows 10 2004 and later), so
//!   captures never contain an overlay. Older versions keep the overlay capturable,
//!   which only matters if another capture starts while one is on screen.

use std::ffi::c_void;

use ::windows::Win32::Foundation::{SetLastError, HWND, WIN32_ERROR};
use ::windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, IsWindowVisible, SetWindowDisplayAffinity, SetWindowLongW, SetWindowPos,
    ShowWindow, GWL_EXSTYLE, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SW_HIDE, SW_SHOW, WDA_EXCLUDEFROMCAPTURE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST,
};
use chartreuse_core::display::DisplayInfo;
use chartreuse_core::{Error, Result};
use raw_window_handle::RawWindowHandle;

use super::util::platform_error;
use crate::overlay_style::{NativeWindow, OverlayWindowStyle};

/// The Windows [`OverlayWindowStyle`] backend.
#[derive(Debug, Default)]
pub struct WindowsOverlayStyle;

impl WindowsOverlayStyle {
    pub fn new() -> Self {
        Self
    }
}

impl OverlayWindowStyle for WindowsOverlayStyle {
    fn apply(&self, window: NativeWindow<'_>, _display: &DisplayInfo) -> Result<()> {
        let RawWindowHandle::Win32(handle) = window.window.as_raw() else {
            return Err(Error::Platform("the overlay is not a Win32 window".into()));
        };
        let hwnd = HWND(handle.hwnd.get() as *mut c_void);
        // SAFETY: `hwnd` is the live window iced handed to `window::run`, on the
        // thread that owns it.
        unsafe {
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let styled = (ex_style | WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0;
            if styled != ex_style {
                let visible = IsWindowVisible(hwnd).as_bool();
                if visible {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                // SetWindowLongW returns the previous value, which may be 0, so
                // failure is told apart by the last error.
                SetLastError(WIN32_ERROR(0));
                let failure = (SetWindowLongW(hwnd, GWL_EXSTYLE, styled as i32) == 0)
                    .then(::windows::core::Error::from_thread)
                    .filter(|error| error.code().is_err());
                if visible {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                }
                if let Some(error) = failure {
                    return Err(platform_error("SetWindowLongW", &error));
                }
            }
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .map_err(|e| platform_error("SetWindowPos", &e))?;
            if let Err(error) = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) {
                tracing::debug!(%error, "overlay stays capturable (needs Windows 10 2004)");
            }
        }
        Ok(())
    }
}
