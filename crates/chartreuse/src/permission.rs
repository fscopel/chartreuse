//! Screen Recording permission: the startup check, the pre-capture gate, and the
//! guidance window (`WindowKind::Permission`). Owned by track 1C.
//!
//! # Startup
//!
//! `boot` schedules [`Message::CheckAtStartup`]. If the permission is missing, it
//! asks macOS to prompt ([`Permissions::request`]). macOS prompts once per signing
//! identity, so on later launches this just reports the status; the macOS backend
//! does not ask at all from ad-hoc signed builds, which have no stable identity.
//! If the permission is still missing, the guidance window opens without taking
//! focus, so it does not cover the system prompt.
//!
//! # Before each capture
//!
//! The capture flow calls [`ensure_screen_recording`] before every capture, and
//! [`show_guidance`] when a capture fails with
//! `Error::PermissionDenied(Permission::ScreenRecording)` or comes back blank
//! (a revoked permission can look like a successful, wallpaper-only capture).
//!
//! # The guidance window
//!
//! Explains why the permission is needed, opens the Screen Recording pane of
//! System Settings, and re-checks on demand. macOS caches the permission status
//! for the life of the process, so the status can be stale in either direction:
//!
//! - Opened because the status said "denied" (startup or the gate): a re-check
//!   that finds the permission granted closes the window. The window also says
//!   that a relaunch may be needed, since the status may never turn "granted".
//! - Opened because a capture failed ([`show_guidance`]): the status is suspect,
//!   so a re-check that finds the permission granted keeps the window open and
//!   asks the user to quit and reopen the app, with a Quit button.
//!
//! At most one guidance window is open at a time.
//!
//! [`Permissions::request`]: chartreuse_platform::Permissions::request

use chartreuse_core::flavor;
use chartreuse_core::permission::{Permission, PermissionStatus};
use iced::widget::{button, column, container, row, space, text};
use iced::{window, Element, Length, Size, Subscription, Task};

use crate::alert::{self, Notice};
use crate::app::{App, Message as AppMessage};
use crate::windows::{self, WindowKind};

/// This feature's part of the app state ([`App::permission`]).
#[derive(Debug, Default)]
pub struct State {
    guidance: Option<Guidance>,
}

/// The open guidance window.
#[derive(Debug)]
struct Guidance {
    window: window::Id,
    reason: Reason,
    /// The result of the last "Check again" since the window was shown.
    checked: Option<PermissionStatus>,
}

impl Guidance {
    /// A re-check found the permission granted, but the window stays open
    /// because only a relaunch can make captures work.
    fn needs_relaunch(&self) -> bool {
        self.reason == Reason::CaptureFailed && self.checked == Some(PermissionStatus::Granted)
    }
}

/// Why the guidance window is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// The permission status said "denied", at startup or at the pre-capture
    /// gate. A "granted" re-check is trusted and closes the window.
    Denied,
    /// A capture failed for lack of permission. The status may be a stale
    /// "granted", so a "granted" re-check asks for a relaunch instead.
    CaptureFailed,
}

/// This feature's messages ([`AppMessage::Permission`]).
#[derive(Debug, Clone)]
pub enum Message {
    /// Check (and on first run request) the permission; sent once by `boot`.
    CheckAtStartup,
    /// The guidance window's "Open System Settings" button.
    OpenSettings,
    /// The guidance window's "Check again" button.
    Recheck,
    /// The guidance window's Quit button, shown when a relaunch is needed.
    Quit,
}

/// The pre-capture gate: whether a capture may start.
///
/// Returns `Ok(())` when Screen Recording is granted. Otherwise returns
/// `Err(task)`, where `task` opens (or focuses) the guidance window; return it
/// from `update` instead of capturing. Never prompts: macOS prompts only once,
/// at startup.
///
/// If the platform cannot report the status, logs a warning and lets the capture
/// proceed, so the capture reports its own error.
///
/// ```ignore
/// if let Err(guidance) = permission::ensure_screen_recording(app) {
///     return guidance;
/// }
/// // … start the capture …
/// ```
pub fn ensure_screen_recording(app: &mut App) -> Result<(), Task<AppMessage>> {
    match app.platform.permissions.status(Permission::ScreenRecording) {
        Ok(PermissionStatus::Granted) => Ok(()),
        Ok(PermissionStatus::Denied) => {
            tracing::info!("Screen Recording permission missing; not capturing");
            Err(open_guidance(app, Reason::Denied, true))
        }
        Err(error) => {
            tracing::warn!(%error, "could not check Screen Recording permission; capturing anyway");
            Ok(())
        }
    }
}

