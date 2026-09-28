//! The Open at login setting: [`Settings::launch_at_login`] kept in step with
//! the OS's login item ([`chartreuse_platform::LaunchAtLogin`]).
//!
//! # The OS wins
//!
//! The OS holds the setting that counts, and the user can change it there as
//! well: System Settings > General > Login Items on macOS, Task Manager's
//! Startup apps on Windows, the desktop's startup settings on Linux. So the
//! OS's state wins. At startup, and whenever the settings window opens,
//! [`follow_the_os`] reads it and, where the settings say otherwise, changes
//! the settings (and the file) to match. A login item the user turned off in
//! the OS stays off: the settings file never turns it back on behind their
//! back, and the toggle always shows what will happen at the next login. If
//! the OS's state cannot be read (on macOS, when the app does not run from
//! its bundle), the setting stays as it is.
//!
//! Changes go the other way through the OS: the toggle asks the OS first
//! ([`toggle`]), and the setting changes only if the OS did; if it did not,
//! the line under the toggle says why ([`note`]), and the setting follows
//! the OS. A hand edit of the settings file is passed on to the OS in the
//! same way ([`hand_edited`]), and a refusal is reported in an alert, the
//! window perhaps not being open.

use chartreuse_config::Settings;
use chartreuse_core::flavor;
use iced::Task;

use super::{change, Note};
use crate::alert::{self, Notice};
use crate::app::{App, Message as AppMessage};

/// Makes the setting follow the OS's login item (see
/// [The OS wins](self#the-os-wins)).
pub(super) fn follow_the_os(app: &mut App) -> Task<AppMessage> {
    match app.platform.launch_at_login.status() {
        Ok(enabled) if enabled != app.config.launch_at_login => {
            tracing::info!(
                enabled,
                "the OS's login item differs from the settings; following the OS"
            );
            change(app, move |config: &mut Settings| {
                config.launch_at_login = enabled;
            })
        }
        Ok(_) => Task::none(),
        Err(error) => {
            tracing::info!(%error, "cannot read the OS's login item; keeping the setting");
            Task::none()
        }
    }
}

/// The toggle was switched: asks the OS to open the app at login, or not to,
/// and changes the setting if it did.
pub(super) fn toggle(app: &mut App, enabled: bool) -> Task<AppMessage> {
    match app.platform.launch_at_login.set(enabled) {
        Ok(()) => {
            app.settings.login_item_problem = None;
            change(app, move |config: &mut Settings| {
                config.launch_at_login = enabled;
            })
        }
        Err(error) => {
            tracing::warn!(%error, enabled, "the OS did not change the login item");
            app.settings.login_item_problem = Some(alert::capitalize(&error.to_string()));
            follow_the_os(app)
        }
    }
}

/// A hand edit of the settings file changed the setting: passes it on to the
/// OS. If the OS refuses, says so, and the setting follows the OS.
pub(super) fn hand_edited(app: &mut App) -> Task<AppMessage> {
    let enabled = app.config.launch_at_login;
    match app.platform.launch_at_login.set(enabled) {
        Ok(()) => {
            app.settings.login_item_problem = None;
            Task::none()
        }
        Err(error) => {
            app.settings.login_item_problem = Some(alert::capitalize(&error.to_string()));
            let title = if enabled {
                format!("{} cannot open at login", flavor::DISPLAY_NAME)
            } else {
                format!("{} still opens at login", flavor::DISPLAY_NAME)
            };
            let reported = alert::report_error(app, Notice::from_error(title, &error));
            Task::batch([reported, follow_the_os(app)])
        }
    }
}

