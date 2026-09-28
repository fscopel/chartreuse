//! The X11 backend: one file per platform trait, so parallel tracks never edit
//! the same file.
//!
//! Each file exports one backend type with a `new()` constructor. Keep that
//! constructor infallible and argument-free so this file never changes; do
//! fallible setup lazily in the trait methods.

mod capture;
mod clipboard;
mod connection;
mod dialogs;
mod displays;
mod hotkeys;
mod image;
mod overlay_style;
mod permissions;
mod status_item;
mod window_list;

use std::sync::Arc;

use crate::Platform;

/// Every X11 backend.
pub fn platform() -> Platform {
    Platform {
        displays: Box::new(displays::X11Displays::new()),
        capture: Box::new(capture::X11Capture::new()),
        window_list: Box::new(window_list::X11WindowList::new()),
        hotkeys: Box::new(hotkeys::X11Hotkeys::new()),
        status_item: Box::new(status_item::X11StatusItem::new()),
        clipboard: Box::new(clipboard::X11Clipboard::new()),
        file_dialogs: Box::new(dialogs::X11FileDialogs::new()),
        overlay_style: Arc::new(overlay_style::X11OverlayStyle::new()),
        permissions: Box::new(permissions::X11Permissions::new()),
        launch_at_login: Box::new(crate::linux::launch_at_login::LinuxLaunchAtLogin::new()),
    }
}
