//! Linux and other free Unix desktops: an X11 and a Wayland backend, chosen at
//! runtime from the session type.
//!
//! Both backends share the tray ([`tray`], a StatusNotifierItem), the file
//! dialogs ([`file_chooser`], the FileChooser portal), the clipboard
//! ([`clipboard`]), and launch at login ([`launch_at_login`], an XDG autostart
//! entry). The backends' pure logic lives in [`logic`], which uses
//! no Linux-only crate: test builds on other hosts compile it (and the session
//! detection here), so its unit tests run on every development machine.
//!
//! # Limitations
//!
//! Known from the protocols and libraries the backends use; not yet checked
//! on every desktop.
//!
//! - Tray: needs a StatusNotifierItem host. KDE Plasma, Xfce, Cinnamon, MATE,
//!   LXQt, Budgie, and Waybar have one; GNOME needs the AppIndicator
//!   extension. There is no fallback to the older XEmbed tray.
//! - X11 scaling: the desktop has one scale factor (`Xft.dpi`, as every
//!   major desktop sets it). Without a DPI setting winit derives a factor per
//!   monitor from its physical size; the display model then uses the primary
//!   monitor's for all, and overlays on monitors whose factor differs are
//!   misplaced.
//! - X11 window capture: without a compositing manager, a window is read
//!   from the screen, so whatever covers it is captured too.
//! - Wayland overlays: winit offers no layer-shell, so overlays are
//!   full-screen windows (see [`wayland`]'s overlay style) on the output the
//!   compositor chooses, which covers single-output desktops only. Rectangle
//!   captures on several outputs use the Screenshot portal's interactive
//!   picker instead.
//! - Wayland windows: clients cannot list other clients' windows, so window
//!   captures use the Screenshot portal's interactive picker too (GNOME,
//!   KDE; wlroots portals capture the whole desktop).
//! - Wayland screen capture: needs the Screenshot portal (GNOME, KDE, or
//!   xdg-desktop-portal-wlr on wlroots compositors), whose first use asks the
//!   user. The screenshot is assumed to cover the logical layout at one scale,
//!   as GNOME's and grim's do.
//! - Wayland hotkeys: need the GlobalShortcuts portal (GNOME 48 and later,
//!   KDE Plasma, Hyprland; not xdg-desktop-portal-wlr), which may ask the user
//!   to confirm or change the triggers. Failures are only logged.
//! - Wayland clipboard: the data-control protocols (KDE, wlroots); on GNOME
//!   through XWayland.

#[cfg(all(unix, not(target_os = "macos")))]
mod blocking;
#[cfg(all(unix, not(target_os = "macos")))]
mod clipboard;
#[cfg(all(unix, not(target_os = "macos")))]
mod file_chooser;
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
mod launch_at_login;
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
mod logic;
#[cfg(all(unix, not(target_os = "macos")))]
mod portal;
#[cfg(all(unix, not(target_os = "macos")))]
mod tray;
#[cfg(all(unix, not(target_os = "macos")))]
mod wayland;
#[cfg(all(unix, not(target_os = "macos")))]
mod x11;

use std::ffi::OsStr;

#[cfg(all(unix, not(target_os = "macos")))]
use crate::Platform;

/// The kind of graphical session Chartreuse runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Session {
    X11,
    Wayland,
}

impl Session {
    /// Wayland if `WAYLAND_DISPLAY` or `WAYLAND_SOCKET` is set (and not
    /// empty), else X11, which covers XWayland too. That is winit's own rule,
    /// and the backend has to match winit's: the overlays and editors are
    /// winit windows. So a Wayland session (`XDG_SESSION_TYPE`) whose
    /// `WAYLAND_DISPLAY` was unset, to run the app through XWayland, gets X11.
    fn detect(wayland_display: Option<&OsStr>, wayland_socket: Option<&OsStr>) -> Self {
        let set = |value: Option<&OsStr>| value.is_some_and(|value| !value.is_empty());
        if set(wayland_display) || set(wayland_socket) {
            Self::Wayland
        } else {
            Self::X11
        }
    }
}

/// The backend for the current session.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn platform() -> Platform {
    let wayland_display = std::env::var_os("WAYLAND_DISPLAY");
    let wayland_socket = std::env::var_os("WAYLAND_SOCKET");
    match Session::detect(wayland_display.as_deref(), wayland_socket.as_deref()) {
        Session::X11 => x11::platform(),
        Session::Wayland => wayland::platform(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(wayland_display: Option<&str>, wayland_socket: Option<&str>) -> Session {
        Session::detect(
            wayland_display.map(OsStr::new),
            wayland_socket.map(OsStr::new),
        )
    }

    #[test]
    fn a_wayland_display_or_socket_selects_wayland() {
        assert_eq!(detect(Some("wayland-0"), None), Session::Wayland);
        assert_eq!(detect(None, Some("3")), Session::Wayland);
    }

    #[test]
    fn everything_else_falls_back_to_x11() {
        assert_eq!(detect(None, None), Session::X11);
        assert_eq!(detect(Some(""), Some("")), Session::X11);
    }
}
