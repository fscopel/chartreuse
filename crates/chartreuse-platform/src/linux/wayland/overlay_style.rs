//! Wayland: overlay window styling, which the platform crate cannot do on
//! Wayland.
//!
//! A Wayland client cannot place its windows or raise them above others:
//! overlays need either a `wlr-layer-shell` surface (KDE, wlroots
//! compositors; not GNOME) on the overlay layer of each output, or, as the
//! fallback, a full-screen `xdg_toplevel` on each output. Both are roles the
//! window's toolkit assigns when it creates the surface, and winit (which iced
//! runs on) gives every window the plain `xdg_toplevel` role, exposes no
//! layer-shell, and requests full screen only through its own API. From the
//! raw `wl_surface` handle this style receives, neither can be had: a surface
//! keeps the role it was created with, and the `xdg_toplevel` object belongs
//! to winit's connection.
//!
//! The full-screen fallback is available to the app instead: showing an
//! overlay with iced's `window::Mode::Fullscreen` rather than
//! `Mode::Windowed`. iced cannot choose the output, though, so the
//! compositor puts every overlay on the same (focused) output, which serves
//! single-output desktops only. Until the overlay setup does that, `apply`
//! reports the gap and the overlay opens as an ordinary window.

use chartreuse_core::display::DisplayInfo;
use chartreuse_core::{Error, Result};

use crate::overlay_style::{NativeWindow, OverlayWindowStyle};

/// The Wayland [`OverlayWindowStyle`] backend.
#[derive(Debug, Default)]
pub struct WaylandOverlayStyle;

impl WaylandOverlayStyle {
    pub fn new() -> Self {
        Self
    }
}

impl OverlayWindowStyle for WaylandOverlayStyle {
    fn apply(&self, _window: NativeWindow<'_>, _display: &DisplayInfo) -> Result<()> {
        Err(Error::Unsupported(
            "overlay windows above other windows on Wayland (winit offers no layer-shell)",
        ))
    }
}
