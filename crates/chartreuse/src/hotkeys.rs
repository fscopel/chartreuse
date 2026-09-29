//! Global hotkeys: registers the hotkey set through the platform
//! [`Hotkeys`](chartreuse_platform::Hotkeys) trait and turns presses into
//! [`Message::Pressed`]. Owned by track 1B; settings (3A) re-register through
//! [`reregister`] and [`replace`], and [`suspend`] the hotkeys while one is
//! being recorded.
//!
//! # Startup
//!
//! `boot` schedules [`Message::Register`] (Carbon needs the running event loop),
//! which registers the hotkeys from the settings ([`App::config`]), one per
//! capture mode ([`bindings`]). By default they are Ctrl+Alt+Shift+3 (display),
//! +4 (rectangle) and +5 (window), echoing the system screenshot shortcuts
//! (Shift+Command+3/4/5) without clashing with them, since macOS keeps those
//! for itself (see [`chartreuse_config::Hotkeys`]). Pressing one starts that
//! capture (`capture::Message::Start`).
//!
//! # Failures
//!
//! Bindings that cannot be registered (typically because another app already uses
//! the combination) are reported in one alert listing every failing combination;
//! the other bindings stay active. If hotkeys are unavailable altogether, that is
//! reported instead. [`replace`] leaves the reporting to its caller, and
//! [`State::problem`] tells what is wrong with each mode's hotkey until the
//! next registration.

use chartreuse_config::Hotkeys;
use chartreuse_core::capture::CaptureMode;
use chartreuse_core::Error;
use chartreuse_platform::{HotkeyBinding, HotkeyEvent, HotkeyRegistration};
use iced::{Subscription, Task};

use crate::alert::{self, Notice};
use crate::app::{App, Message as AppMessage};
use crate::{capture, events};

/// This feature's part of the app state ([`App::hotkeys`]).
#[derive(Debug, Default)]
pub struct State {
    /// The active set; its receiver drives [`subscription`].
    registration: Option<HotkeyRegistration>,
    /// Why nothing is registered, if registering failed altogether.
    unavailable: Option<Error>,
}

impl State {
    /// Why the hotkey of `mode` is not active after the last registration: it
    /// failed to register, or hotkeys are unavailable altogether. `None` if it
    /// is active, or nothing has been registered (or it is [`suspend`]ed).
    #[must_use]
    pub fn problem(&self, mode: CaptureMode) -> Option<&Error> {
        if let Some(error) = &self.unavailable {
            return Some(error);
        }
        self.registration
            .as_ref()?
            .failures
            .iter()
            .find(|(binding, _)| binding.mode == mode)
            .map(|(_, error)| error)
    }
}

/// This feature's messages ([`AppMessage::Hotkeys`]).
#[derive(Debug, Clone)]
pub enum Message {
    /// Register the hotkeys from the settings ([`bindings`] of
    /// [`App::config`]). Sent by `boot`.
    Register,
    /// A registered hotkey was pressed.
    Pressed(HotkeyEvent),
}

/// What to register for `hotkeys`: one binding per capture mode.
#[must_use]
pub fn bindings(hotkeys: &Hotkeys) -> Vec<HotkeyBinding> {
    hotkeys
        .bindings()
        .into_iter()
        .map(|(mode, hotkey)| HotkeyBinding { mode, hotkey })
        .collect()
}

/// Replaces the registered hotkeys with `bindings` and returns the task to
/// return from `update` (an alert if some or all of them failed).
///
/// The previous set is unregistered first, so a combination that stays in the
/// set is free to register again. Presses of the new set arrive as
/// [`Message::Pressed`]. Main thread only: call it from `update`.
pub fn reregister(app: &mut App, bindings: Vec<HotkeyBinding>) -> Task<AppMessage> {
    match replace(app, &bindings) {
        Some(notice) => alert::report_error(app, notice),
        None => Task::none(),
    }
}

/// Replaces the registered hotkeys with `bindings`, as [`reregister`] does,
/// but leaves telling the user about failures to the caller: returns the
/// notice [`reregister`] would show, if any. [`State::problem`] also tells
/// what failed. Main thread only.
pub fn replace(app: &mut App, bindings: &[HotkeyBinding]) -> Option<Notice> {
    suspend(app);
    match app.platform.hotkeys.register(bindings) {
        Ok(registration) => {
            tracing::info!(
                registered = bindings.len() - registration.failures.len(),
                failed = registration.failures.len(),
                "hotkeys registered"
            );
            let notice = failure_notice(&registration.failures);
            app.hotkeys.registration = Some(registration);
            notice
        }
        Err(error) => {
            let notice = Notice::from_error("Hotkeys unavailable", &error);
            app.hotkeys.unavailable = Some(error);
            Some(notice)
        }
    }
}

