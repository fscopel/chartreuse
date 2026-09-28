//! A synthetic backend for tests and for UI work without real capture.
//!
//! [`Fake`] implements every platform trait against an in-memory world: three
//! displays with mixed scale factors (one with a negative origin), a few
//! overlapping windows, generated capture images, a clipboard, and scripted dialog
//! answers. A `Fake` is a cheap handle to shared state, so a test can keep one
//! clone to drive events (press a hotkey, pick a menu item) while the code under
//! test uses the [`Platform`] made by [`Fake::platform`].
//!
//! The app uses this backend when started with `CHARTREUSE_BACKEND=fake`.

mod capture;
mod clipboard;
mod dialogs;
mod displays;
mod hotkeys;
mod launch_at_login;
mod overlay_style;
mod permissions;
mod status_item;
mod window_list;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use chartreuse_core::color::Rgba8;
use chartreuse_core::display::{DisplayId, DisplayInfo};
use chartreuse_core::geometry::{LogicalRect, PhysicalSize, ScaleFactor};
use chartreuse_core::hotkey::Hotkey;
use chartreuse_core::image::Image;
use chartreuse_core::permission::{Permission, PermissionStatus};
use chartreuse_core::window::{WindowId, WindowInfo, WindowOwner};
use chartreuse_core::Error;
use parking_lot::Mutex;

use crate::dialogs::SaveImageRequest;
use crate::event::EventSender;
use crate::hotkeys::{HotkeyBinding, HotkeyEvent};
use crate::status_item::MenuAction;
use crate::Platform;

/// A handle to one synthetic world. Clones share the world.
#[derive(Debug, Clone, Default)]
pub struct Fake {
    state: Arc<Mutex<State>>,
}

#[derive(Debug)]
struct State {
    displays: Vec<DisplayInfo>,
    windows: Vec<WindowInfo>,
    clipboard: Option<Image>,
    screen_recording: PermissionStatus,
    grant_on_request: bool,
    opened_settings: Vec<Permission>,
    /// Combinations "owned by another program": registering them fails.
    reserved_hotkeys: HashSet<Hotkey>,
    /// Live hotkey registrations, keyed by a unique generation number.
    hotkeys: Vec<(u64, Vec<HotkeyBinding>, EventSender<HotkeyEvent>)>,
    /// The installed status item, keyed like `hotkeys`. Installing again replaces
    /// it; dropping a replaced handle leaves the newer one alone.
    menu: Option<(u64, EventSender<MenuAction>)>,
    generation: u64,
    open_answer: Option<PathBuf>,
    save_answer: Option<PathBuf>,
    save_requests: Vec<SaveImageRequest>,
    overlays_styled: usize,
    /// Whether "the OS" starts the app at login.
    launch_at_login: bool,
    /// While set, `LaunchAtLogin::set` fails with this, changing nothing.
    launch_at_login_refusal: Option<Error>,
    /// While set, `LaunchAtLogin::status` fails with this.
    launch_at_login_unreadable: Option<Error>,
    /// Every value given to `LaunchAtLogin::set` that took, in order.
    launch_at_login_sets: Vec<bool>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            displays: default_displays(),
            windows: default_windows(),
            clipboard: None,
            screen_recording: PermissionStatus::Granted,
            grant_on_request: true,
            opened_settings: Vec::new(),
            reserved_hotkeys: HashSet::new(),
            hotkeys: Vec::new(),
            menu: None,
            generation: 0,
            open_answer: None,
            save_answer: None,
            save_requests: Vec::new(),
            overlays_styled: 0,
            launch_at_login: false,
            launch_at_login_refusal: None,
            launch_at_login_unreadable: None,
            launch_at_login_sets: Vec::new(),
        }
    }
}

impl State {
    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }
}

fn scale(factor: f64) -> ScaleFactor {
    ScaleFactor::new(factor).expect("fake scale factors are positive")
}

