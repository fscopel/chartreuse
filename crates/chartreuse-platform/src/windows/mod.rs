//! The Windows backend: one file per platform trait, so parallel tracks never edit
//! the same file.
//!
//! Each trait file exports one backend type with a `new()` constructor. Keep that
//! constructor infallible and argument-free; do fallible setup lazily in the trait
//! methods.
//!
//! The trait files call Win32 and are compiled on Windows only. Their portable
//! logic (DPI math, coordinate conversion, …) lives in separate modules that are
//! also compiled into the test build on every host, so it is unit-tested
//! everywhere.

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod dialogs;
#[cfg(windows)]
mod displays;
#[cfg(windows)]
mod hotkeys;
#[cfg(windows)]
mod launch_at_login;
#[cfg(windows)]
mod overlay_style;
#[cfg(windows)]
mod permissions;
#[cfg(windows)]
mod status_item;
#[cfg(windows)]
mod window_list;

mod clipboard_data;
mod file_types;
#[cfg(windows)]
mod hidden_window;
mod ico;
mod keys;
mod layout;
mod pixels;
mod tray_menu;
#[cfg(windows)]
mod util;
mod window_filter;

#[cfg(windows)]
use std::sync::Arc;

#[cfg(windows)]
use crate::Platform;

/// Every Windows backend.
#[cfg(windows)]
pub fn platform() -> Platform {
    Platform {
        displays: Box::new(displays::WindowsDisplays::new()),
        capture: Box::new(capture::WindowsCapture::new()),
        window_list: Box::new(window_list::WindowsWindowList::new()),
        hotkeys: Box::new(hotkeys::WindowsHotkeys::new()),
        status_item: Box::new(status_item::WindowsStatusItem::new()),
        clipboard: Box::new(clipboard::WindowsClipboard::new()),
        file_dialogs: Box::new(dialogs::WindowsFileDialogs::new()),
        overlay_style: Arc::new(overlay_style::WindowsOverlayStyle::new()),
        permissions: Box::new(permissions::WindowsPermissions::new()),
        launch_at_login: Box::new(launch_at_login::WindowsLaunchAtLogin::new()),
    }
}
