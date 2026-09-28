//! macOS: the app as its own login item, through ServiceManagement's
//! `SMAppService.mainAppService` (macOS 13 and later).
//!
//! The service is the running app bundle, so this works only when Chartreuse
//! runs from its bundle (`cargo xtask run`, or an installed release); a bare
//! `cargo run` binary has no bundle, and both calls then fail saying so. macOS
//! keeps one login item per bundle identifier, so Chartreuse and Chartreuse
//! Dev are separate items, each starting its own bundle.
//!
//! # Statuses
//!
//! - *Enabled*: registered, and the user lets it run. The only status that
//!   starts the app at login, so the only one [`LaunchAtLogin::status`] reports
//!   as `true`.
//! - *Not registered*: never registered, or unregistered.
//! - *Requires approval*: registered, but the user has to allow it in System
//!   Settings > General > Login Items first, typically because they turned it
//!   off there. It does not start the app, so it is `false`. Registering again
//!   leaves it waiting for approval: [`LaunchAtLogin::set`] then opens the
//!   Login Items settings, as Apple recommends when the user has asked for the
//!   item, and fails with an error telling the user to turn Chartreuse on
//!   there.
//! - *Not found*: ServiceManagement could not find the service. An error.
//!
//! `register` and `unregister` fail when the item already has the state asked
//! for, and their errors carry no stable codes for that, so [`set`] goes by
//! the status afterwards: it succeeds if the status is what was asked for,
//! whatever the call returned.
//!
//! [`set`]: LaunchAtLogin::set

use chartreuse_core::{flavor, Error, Result};
use objc2_foundation::NSBundle;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

use crate::launch_at_login::LaunchAtLogin;

/// The macOS [`LaunchAtLogin`] backend.
#[derive(Debug, Default)]
pub struct MacosLaunchAtLogin;

impl MacosLaunchAtLogin {
    pub fn new() -> Self {
        Self
    }
}

impl LaunchAtLogin for MacosLaunchAtLogin {
    fn status(&self) -> Result<bool> {
        let service = main_app_service()?;
        // SAFETY: `status` takes no arguments and only queries the service.
        starts_at_login(unsafe { service.status() })
    }

    fn set(&self, enabled: bool) -> Result<()> {
        let service = main_app_service()?;
        // SAFETY: both methods take no arguments besides the error out
        // parameter, which objc2 manages.
        let attempt = unsafe {
            if enabled {
                service.registerAndReturnError()
            } else {
                service.unregisterAndReturnError()
            }
        }
        .map_err(|error| error.localizedDescription().to_string());
        // SAFETY: as in `status`.
        let status = unsafe { service.status() };
        tracing::info!(enabled, ?attempt, ?status, "changed the login item");
        match outcome(enabled, attempt, status) {
            Ok(()) => Ok(()),
            Err(Failure::NeedsApproval) => {
                // SAFETY: takes no arguments; opens System Settings.
                unsafe { SMAppService::openSystemSettingsLoginItems() };
                Err(Error::Platform(format!(
                    "macOS needs your approval to open {} at login: turn it on in the Login \
                     Items settings that just opened",
                    flavor::DISPLAY_NAME
                )))
            }
            Err(Failure::Refused(reason)) => Err(Error::Platform(reason)),
        }
    }
}

/// The login item of the running app bundle; an error if the app does not
/// run from a bundle.
fn main_app_service() -> Result<objc2::rc::Retained<SMAppService>> {
    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        return Err(Error::Platform(format!(
            "{} can open at login only when it runs from its app bundle",
            flavor::DISPLAY_NAME
        )));
    }
    // SAFETY: takes no arguments; returns the main app's service object.
    Ok(unsafe { SMAppService::mainAppService() })
}