/// The default synthetic displays:
///
/// 1. a primary 2× "built-in" display at the origin,
/// 2. a 1× external display up and to the left, with a negative origin,
/// 3. a 1.5× portrait display to the right, offset downwards.
#[must_use]
pub fn default_displays() -> Vec<DisplayInfo> {
    vec![
        DisplayInfo {
            id: DisplayId(1),
            name: "Fake Built-in (2×)".into(),
            logical_bounds: LogicalRect::new(0.0, 0.0, 1512.0, 982.0),
            pixel_size: PhysicalSize::new(3024, 1964),
            scale_factor: scale(2.0),
            is_primary: true,
        },
        DisplayInfo {
            id: DisplayId(2),
            name: "Fake External (1×)".into(),
            logical_bounds: LogicalRect::new(-1920.0, -400.0, 1920.0, 1080.0),
            pixel_size: PhysicalSize::new(1920, 1080),
            scale_factor: ScaleFactor::ONE,
            is_primary: false,
        },
        DisplayInfo {
            id: DisplayId(3),
            name: "Fake Portrait (1.5×)".into(),
            logical_bounds: LogicalRect::new(1512.0, 100.0, 800.0, 1280.0),
            pixel_size: PhysicalSize::new(1200, 1920),
            scale_factor: scale(1.5),
            is_primary: false,
        },
    ]
}

/// The default synthetic windows, front to back. One spans the primary and the
/// external display; one has no title.
#[must_use]
pub fn default_windows() -> Vec<WindowInfo> {
    let window =
        |id: u64, z_order: u32, title: Option<&str>, owner: &str, bounds: LogicalRect| WindowInfo {
            id: WindowId(id),
            title: title.map(str::to_owned),
            owner: WindowOwner {
                name: owner.to_owned(),
                pid: Some(1000 + u32::try_from(id).unwrap_or(0)),
            },
            bounds,
            z_order,
        };
    vec![
        window(
            101,
            0,
            Some("Fake Terminal"),
            "Terminal",
            LogicalRect::new(100.0, 100.0, 800.0, 500.0),
        ),
        window(
            102,
            1,
            Some("Fake Browser"),
            "Browser",
            LogicalRect::new(-600.0, 50.0, 1200.0, 700.0),
        ),
        window(
            103,
            2,
            None,
            "Editor",
            LogicalRect::new(1600.0, 300.0, 600.0, 900.0),
        ),
        window(
            104,
            3,
            Some("Fake Notes"),
            "Notes",
            LogicalRect::new(-1800.0, -300.0, 900.0, 800.0),
        ),
    ]
}

/// A deterministic test pattern: a red/green gradient across the image, a blue
/// level identifying the source (`tint`), and white grid lines every 100 logical
/// points (`100 × scale` pixels) so scaling mistakes are visible.
#[must_use]
pub fn test_pattern(size: PhysicalSize, scale: ScaleFactor, tint: u8) -> Image {
    let grid = (100.0 * scale.get()).round().max(1.0) as u32;
    let ramp = |value: u32, extent: u32| {
        u8::try_from(u64::from(value) * 255 / u64::from(extent.max(1))).unwrap_or(255)
    };
    Image::from_fn(size, |x, y| {
        if x % grid == 0 || y % grid == 0 {
            Rgba8::WHITE
        } else {
            Rgba8::rgb(ramp(x, size.width), ramp(y, size.height), tint)
        }
    })
}

impl Fake {
    /// The default world ([`default_displays`], [`default_windows`], permission
    /// granted, empty clipboard).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A world with the given displays and windows.
    #[must_use]
    pub fn with_world(displays: Vec<DisplayInfo>, windows: Vec<WindowInfo>) -> Self {
        let fake = Self::new();
        {
            let mut state = fake.state.lock();
            state.displays = displays;
            state.windows = windows;
        }
        fake
    }

    /// A [`Platform`] whose every backend is this fake world.
    #[must_use]
    pub fn platform(&self) -> Platform {
        Platform {
            displays: Box::new(self.clone()),
            capture: Box::new(self.clone()),
            window_list: Box::new(self.clone()),
            hotkeys: Box::new(self.clone()),
            status_item: Box::new(self.clone()),
            clipboard: Box::new(self.clone()),
            file_dialogs: Box::new(self.clone()),
            overlay_style: Arc::new(self.clone()),
            permissions: Box::new(self.clone()),
            launch_at_login: Box::new(self.clone()),
        }
    }

