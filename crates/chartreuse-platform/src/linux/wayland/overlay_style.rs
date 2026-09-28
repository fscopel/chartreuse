//! Wayland: overlay windows as full-screen `xdg_toplevel`s.
//!
//! A Wayland client can neither place its windows nor raise them above
//! others. The overlays PLAN.md asks for are `wlr-layer-shell` surfaces (KDE,
//! wlroots compositors; not GNOME) on the overlay layer of each output, with
//! a full-screen `xdg_toplevel` per output as the fallback. Both are roles the
//! window's toolkit assigns when it creates the surface, and winit (which iced
//! runs on) gives every window the plain `xdg_toplevel` role and exposes no
//! layer-shell. From the raw `wl_surface` handle this style receives, neither
//! role can be changed: a surface keeps the role it was created with, and the
//! `xdg_toplevel` object belongs to winit's connection.
//!
//! So the overlays take the fallback, through iced: they open and are shown
//! full screen ([`OverlayPlacement::Fullscreen`]), which compositors stack
//! above panels and other windows, and there is nothing left to apply. iced
//! cannot choose the output of a full-screen window, so the compositor puts
//! every overlay on the same one (usually the focused output): overlays
//! serve single-output desktops only, and the app has rectangle captures on
//! several outputs picked through the Screenshot portal instead (see
//! [`WaylandCapture`](super::capture::WaylandCapture)).

use chartreuse_core::display::DisplayInfo;
use chartreuse_core::Result;

use crate::overlay_style::{NativeWindow, OverlayPlacement, OverlayWindowStyle};

/// The Wayland [`OverlayWindowStyle`] backend.
#[derive(Debug, Default)]
pub struct WaylandOverlayStyle;

impl WaylandOverlayStyle {
    pub fn new() -> Self {
        Self
    }
}

impl OverlayWindowStyle for WaylandOverlayStyle {
    fn placement(&self) -> OverlayPlacement {
        OverlayPlacement::Fullscreen
    }

    fn apply(&self, _window: NativeWindow<'_>, _display: &DisplayInfo) -> Result<()> {
        Ok(())
    }
}