/// Whether a login item with `status` starts the app at login.
fn starts_at_login(status: SMAppServiceStatus) -> Result<bool> {
    match status {
        SMAppServiceStatus::Enabled => Ok(true),
        SMAppServiceStatus::NotRegistered | SMAppServiceStatus::RequiresApproval => Ok(false),
        SMAppServiceStatus::NotFound => Err(Error::Platform(format!(
            "macOS cannot find the login item of {}",
            flavor::DISPLAY_NAME
        ))),
        SMAppServiceStatus(other) => Err(Error::Platform(format!(
            "macOS reports an unknown login item status ({other})"
        ))),
    }
}

/// Why [`outcome`] failed.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// Registered, but the user has to allow it in System Settings.
    NeedsApproval,
    /// The user-facing reason.
    Refused(String),
}

/// The result of registering (`enabled`) or unregistering the login item:
/// `attempt` is what the call returned (its error's description), `status`
/// the item's status afterwards, which decides (see [Statuses](self#statuses)).
fn outcome(
    enabled: bool,
    attempt: std::result::Result<(), String>,
    status: SMAppServiceStatus,
) -> std::result::Result<(), Failure> {
    let name = flavor::DISPLAY_NAME;
    let because = |attempt: std::result::Result<(), String>| match attempt {
        Ok(()) => String::new(),
        Err(reason) => format!(": {reason}"),
    };
    match (enabled, status) {
        (true, SMAppServiceStatus::Enabled) => Ok(()),
        (true, SMAppServiceStatus::RequiresApproval) => Err(Failure::NeedsApproval),
        (true, _) => Err(Failure::Refused(format!(
            "macOS did not add {name} to the login items{}",
            because(attempt)
        ))),
        (false, SMAppServiceStatus::Enabled) => Err(Failure::Refused(format!(
            "macOS did not remove {name} from the login items{}",
            because(attempt)
        ))),
        // Not registered, waiting for an approval it will not get, or gone:
        // it will not start the app.
        (false, _) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn denied(reason: &str) -> std::result::Result<(), String> {
        Err(reason.to_owned())
    }

    #[test]
    fn only_an_enabled_item_starts_the_app() {
        assert!(starts_at_login(SMAppServiceStatus::Enabled).unwrap());
        assert!(!starts_at_login(SMAppServiceStatus::NotRegistered).unwrap());
        assert!(!starts_at_login(SMAppServiceStatus::RequiresApproval).unwrap());
        assert!(starts_at_login(SMAppServiceStatus::NotFound).is_err());
        assert!(starts_at_login(SMAppServiceStatus(42)).is_err());
    }

    #[test]
    fn the_status_afterwards_decides_even_when_the_call_failed() {
        // Registering an item that is already registered fails, but it is on.
        assert_eq!(
            outcome(
                true,
                denied("already registered"),
                SMAppServiceStatus::Enabled
            ),
            Ok(())
        );
        assert_eq!(
            outcome(
                false,
                denied("not registered"),
                SMAppServiceStatus::NotRegistered
            ),
            Ok(())
        );
        assert_eq!(
            outcome(false, Ok(()), SMAppServiceStatus::RequiresApproval),
            Ok(()),
            "an item waiting for approval does not start the app"
        );
    }

    #[test]
    fn an_item_waiting_for_approval_asks_the_user_to_approve_it() {
        for attempt in [Ok(()), denied("Operation not permitted")] {
            assert_eq!(
                outcome(true, attempt, SMAppServiceStatus::RequiresApproval),
                Err(Failure::NeedsApproval)
            );
        }
    }

    #[test]
    fn a_refusal_says_why() {
        let Err(Failure::Refused(reason)) = outcome(
            true,
            denied("Operation not permitted"),
            SMAppServiceStatus::NotRegistered,
        ) else {
            panic!("expected a refusal");
        };
        assert!(
            reason.ends_with("login items: Operation not permitted"),
            "{reason}"
        );

        let Err(Failure::Refused(reason)) = outcome(false, Ok(()), SMAppServiceStatus::Enabled)
        else {
            panic!("expected a refusal");
        };
        assert!(reason.ends_with("from the login items"), "{reason}");
    }
}