/// Shows the guidance window after a capture failed for lack of Screen
/// Recording permission, or focuses it if it is already open. Returns the task
/// to return from `update`.
///
/// Call it when a capture fails with
/// `Error::PermissionDenied(Permission::ScreenRecording)` or comes back blank
/// (a revoked permission can look like a successful, wallpaper-only capture).
///
/// macOS caches the permission status for the life of the process, so after
/// such a failure it may keep reporting "granted". The window therefore stays
/// open when "Check again" finds the permission granted, and asks the user to
/// quit and reopen the app.
pub fn show_guidance(app: &mut App) -> Task<AppMessage> {
    open_guidance(app, Reason::CaptureFailed, true)
}

/// Opens the guidance window, or re-shows it if it is already open. With
/// `focus`, also brings it to the front.
fn open_guidance(app: &mut App, reason: Reason, focus: bool) -> Task<AppMessage> {
    if let Some(guidance) = &mut app.permission.guidance {
        // A failed capture outweighs the status, so the reason only escalates.
        if reason == Reason::CaptureFailed {
            guidance.reason = reason;
        }
        guidance.checked = None;
        return if focus {
            window::gain_focus(guidance.window)
        } else {
            Task::none()
        };
    }
    let (id, open) = app.windows.open(
        WindowKind::Permission,
        window::Settings {
            size: Size::new(480.0, 280.0),
            resizable: false,
            minimizable: false,
            icon: windows::icon(),
            ..window::Settings::default()
        },
    );
    app.permission.guidance = Some(Guidance {
        window: id,
        reason,
        checked: None,
    });
    let open = open.discard();
    if focus {
        // The app has no Dock icon, so bring the window forward explicitly.
        open.chain(window::gain_focus(id))
    } else {
        open
    }
}

pub fn boot(_app: &mut App) -> Task<AppMessage> {
    Task::done(AppMessage::Permission(Message::CheckAtStartup))
}

pub fn update(app: &mut App, message: Message) -> Task<AppMessage> {
    match message {
        Message::CheckAtStartup => check_at_startup(app),
        Message::OpenSettings => match app
            .platform
            .permissions
            .open_settings(Permission::ScreenRecording)
        {
            Ok(()) => Task::none(),
            Err(error) => alert::report_error(
                app,
                Notice::from_error("Could not open System Settings", &error),
            ),
        },
        Message::Recheck => recheck(app),
        Message::Quit => iced::exit(),
    }
}

fn check_at_startup(app: &mut App) -> Task<AppMessage> {
    let permissions = &app.platform.permissions;
    let status = match permissions.status(Permission::ScreenRecording) {
        Ok(PermissionStatus::Granted) => {
            tracing::info!("Screen Recording permission granted");
            return Task::none();
        }
        Ok(PermissionStatus::Denied) => {
            tracing::info!("Screen Recording permission missing; requesting it");
            permissions.request(Permission::ScreenRecording)
        }
        Err(error) => Err(error),
    };
    match status {
        Ok(PermissionStatus::Granted) => {
            tracing::info!("Screen Recording permission granted on request");
            Task::none()
        }
        Ok(PermissionStatus::Denied) => {
            // Unfocused, so the window does not cover the system prompt.
            tracing::info!("Screen Recording permission still missing; showing guidance");
            open_guidance(app, Reason::Denied, false)
        }
        Err(error) => {
            tracing::warn!(%error, "could not check Screen Recording permission");
            Task::none()
        }
    }
}

fn recheck(app: &mut App) -> Task<AppMessage> {
    let status = match app.platform.permissions.status(Permission::ScreenRecording) {
        Ok(status) => status,
        Err(error) => {
            return alert::report_error(
                app,
                Notice::from_error("Could not check Screen Recording permission", &error),
            );
        }
    };
    let Some(guidance) = &mut app.permission.guidance else {
        return Task::none();
    };
    guidance.checked = Some(status);
    match (status, guidance.reason) {
        (PermissionStatus::Granted, Reason::Denied) => {
            tracing::info!("Screen Recording permission granted");
            let window = guidance.window;
            app.permission.guidance = None;
            window::close(window)
        }
        (PermissionStatus::Granted, Reason::CaptureFailed) => {
            tracing::info!(
                "Screen Recording permission reported granted after a failed capture; \
                 asking for a relaunch"
            );
            Task::none()
        }
        (PermissionStatus::Denied, _) => {
            tracing::info!("Screen Recording permission still missing");
            Task::none()
        }
    }
}

