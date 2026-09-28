//! The macOS backend: one file per platform trait, so parallel tracks never edit
//! the same file.
//!
//! Each file exports one backend type with a `new()` constructor. Keep that
//! constructor infallible and argument-free so this file never changes; do
//! fallible setup lazily in the trait methods. `cgimage` is the exception: it
//! holds no backend, only a conversion the backends share.

mod capture;
mod cgimage;
mod clipboard;
mod dialogs;
mod displays;
mod hotkeys;
mod launch_at_login;
mod overlay_style;
mod permissions;
mod status_item;
mod window_list;

use std::sync::Arc;

use crate::Platform;

/// Every macOS backend.
pub fn platform() -> Platform {
    Platform {
        displays: Box::new(displays::MacosDisplays::new()),
        capture: Box::new(capture::MacosCapture::new()),
        window_list: Box::new(window_list::MacosWindowList::new()),
        hotkeys: Box::new(hotkeys::MacosHotkeys::new()),
        status_item: Box::new(status_item::MacosStatusItem::new()),
        clipboard: Box::new(clipboard::MacosClipboard::new()),
        file_dialogs: Box::new(dialogs::MacosFileDialogs::new()),
        overlay_style: Arc::new(overlay_style::MacosOverlayStyle::new()),
        permissions: Box::new(permissions::MacosPermissions::new()),
        launch_at_login: Box::new(launch_at_login::MacosLaunchAtLogin::new()),
    }
}