    /// Simulates pressing `hotkey`. Returns `true` if it was registered and the
    /// event was delivered.
    pub fn press_hotkey(&self, hotkey: Hotkey) -> bool {
        let state = self.state.lock();
        state.hotkeys.iter().any(|(_, bindings, sender)| {
            bindings
                .iter()
                .find(|binding| binding.hotkey == hotkey)
                .is_some_and(|binding| {
                    sender.send(HotkeyEvent {
                        mode: binding.mode,
                        hotkey,
                    })
                })
        })
    }

    /// The bindings currently registered (successfully).
    #[must_use]
    pub fn registered_hotkeys(&self) -> Vec<HotkeyBinding> {
        self.state
            .lock()
            .hotkeys
            .iter()
            .flat_map(|(_, bindings, _)| bindings.iter().copied())
            .collect()
    }

    /// Marks `hotkey` as owned by another program, so registering it fails.
    pub fn reserve_hotkey(&self, hotkey: Hotkey) {
        self.state.lock().reserved_hotkeys.insert(hotkey);
    }

    /// Simulates choosing `action` from the status item menu. Returns `true` if the
    /// status item is installed and the event was delivered.
    pub fn choose_menu_action(&self, action: MenuAction) -> bool {
        self.state
            .lock()
            .menu
            .as_ref()
            .is_some_and(|(_, sender)| sender.send(action))
    }

    /// True while a status item is installed.
    #[must_use]
    pub fn status_item_installed(&self) -> bool {
        self.state.lock().menu.is_some()
    }

    /// Replaces the clipboard contents.
    pub fn set_clipboard(&self, image: Option<Image>) {
        self.state.lock().clipboard = image;
    }

    /// The clipboard contents.
    #[must_use]
    pub fn clipboard(&self) -> Option<Image> {
        self.state.lock().clipboard.clone()
    }

    /// Sets the Screen Recording status. While denied, captures fail.
    pub fn set_screen_recording(&self, status: PermissionStatus) {
        self.state.lock().screen_recording = status;
    }

    /// Whether a permission request grants the permission (default `true`).
    pub fn set_grant_on_request(&self, grant: bool) {
        self.state.lock().grant_on_request = grant;
    }

    /// The permissions whose settings page was opened, in order.
    #[must_use]
    pub fn opened_settings(&self) -> Vec<Permission> {
        self.state.lock().opened_settings.clone()
    }

    /// The path the next open dialogs return; `None` simulates cancelling.
    pub fn set_open_answer(&self, path: Option<PathBuf>) {
        self.state.lock().open_answer = path;
    }

    /// The path the next save dialogs return; `None` simulates cancelling.
    pub fn set_save_answer(&self, path: Option<PathBuf>) {
        self.state.lock().save_answer = path;
    }

    /// Every save dialog shown so far.
    #[must_use]
    pub fn save_requests(&self) -> Vec<SaveImageRequest> {
        self.state.lock().save_requests.clone()
    }

    /// How many windows received the overlay style.
    #[must_use]
    pub fn overlays_styled(&self) -> usize {
        self.state.lock().overlays_styled
    }

    /// Changes whether the app starts at login behind the app's back, as the
    /// user can in the OS's own settings.
    pub fn set_launch_at_login(&self, enabled: bool) {
        self.state.lock().launch_at_login = enabled;
    }

    /// Whether the app starts at login (off by default).
    #[must_use]
    pub fn launch_at_login(&self) -> bool {
        self.state.lock().launch_at_login
    }

    /// Makes changing launch at login fail with `error`, until called again
    /// with `None`.
    pub fn set_launch_at_login_refusal(&self, error: Option<Error>) {
        self.state.lock().launch_at_login_refusal = error;
    }

    /// Makes reading launch at login fail with `error`, until called again
    /// with `None`.
    pub fn set_launch_at_login_unreadable(&self, error: Option<Error>) {
        self.state.lock().launch_at_login_unreadable = error;
    }

    /// Every value `LaunchAtLogin::set` applied, in order (failed calls are
    /// not listed).
    #[must_use]
    pub fn launch_at_login_sets(&self) -> Vec<bool> {
        self.state.lock().launch_at_login_sets.clone()
    }
}
