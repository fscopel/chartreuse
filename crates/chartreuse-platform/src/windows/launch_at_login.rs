//! Windows: launch at login. Not implemented yet.

use chartreuse_core::{Error, Result};

use crate::launch_at_login::LaunchAtLogin;

/// The Windows [`LaunchAtLogin`] backend.
#[derive(Debug, Default)]
pub struct WindowsLaunchAtLogin;

impl WindowsLaunchAtLogin {
    pub fn new() -> Self {
        Self
    }
}

impl LaunchAtLogin for WindowsLaunchAtLogin {
    fn status(&self) -> Result<bool> {
        Err(Error::Unsupported("launch at login"))
    }

    fn set(&self, _enabled: bool) -> Result<()> {
        Err(Error::Unsupported("launch at login"))
    }
}
