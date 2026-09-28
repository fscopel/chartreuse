//! Platform traits and per-OS backends.
//!
//! # Layout
//!
//! - One trait per concern, each in its own module: [`Displays`], [`Capture`],
//!   [`WindowList`], [`Hotkeys`], [`StatusItem`], [`Clipboard`], [`FileDialogs`],
//!   [`OverlayWindowStyle`], [`Permissions`], [`LaunchAtLogin`].
//! - One backend directory per platform — `macos/`, `windows/`, `linux/x11/`,
//!   `linux/wayland/` — with **one file per trait** (`displays.rs`, `capture.rs`,
//!   `window_list.rs`, `hotkeys.rs`, `status_item.rs`, `clipboard.rs`,
//!   `dialogs.rs`, `overlay_style.rs`, `permissions.rs`, `launch_at_login.rs`),
//!   so parallel tracks never edit the same file. (The two Linux backends share
//!   `linux/launch_at_login.rs`: XDG autostart is the same on X11 and Wayland.)
//!   Each file exports one type with an infallible `new()`; the directory's
//!   `mod.rs` assembles them into a [`Platform`] and never needs to change.
//! - [`fake`]: a synthetic backend for tests and UI work.
//!
//! Backends that are not implemented yet fail every call with
//! [`Error::Unsupported`](chartreuse_core::Error::Unsupported), so every target
//! compiles and the app can report the gap instead of crashing.
//!
//! # Threading
//!
//! On macOS, iced/winit owns the main thread and its run loop. iced's `boot`,
//! `update`, and `view` — and the callback of `iced::window::run` — execute on that
//! thread; `Task` futures and `Subscription` streams execute on a thread pool.
//!
//! - Call every trait method from `boot` or `update` (the main thread). Several
//!   require it (`NSStatusItem`, Carbon hotkeys, `NSScreen`, `NSOpenPanel`).
//! - Do AppKit UI setup — [`StatusItem::install`], and anything else that needs
//!   the running `NSApplication` run loop — in `update`, never in `boot`: iced
//!   0.14 calls `boot` before the event loop starts. To do it at startup, have
//!   `boot` return `Task::done` with an install message (for the tray,
//!   `Task::done(Message::Tray(tray::Message::Install))`) and install when
//!   `update` receives it; tasks from `boot` are delivered only once the event
//!   loop is running.
//! - Slow operations (capture, window enumeration, dialogs) return a
//!   `BoxFuture<'static, _>` that is `Send`: start them in `update` and hand the
//!   future to `Task::perform`. The backend itself hops to whatever thread its OS
//!   API needs.
//! - [`OverlayWindowStyle::apply`] runs inside the `iced::window::run` callback,
//!   which is on the main thread but must be `Send`, so that trait is
//!   `Send + Sync` and held in an [`Arc`].
//! - Registrations ([`event::Registration`]) are `!Send` and are dropped on the
//!   main thread along with the app state.
//!
//! # Event delivery
//!
//! Sources of spontaneous events — [`StatusItem`] menu choices and [`Hotkeys`]
//! presses — hand back an [`event::EventReceiver`] together with a
//! [`event::Registration`]:
//!
//! 1. In `update` (at startup, via a message from `boot`; see
//!    [Threading](#threading)), call [`StatusItem::install`] or
//!    [`Hotkeys::register`]. Store the returned handle (receiver + registration)
//!    in the feature's state.
//! 2. The OS callback, on the main thread, pushes each event through the
//!    backend's [`event::EventSender`] without blocking.
//! 3. The feature's `subscription()` turns the stored receiver into an iced
//!    `Subscription` with the app crate's `events::subscription(&receiver)`, which
//!    keys the subscription on the receiver's identity, so it lives exactly as
//!    long as the receiver stays in the state.
//! 4. Dropping the handle (for example to re-register hotkeys) unregisters with the
//!    OS and ends the subscription.

pub mod capture;
pub mod clipboard;
pub mod dialogs;
pub mod displays;
pub mod event;
pub mod fake;
pub mod hotkeys;
pub mod launch_at_login;
pub mod overlay_style;
pub mod permissions;
pub mod status_item;
pub mod window_list;

#[cfg(target_os = "macos")]
mod macos;

// On other hosts only its portable helpers are compiled, for their unit tests.
#[cfg(any(target_os = "windows", test))]
mod windows;

// On other hosts only its portable logic is compiled, for its unit tests.
#[cfg(any(all(unix, not(target_os = "macos")), test))]
mod linux;

#[cfg(not(any(unix, windows)))]
compile_error!("Chartreuse supports macOS, Windows, Linux, and other Unix desktops only");

use std::sync::Arc;

pub use capture::{Capture, DisplayCapture};
pub use clipboard::Clipboard;
pub use dialogs::{FileDialogs, OpenImageRequest, SaveImageRequest};
pub use displays::Displays;
pub use event::{EventReceiver, EventSender, Registration};
pub use hotkeys::{HotkeyBinding, HotkeyEvent, HotkeyRegistration, Hotkeys};
pub use launch_at_login::LaunchAtLogin;
pub use overlay_style::{NativeWindow, OverlayPlacement, OverlayWindowStyle};
pub use permissions::Permissions;
pub use status_item::{MenuAction, MenuEntry, StatusItem, StatusItemHandle, MENU};
pub use window_list::WindowList;

/// One implementation of every platform trait.
///
/// The fields are independent, so a `Platform` can mix backends (for example fake
/// capture with the real status item) during development.
pub struct Platform {
    pub displays: Box<dyn Displays>,
    pub capture: Box<dyn Capture>,
    pub window_list: Box<dyn WindowList>,
    pub hotkeys: Box<dyn Hotkeys>,
    pub status_item: Box<dyn StatusItem>,
    pub clipboard: Box<dyn Clipboard>,
    pub file_dialogs: Box<dyn FileDialogs>,
    pub overlay_style: Arc<dyn OverlayWindowStyle>,
    pub permissions: Box<dyn Permissions>,
    pub launch_at_login: Box<dyn LaunchAtLogin>,
}

impl std::fmt::Debug for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Platform").finish_non_exhaustive()
    }
}

/// The backend for the platform this binary was compiled for (on Linux, for the
/// current X11 or Wayland session).
#[must_use]
pub fn current() -> Platform {
    #[cfg(target_os = "macos")]
    return macos::platform();

    #[cfg(target_os = "windows")]
    return windows::platform();

    #[cfg(all(unix, not(target_os = "macos")))]
    return linux::platform();
}
