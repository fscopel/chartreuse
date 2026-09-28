//! What the Background portal's answer to a request to start the app at
//! login, or not to, means.

use chartreuse_core::{flavor, Error, Result};

/// The Background portal's answer to a `RequestBackground` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The app may run in the background; `autostart` is whether the portal
    /// now starts it at login.
    Allowed { autostart: bool },
    /// The app may not run in the background: the user said no, now or
    /// earlier. The portal then turns autostart off.
    Denied,
}

/// Whether the portal did as asked, given its `answer` to a request to
/// start the app at login (`enable`), or not to. A denied request to turn
/// autostart off is done, since a denial turns it off.
pub fn outcome(enable: bool, answer: Answer) -> Result<()> {
    match answer {
        Answer::Allowed { autostart } if autostart == enable => Ok(()),
        Answer::Denied if !enable => Ok(()),
        Answer::Denied => Err(Error::Platform(format!(
            "the desktop does not allow {name} to run in the background, which opening at \
             login needs: allow it in the desktop's settings for {name}, or run `flatpak \
             permission-set background background {id} yes`",
            name = flavor::DISPLAY_NAME,
            id = flavor::BUNDLE_ID,
        ))),
        Answer::Allowed { .. } => Err(Error::Platform(format!(
            "the desktop's Background portal did not turn opening at login {}",
            if enable { "on" } else { "off" }
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_done_only_if_the_portal_did_as_asked() {
        for enable in [true, false] {
            assert!(outcome(enable, Answer::Allowed { autostart: enable }).is_ok());
            assert!(outcome(enable, Answer::Allowed { autostart: !enable }).is_err());
        }
    }

    #[test]
    fn a_denial_fails_turning_autostart_on_but_not_off() {
        assert!(outcome(true, Answer::Denied).is_err());
        assert!(outcome(false, Answer::Denied).is_ok());
    }
}