/// Unregisters every hotkey until the next [`reregister`] or [`replace`], so
/// that their key combinations reach the app's own windows (the settings
/// window records hotkeys this way). Main thread only.
pub fn suspend(app: &mut App) {
    app.hotkeys.registration = None;
    app.hotkeys.unavailable = None;
}

/// The alert for bindings that failed to register: one line per combination, or
/// `None` if all of them registered.
fn failure_notice(failures: &[(HotkeyBinding, Error)]) -> Option<Notice> {
    let title = match failures.len() {
        0 => return None,
        1 => "Hotkey unavailable",
        _ => "Hotkeys unavailable",
    };
    let lines: Vec<String> = failures
        .iter()
        .map(|(binding, error)| {
            let reason = match error {
                Error::HotkeyUnavailable { reason, .. } => reason.clone(),
                other => other.to_string(),
            };
            format!("{} ({}): {reason}.", binding.hotkey, binding.mode)
        })
        .collect();
    Some(Notice::new(title, lines.join("\n")))
}

pub fn boot(_app: &mut App) -> Task<AppMessage> {
    Task::done(AppMessage::Hotkeys(Message::Register))
}

pub fn update(app: &mut App, message: Message) -> Task<AppMessage> {
    match message {
        Message::Register => reregister(app, bindings(&app.config.hotkeys)),
        Message::Pressed(event) => {
            tracing::info!(hotkey = %event.hotkey, "hotkey pressed: {}", event.mode);
            Task::done(AppMessage::Capture(capture::Message::Start(event.mode)))
        }
    }
}

pub fn subscription(app: &App) -> Subscription<AppMessage> {
    match &app.hotkeys.registration {
        Some(registration) => events::subscription(&registration.events)
            .map(|event| AppMessage::Hotkeys(Message::Pressed(event))),
        None => Subscription::none(),
    }
}

#[cfg(test)]
mod tests {
    use chartreuse_core::hotkey::{Hotkey, Key, Modifiers};
    use chartreuse_platform::Hotkeys as _;
    use futures::executor::block_on;
    use futures::StreamExt;
    use iced::advanced::subscription::into_recipes;

    use super::*;
    use crate::windows::WindowKind;

    fn alerts(app: &App) -> usize {
        app.windows.of_kind(WindowKind::Alert).count()
    }

    fn binding(mode: CaptureMode, key: Key) -> HotkeyBinding {
        HotkeyBinding {
            mode,
            hotkey: Hotkey::new(Modifiers::SUPER | Modifiers::SHIFT, key),
        }
    }

    /// The bindings of the default settings.
    fn defaults() -> Vec<HotkeyBinding> {
        bindings(&Hotkeys::default())
    }

    #[test]
    fn boot_leaves_registering_the_configured_hotkeys_to_update() {
        let (mut app, fake) = App::for_test();
        let [display, selection] = [Key::F1, Key::F2].map(|key| Hotkey::new(Modifiers::SUPER, key));
        app.config.hotkeys = Hotkeys::new(display, selection).unwrap();
        let _ = boot(&mut app);
        assert!(fake.registered_hotkeys().is_empty());

        let _ = app.update(AppMessage::Hotkeys(Message::Register));
        assert_eq!(
            fake.registered_hotkeys(),
            [
                HotkeyBinding {
                    mode: CaptureMode::Display,
                    hotkey: display
                },
                HotkeyBinding {
                    mode: CaptureMode::Selection,
                    hotkey: selection
                },
            ]
        );
        assert_eq!(alerts(&app), 0);
    }

    #[test]
    fn presses_start_the_bound_capture() {
        let (mut app, fake) = App::for_test();
        let _ = update(&mut app, Message::Register);
        let mut recipes = into_recipes(subscription(&app));
        assert_eq!(recipes.len(), 1);
        let mut presses = recipes
            .pop()
            .unwrap()
            .stream(futures::stream::empty().boxed());

        for binding in defaults() {
            assert!(fake.press_hotkey(binding.hotkey));
            let message = block_on(presses.next());
            let Some(AppMessage::Hotkeys(Message::Pressed(event))) = message else {
                panic!("expected a press, got {message:?}");
            };
            assert_eq!(
                event,
                HotkeyEvent {
                    mode: binding.mode,
                    hotkey: binding.hotkey
                }
            );

            let started: Vec<_> =
                iced_runtime::task::into_stream(update(&mut app, Message::Pressed(event)))
                    .map(|stream| block_on(stream.collect()))
                    .unwrap_or_default();
            assert!(
                matches!(
                    started.as_slice(),
                    [iced_runtime::Action::Output(AppMessage::Capture(
                        capture::Message::Start(mode)
                    ))] if *mode == binding.mode
                ),
                "{}: {started:?}",
                binding.mode
            );
        }
    }

