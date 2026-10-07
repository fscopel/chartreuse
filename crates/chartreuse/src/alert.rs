//! User notices: [`report_error`] opens a small alert window.
//!
//! Every feature reports user-visible problems here (hotkey conflicts, an empty
//! clipboard, undecodable files, …) instead of failing silently.

use std::collections::HashMap;

use chartreuse_core::Error;
use iced::widget::{button, column, container, row, space, text};
use iced::{window, Element, Length, Size, Subscription, Task};

use crate::app::{App, Message as AppMessage};
use crate::windows::{self, WindowKind};

/// What an alert window says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// A short headline, e.g. "Hotkey unavailable".
    pub title: String,
    /// The explanation, e.g. the error's `Display` text.
    pub body: String,
}

impl Notice {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
        }
    }

    /// A notice whose body is `error`'s message, capitalized.
    pub fn from_error(title: impl Into<String>, error: &Error) -> Self {
        Self::new(title, capitalize(&error.to_string()))
    }
}

/// `sentence` with its first letter capitalized: error messages are written to
/// follow a colon, but notices show them on their own.
pub(crate) fn capitalize(sentence: &str) -> String {
    let mut chars = sentence.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The open alerts.
#[derive(Debug, Default)]
pub struct State {
    notices: HashMap<window::Id, Notice>,
}

impl State {
    /// The notices of the open alerts, in no particular order.
    #[cfg(test)]
    pub fn notices(&self) -> impl Iterator<Item = &Notice> {
        self.notices.values()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The OK button of an alert window.
    Dismiss(window::Id),
}

/// Shows `notice` in a new alert window and logs it. Returns the task that opens
/// (and focuses) the window; return it from `update`.
pub fn report_error(app: &mut App, notice: Notice) -> Task<AppMessage> {
    tracing::warn!(title = %notice.title, body = %notice.body, "reporting to the user");
    let (id, open) = app.windows.open(
        WindowKind::Alert,
        window::Settings {
            size: Size::new(420.0, 180.0),
            resizable: false,
            minimizable: false,
            level: window::Level::AlwaysOnTop,
            icon: windows::icon(),
            ..window::Settings::default()
        },
    );
    app.alert.notices.insert(id, notice);
    // The app has no Dock icon, so bring the window forward explicitly.
    open.discard().chain(window::gain_focus(id))
}

pub fn boot(_app: &mut App) -> Task<AppMessage> {
    Task::none()
}

pub fn update(_app: &mut App, message: Message) -> Task<AppMessage> {
    match message {
        Message::Dismiss(id) => window::close(id),
    }
}

pub fn subscription(_app: &App) -> Subscription<AppMessage> {
    Subscription::none()
}

pub fn view(app: &App, id: window::Id) -> Element<'_, AppMessage> {
    let Some(notice) = app.alert.notices.get(&id) else {
        return space().into();
    };
    let ok = button(text("OK"))
        .style(button::primary)
        .padding([6, 24])
        .on_press(AppMessage::Alert(Message::Dismiss(id)));
    container(
        column![
            text(&notice.title).size(18),
            text(&notice.body),
            space().height(Length::Fill),
            row![space().width(Length::Fill), ok],
        ]
        .spacing(10),
    )
    .padding(20)
    .into()
}

pub fn window_closed(app: &mut App, id: window::Id) -> Task<AppMessage> {
    app.alert.notices.remove(&id);
    Task::none()
}

#[cfg(test)]
mod tests {
    use chartreuse_core::permission::Permission;

    use super::*;

    #[test]
    fn notices_from_errors_read_as_sentences() {
        let notice = Notice::from_error(
            "Capture failed",
            &Error::PermissionDenied(Permission::ScreenRecording),
        );
        assert_eq!(
            notice.body,
            "Chartreuse does not have Screen Recording permission"
        );
        let notice = Notice::from_error("Nothing to open", &Error::ClipboardEmpty);
        assert_eq!(notice.body, "The clipboard does not contain an image");
    }

    #[test]
    fn each_report_opens_its_own_alert_until_it_closes() {
        let (mut app, _fake) = App::for_test();
        let _ = report_error(&mut app, Notice::new("First", "one"));
        let _ = report_error(&mut app, Notice::new("Second", "two"));
        let alerts: Vec<window::Id> = app.windows.of_kind(WindowKind::Alert).collect();
        assert_eq!(alerts.len(), 2);

        let first = alerts
            .iter()
            .copied()
            .find(|id| app.alert.notices[id].title == "First")
            .unwrap();
        let _ = app.update(AppMessage::WindowClosed(first));
        assert_eq!(app.windows.of_kind(WindowKind::Alert).count(), 1);
        assert!(!app.alert.notices.contains_key(&first));
    }

    #[test]
    fn an_alert_shows_the_app_icon() {
        let (mut app, _fake) = App::for_test();
        let open = report_error(&mut app, Notice::new("First", "one"));
        assert_eq!(windows::opened_with_icon(open), [true]);
    }
}
