//! Launch at login for both Linux backends. Not implemented yet.

use chartreuse_core::{Error, Result};

use crate::launch_at_login::LaunchAtLogin;

/// The [`LaunchAtLogin`] backend of X11 and Wayland sessions.
#[derive(Debug, Default)]
pub struct LinuxLaunchAtLogin;

impl LinuxLaunchAtLogin {
    pub fn new() -> Self {
        Self
    }
}

impl LaunchAtLogin for LinuxLaunchAtLogin {
    fn status(&self) -> Result<bool> {
        Err(Error::Unsupported("launch at login"))
    }

    fn set(&self, _enabled: bool) -> Result<()> {
        Err(Error::Unsupported("launch at login"))
    }
}