    #[test]
    fn taken_hotkeys_are_reported_in_one_alert() {
        let (mut app, fake) = App::for_test();
        let display = binding(CaptureMode::Display, Key::Digit1);
        let selection = binding(CaptureMode::Selection, Key::Digit2);
        fake.reserve_hotkey(display.hotkey);
        fake.reserve_hotkey(selection.hotkey);

        let _ = reregister(&mut app, vec![display, selection]);
        assert_eq!(alerts(&app), 1);
        assert!(fake.registered_hotkeys().is_empty());
    }

    #[test]
    fn replacing_leaves_reporting_to_the_caller_and_tells_what_failed_per_mode() {
        let (mut app, fake) = App::for_test();
        let display = binding(CaptureMode::Display, Key::Digit1);
        let selection = binding(CaptureMode::Selection, Key::Digit2);
        fake.reserve_hotkey(selection.hotkey);

        let notice = replace(&mut app, &[display, selection]);
        assert_eq!(
            notice.map(|notice| notice.title).as_deref(),
            Some("Hotkey unavailable")
        );
        assert_eq!(alerts(&app), 0);
        assert!(matches!(
            app.hotkeys.problem(CaptureMode::Selection),
            Some(Error::HotkeyUnavailable { hotkey, .. }) if *hotkey == selection.hotkey
        ));
        assert!(app.hotkeys.problem(CaptureMode::Display).is_none());

        // Once it registers, the problem is gone.
        let selection = binding(CaptureMode::Selection, Key::Digit3);
        assert_eq!(replace(&mut app, &[display, selection]), None);
        assert!(app.hotkeys.problem(CaptureMode::Selection).is_none());
    }

    #[test]
    fn suspended_hotkeys_are_released_until_registered_again() {
        let (mut app, fake) = App::for_test();
        let _ = update(&mut app, Message::Register);

        suspend(&mut app);
        assert!(fake.registered_hotkeys().is_empty());
        assert!(!fake.press_hotkey(defaults()[0].hotkey));
        assert!(into_recipes(subscription(&app)).is_empty());

        let _ = replace(&mut app, &defaults());
        assert_eq!(fake.registered_hotkeys(), defaults());
    }

    #[test]
    fn the_failure_notice_lists_every_failing_combination() {
        let fake = chartreuse_platform::fake::Fake::new();
        let display = binding(CaptureMode::Display, Key::Digit1);
        let selection = binding(CaptureMode::Selection, Key::Digit2);
        fake.reserve_hotkey(display.hotkey);
        fake.reserve_hotkey(selection.hotkey);
        let registration = fake.register(&[display, selection]).unwrap();

        let notice = failure_notice(&registration.failures).unwrap();
        let lines: Vec<&str> = notice.body.lines().collect();
        assert_eq!(lines.len(), 2, "{}", notice.body);
        for (line, failed) in lines.iter().zip([display, selection]) {
            assert!(line.contains(&failed.hotkey.to_string()), "{line}");
            assert!(line.contains(&failed.mode.to_string()), "{line}");
            assert!(line.contains("another program has registered it"), "{line}");
        }
        assert_eq!(failure_notice(&[]), None);
    }

    #[test]
    fn reregistering_replaces_the_set() {
        let (mut app, fake) = App::for_test();
        let _ = update(&mut app, Message::Register);
        let old_events = app.hotkeys.registration.as_ref().unwrap().events.clone();

        // The same set again: the old registration is gone first, so no clash.
        let _ = reregister(&mut app, defaults());
        assert_eq!(fake.registered_hotkeys(), defaults());
        assert_eq!(alerts(&app), 0);

        let selection = binding(CaptureMode::Selection, Key::W);
        let _ = reregister(&mut app, vec![selection]);
        assert_eq!(fake.registered_hotkeys(), [selection]);
        assert!(!fake.press_hotkey(defaults()[0].hotkey));
        assert!(fake.press_hotkey(selection.hotkey));
        let registration = app.hotkeys.registration.as_ref().unwrap();
        assert_ne!(registration.events, old_events, "a new subscription");
        assert_eq!(
            registration.events.try_recv().map(|event| event.mode),
            Some(CaptureMode::Selection)
        );
    }

    #[derive(Debug)]
    struct Unavailable;

    impl chartreuse_platform::Hotkeys for Unavailable {
        fn register(
            &self,
            _bindings: &[HotkeyBinding],
        ) -> chartreuse_core::Result<HotkeyRegistration> {
            Err(Error::Unsupported("global hotkey registration"))
        }
    }

    #[test]
    fn unavailable_hotkeys_are_reported_and_the_old_set_is_released() {
        let (mut app, fake) = App::for_test();
        let _ = update(&mut app, Message::Register);
        app.platform.hotkeys = Box::new(Unavailable);

        let _ = reregister(&mut app, defaults());
        assert_eq!(alerts(&app), 1);
        assert!(app.hotkeys.registration.is_none());
        assert!(fake.registered_hotkeys().is_empty());
        for mode in CaptureMode::ALL {
            assert!(
                matches!(app.hotkeys.problem(mode), Some(Error::Unsupported(_))),
                "{mode}"
            );
        }
    }
}
