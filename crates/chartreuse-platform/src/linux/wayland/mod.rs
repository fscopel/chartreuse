//! The Wayland backend: one file per platform trait, so parallel tracks never edit
//! the same file.
//!
//! Each file exports one backend type with a `new()` constructor. Keep that
//! constructor infallible and argument-free so this file never changes; do
//! fallible setup lazily in the trait methods.

mod capture;
mod clipboard;
mod dialogs;
mod displays;
mod hotkeys;
mod overlay_style;
mod permissions;
mod status_item;
mod window_list;

use std::sync::Arc;

use crate::Platform;

/// Every Wayland backend.
pub fn platform() -> Platform {
    Platform {
        displays: Box::new(displays::WaylandDisplays::new()),
        capture: Box::new(capture::WaylandCapture::new()),
        window_list: Box::new(window_list::WaylandWindowList::new()),
        hotkeys: Box::new(hotkeys::WaylandHotkeys::new()),
        status_item: Box::new(status_item::WaylandStatusItem::new()),
        clipboard: Box::new(clipboard::WaylandClipboard::new()),
        file_dialogs: Box::new(dialogs::WaylandFileDialogs::new()),
        overlay_style: Arc::new(overlay_style::WaylandOverlayStyle::new()),
        permissions: Box::new(permissions::WaylandPermissions::new()),
        launch_at_login: Box::new(crate::linux::launch_at_login::LinuxLaunchAtLogin::new()),
    }
}