pub fn subscription(_app: &App) -> Subscription<AppMessage> {
    Subscription::none()
}

pub fn view(app: &App, id: window::Id) -> Element<'_, AppMessage> {
    let Some(guidance) = app
        .permission
        .guidance
        .as_ref()
        .filter(|guidance| guidance.window == id)
    else {
        return space().into();
    };
    let name = flavor::DISPLAY_NAME;
    let needs_relaunch = guidance.needs_relaunch();
    let open_settings = button(text("Open System Settings"))
        .style(if needs_relaunch {
            button::secondary
        } else {
            button::primary
        })
        .padding([6, 16])
        .on_press(AppMessage::Permission(Message::OpenSettings));
    // Once only a relaunch helps, Quit replaces "Check again" and becomes the
    // primary action, which sits rightmost.
    let (status, buttons) = if needs_relaunch {
        let quit = button(text(format!("Quit {name}")))
            .style(button::primary)
            .padding([6, 16])
            .on_press(AppMessage::Permission(Message::Quit));
        (
            text(format!(
                "Screen Recording is on, but macOS applies it to {name} only after \
                 a restart. Quit {name} and open it again."
            )),
            row![space().width(Length::Fill), open_settings, quit],
        )
    } else {
        let recheck = button(text("Check again"))
            .style(button::secondary)
            .padding([6, 16])
            .on_press(AppMessage::Permission(Message::Recheck));
        let status = match guidance.checked {
            Some(PermissionStatus::Denied) => {
                text("Screen Recording is still off for this copy of the app.")
            }
            _ => text(""),
        };
        (
            status,
            row![space().width(Length::Fill), recheck, open_settings],
        )
    };
    container(
        column![
            text("Allow Screen Recording").size(18),
            text(format!(
                "{name} needs Screen Recording permission to capture your screen. \
                 Turn on {name} in System Settings › Privacy & Security › \
                 Screen & System Audio Recording."
            )),
            text(format!(
                "After granting access, you may need to quit and reopen {name}."
            )),
            status,
            space().height(Length::Fill),
            buttons.spacing(10),
        ]
        .spacing(10),
    )
    .padding(20)
    .into()
}

