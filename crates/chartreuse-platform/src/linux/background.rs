//! Launch at login inside a Flatpak: the Background portal, which writes the
//! host's autostart entry for a sandboxed app (see
//! [`launch_at_login`](super::launch_at_login)).

use ashpd::desktop::background::Background;
use chartreuse_core::{flavor, Result};

use super::logic::background::{outcome, Answer};
use super::portal;

/// Asks the Background portal to start `command` (the executable, by its
/// path in the sandbox) at login, or not to, and waits for its answer. It
/// answers at once, unless the user has told the desktop to ask them first:
/// then it waits for their choice in the desktop's dialog.
pub fn request_autostart(enable: bool, command: &str) -> Result<()> {
    futures::executor::block_on(request(enable, command))
}

async fn request(enable: bool, command: &str) -> Result<()> {
    // The dialog's explanation, if the desktop asks.
    let reason = format!(
        "{} starts when you log in, so its capture hotkeys work right away.",
        flavor::DISPLAY_NAME
    );
    let response = Background::request()
        .reason(reason.as_str())
        .auto_start(enable)
        .command([command])
        .dbus_activatable(false)
        .send()
        .await
        .and_then(|request| request.response());
    let answer = match response {
        Ok(background) => Answer::Allowed {
            autostart: background.auto_start(),
        },
        Err(error) if portal::cancelled(&error) => Answer::Denied,
        Err(error) => return Err(portal::error("opening at login", &error)),
    };
    outcome(enable, answer)
}