/// The line under the toggle: why the OS did not take the last change.
pub(super) fn note(app: &App) -> Option<Note> {
    app.settings.login_item_problem.clone().map(Note::Problem)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chartreuse_core::Error;
    use chartreuse_platform::fake::Fake;
    use tempfile::TempDir;

    use super::super::{Message, SettingsFile};
    use super::*;
    use crate::windows::WindowKind;

    /// A test app keeping its settings in a file in a new temporary
    /// directory.
    fn app_with_file() -> (App, Fake, TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let (mut app, fake) = App::for_test();
        app.settings.file = Some(SettingsFile::new(temp.path().join("settings.toml")));
        (app, fake, temp)
    }

    fn send(app: &mut App, message: Message) {
        let _ = app.settle(AppMessage::Settings(message));
    }

    /// The settings in the settings file.
    fn on_disk(app: &App) -> Settings {
        chartreuse_config::load(&app.settings.file.as_ref().unwrap().path).unwrap()
    }

    fn alerts(app: &App) -> usize {
        app.windows.of_kind(WindowKind::Alert).count()
    }

    /// Runs [`follow_the_os`] and every message it leads to, as at startup.
    fn follow(app: &mut App) {
        let task = follow_the_os(app);
        for action in iced_runtime::task::into_stream(task)
            .map(|stream| {
                futures::executor::block_on(futures::StreamExt::collect::<Vec<_>>(stream))
            })
            .unwrap_or_default()
        {
            if let iced_runtime::Action::Output(message) = action {
                let _ = app.settle(message);
            }
        }
    }

    #[test]
    fn the_toggle_changes_the_login_item_and_saves_the_setting() {
        let (mut app, fake, _temp) = app_with_file();
        send(&mut app, Message::Open);

        send(&mut app, Message::LaunchAtLogin(true));
        assert!(fake.launch_at_login());
        assert!(app.config.launch_at_login);
        assert!(on_disk(&app).launch_at_login);

        send(&mut app, Message::LaunchAtLogin(false));
        assert!(!fake.launch_at_login());
        assert!(!on_disk(&app).launch_at_login);
        assert_eq!(fake.launch_at_login_sets(), [true, false]);
    }

    #[test]
    fn a_change_the_os_refuses_is_shown_under_the_toggle_and_not_saved() {
        let (mut app, fake, _temp) = app_with_file();
        send(&mut app, Message::Open);
        fake.set_launch_at_login_refusal(Some(Error::Platform(
            "the login item needs approval".into(),
        )));

        send(&mut app, Message::LaunchAtLogin(true));
        assert!(!app.config.launch_at_login, "the toggle stays off");
        assert!(!fake.launch_at_login());
        assert_eq!(
            note(&app),
            Some(Note::Problem("The login item needs approval".into()))
        );
        assert_eq!(alerts(&app), 0, "the window shows it");

        // Once the OS takes it, the problem goes.
        fake.set_launch_at_login_refusal(None);
        send(&mut app, Message::LaunchAtLogin(true));
        assert!(app.config.launch_at_login);
        assert_eq!(note(&app), None);
    }

    #[test]
    fn at_startup_the_setting_follows_the_os() {
        let (mut app, fake, _temp) = app_with_file();
        // The user turned the login item off in the OS since the file said on.
        app.config.launch_at_login = true;
        follow(&mut app);
        assert!(!app.config.launch_at_login);
        assert!(!on_disk(&app).launch_at_login);

        // And on.
        fake.set_launch_at_login(true);
        follow(&mut app);
        assert!(app.config.launch_at_login);
        assert!(on_disk(&app).launch_at_login);
        assert!(
            fake.launch_at_login_sets().is_empty(),
            "following the OS never changes it"
        );
    }

    #[test]
    fn a_login_item_that_cannot_be_read_leaves_the_setting_alone() {
        let (mut app, fake, _temp) = app_with_file();
        app.config.launch_at_login = true;
        fake.set_launch_at_login_unreadable(Some(Error::Platform("no app bundle".into())));
        follow(&mut app);
        assert!(app.config.launch_at_login);
        assert_eq!(alerts(&app), 0);
    }

    #[test]
    fn opening_the_window_picks_up_a_change_made_in_the_os() {
        let (mut app, fake, _temp) = app_with_file();
        fake.set_launch_at_login(true);
        send(&mut app, Message::Open);
        assert!(app.config.launch_at_login);
    }

    #[test]
    fn a_hand_edit_is_passed_on_to_the_os() {
        let (mut app, fake, _temp) = app_with_file();
        let path = app.settings.file.as_ref().unwrap().path.clone();
        fs::write(&path, "launch_at_login = true\n").unwrap();
        send(&mut app, Message::FileChanged);
        assert!(app.config.launch_at_login);
        assert!(fake.launch_at_login());

        // Refused, it is reported, and the setting follows the OS again.
        fake.set_launch_at_login_refusal(Some(Error::Platform("denied".into())));
        fs::write(&path, "launch_at_login = false\n").unwrap();
        send(&mut app, Message::FileChanged);
        assert_eq!(alerts(&app), 1);
        assert!(fake.launch_at_login());
        assert!(app.config.launch_at_login, "following the OS");
        assert!(on_disk(&app).launch_at_login);
    }
}