pub fn window_closed(app: &mut App, id: window::Id) -> Task<AppMessage> {
    if app
        .permission
        .guidance
        .as_ref()
        .is_some_and(|guidance| guidance.window == id)
    {
        app.permission.guidance = None;
    }
    Task::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guidance_windows(app: &App) -> usize {
        app.windows.of_kind(WindowKind::Permission).count()
    }

    fn start(app: &mut App) {
        let _ = app.update(AppMessage::Permission(Message::CheckAtStartup));
    }

    #[test]
    fn startup_with_permission_opens_nothing() {
        let (mut app, _fake) = App::for_test();
        start(&mut app);
        assert_eq!(guidance_windows(&app), 0);
        assert!(app.permission.guidance.is_none());
    }

    #[test]
    fn startup_without_permission_requests_it() {
        let (mut app, fake) = App::for_test();
        fake.set_screen_recording(PermissionStatus::Denied);
        fake.set_grant_on_request(true);
        start(&mut app);
        // The request was made (and accepted), so no guidance is needed.
        assert_eq!(
            app.platform
                .permissions
                .status(Permission::ScreenRecording)
                .unwrap(),
            PermissionStatus::Granted
        );
        assert_eq!(guidance_windows(&app), 0);
    }

    #[test]
    fn startup_shows_guidance_when_the_request_is_declined() {
        let (mut app, fake) = App::for_test();
        fake.set_screen_recording(PermissionStatus::Denied);
        fake.set_grant_on_request(false);
        start(&mut app);
        assert_eq!(guidance_windows(&app), 1);
        assert!(app.permission.guidance.is_some());
    }

    #[test]
    fn guidance_shows_the_app_icon() {
        let (mut app, _fake) = App::for_test();
        let open = show_guidance(&mut app);
        assert_eq!(windows::opened_with_icon(open), [true]);
    }

    #[test]
    fn guidance_opens_the_screen_recording_settings() {
        let (mut app, fake) = App::for_test();
        let _ = update(&mut app, Message::OpenSettings);
        assert_eq!(fake.opened_settings(), vec![Permission::ScreenRecording]);
        assert_eq!(app.windows.of_kind(WindowKind::Alert).count(), 0);
    }

    #[test]
    fn recheck_keeps_guidance_until_granted() {
        let (mut app, fake) = App::for_test();
        fake.set_screen_recording(PermissionStatus::Denied);
        fake.set_grant_on_request(false);
        start(&mut app);

        let _ = update(&mut app, Message::Recheck);
        let guidance = app.permission.guidance.as_ref().expect("still open");
        assert_eq!(guidance.checked, Some(PermissionStatus::Denied));
        assert!(!guidance.needs_relaunch());

        fake.set_screen_recording(PermissionStatus::Granted);
        let _ = update(&mut app, Message::Recheck);
        assert!(app.permission.guidance.is_none());
        assert!(ensure_screen_recording(&mut app).is_ok());
    }

    #[test]
    fn granted_recheck_after_a_failed_capture_asks_for_a_relaunch() {
        // The status says granted (possibly stale), but the capture failed.
        let (mut app, fake) = App::for_test();
        let _ = show_guidance(&mut app);
        assert!(!app.permission.guidance.as_ref().unwrap().needs_relaunch());

        let _ = update(&mut app, Message::Recheck);
        let guidance = app.permission.guidance.as_ref().expect("stays open");
        assert!(guidance.needs_relaunch());
        assert_eq!(guidance_windows(&app), 1);

        // A denied re-check drops the relaunch request: granting comes first.
        fake.set_screen_recording(PermissionStatus::Denied);
        let _ = update(&mut app, Message::Recheck);
        let guidance = app.permission.guidance.as_ref().expect("stays open");
        assert!(!guidance.needs_relaunch());
        assert_eq!(guidance.checked, Some(PermissionStatus::Denied));
    }

    #[test]
    fn a_failed_capture_keeps_open_guidance_from_closing_on_grant() {
        let (mut app, fake) = App::for_test();
        fake.set_screen_recording(PermissionStatus::Denied);
        fake.set_grant_on_request(false);
        start(&mut app);
        let window = app.permission.guidance.as_ref().unwrap().window;

        // A capture fails while the startup guidance is still open: the same
        // window now distrusts a "granted" status.
        let _ = show_guidance(&mut app);
        // The gate refusing again does not make it trust the status again.
        assert!(ensure_screen_recording(&mut app).is_err());
        fake.set_screen_recording(PermissionStatus::Granted);
        let _ = update(&mut app, Message::Recheck);
        let guidance = app.permission.guidance.as_ref().expect("stays open");
        assert_eq!(guidance.window, window);
        assert!(guidance.needs_relaunch());
        assert_eq!(guidance_windows(&app), 1);
    }

    #[test]
    fn gate_allows_capture_only_with_permission() {
        let (mut app, fake) = App::for_test();
        assert!(ensure_screen_recording(&mut app).is_ok());
        assert_eq!(guidance_windows(&app), 0);

        // Revoked while running: the gate refuses and shows guidance without
        // prompting (a prompt would grant here).
        fake.set_screen_recording(PermissionStatus::Denied);
        fake.set_grant_on_request(true);
        assert!(ensure_screen_recording(&mut app).is_err());
        assert_eq!(guidance_windows(&app), 1);
        assert_eq!(
            app.platform
                .permissions
                .status(Permission::ScreenRecording)
                .unwrap(),
            PermissionStatus::Denied
        );
    }

    #[test]
    fn repeated_refusals_reuse_one_guidance_window() {
        let (mut app, fake) = App::for_test();
        fake.set_screen_recording(PermissionStatus::Denied);
        assert!(ensure_screen_recording(&mut app).is_err());
        assert!(ensure_screen_recording(&mut app).is_err());
        assert_eq!(guidance_windows(&app), 1);

        // Once the user closes it, the next refusal opens a new one.
        let first = app.permission.guidance.as_ref().unwrap().window;
        let _ = app.update(AppMessage::WindowClosed(first));
        assert_eq!(guidance_windows(&app), 0);
        assert!(ensure_screen_recording(&mut app).is_err());
        let second = app.permission.guidance.as_ref().unwrap().window;
        assert_ne!(first, second);
        assert_eq!(guidance_windows(&app), 1);
    }
}
