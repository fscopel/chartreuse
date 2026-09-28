//! X11: overlay windows as override-redirect windows.
//!
//! The window manager leaves override-redirect windows alone: no frame, no
//! taskbar entry, no placement of its own, and no stacking below panels or
//! full-screen windows. So an overlay stays exactly where iced placed it (its
//! display's area), above everything once raised. The flip side is that the
//! window manager does not give it the keyboard focus either, so a watcher
//! thread raises and focuses each overlay the moment it is mapped, and Escape
//! reaches it.
//!
//! The style must be applied before the window is first shown: the X server
//! reads the attribute when the window is mapped.

use std::thread;

use chartreuse_core::display::DisplayInfo;
use chartreuse_core::{Error, Result};
use raw_window_handle::RawWindowHandle;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _, EventMask, InputFocus,
    StackMode, Window,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use super::connection::{self, failed};
use crate::overlay_style::{NativeWindow, OverlayWindowStyle};

/// The X11 [`OverlayWindowStyle`] backend.
#[derive(Debug, Default)]
pub struct X11OverlayStyle;

impl X11OverlayStyle {
    pub fn new() -> Self {
        Self
    }
}

impl OverlayWindowStyle for X11OverlayStyle {
    fn apply(&self, native: NativeWindow<'_>, _display: &DisplayInfo) -> Result<()> {
        let window = match native.window.as_raw() {
            RawWindowHandle::Xlib(handle) => Window::try_from(handle.window)
                .map_err(|_| Error::Platform(format!("invalid X11 window {}", handle.window)))?,
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            other => {
                return Err(Error::Platform(format!(
                    "the overlay is not an X11 window: {other:?}"
                )));
            }
        };
        let x11 = connection::get()?;
        let what = "making the overlay override-redirect";
        x11.conn
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().override_redirect(1),
            )
            .map_err(|e| failed(what, e))?
            .check()
            .map_err(|e| failed(what, e))?;
        focus_when_mapped(window)
    }
}

/// Raises `window` and gives it the keyboard focus as soon as it is mapped,
/// from a thread with its own connection (whose events are only this
/// window's). The thread ends then, or when the window is destroyed first.
fn focus_when_mapped(window: Window) -> Result<()> {
    let what = "watching the overlay";
    let (conn, _) = x11rb::connect(None).map_err(|e| failed(what, e))?;
    // Event masks are per client: this selects the notifications for the new
    // connection only, leaving winit's own selection alone.
    conn.change_window_attributes(
        window,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
    )
    .map_err(|e| failed(what, e))?
    .check()
    .map_err(|e| failed(what, e))?;
    thread::Builder::new()
        .name("chartreuse overlay focus".into())
        .spawn(move || watch(&conn, window))
        .map_err(|e| failed(what, e))?;
    Ok(())
}

fn watch(conn: &RustConnection, window: Window) {
    loop {
        match conn.wait_for_event() {
            Ok(Event::MapNotify(event)) if event.window == window => {
                let raised = conn
                    .configure_window(
                        window,
                        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
                    )
                    .map(drop);
                let focused = conn
                    .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)
                    .map(drop);
                if let Err(error) = raised.and(focused).and_then(|()| conn.flush()) {
                    tracing::warn!("could not raise and focus the overlay: {error}");
                }
                return;
            }
            Ok(Event::DestroyNotify(event)) if event.window == window => return,
            Ok(_) => {}
            Err(error) => {
                tracing::warn!("lost the X server while waiting for the overlay: {error}");
                return;
            }
        }
    }
}
