//! Wayland: privacy permissions. Screen capture goes through the Screenshot
//! portal, which asks the user itself when Chartreuse captures (and remembers
//! the answer per app), so there is nothing to check or request beforehand:
//! the status is reported as granted, and a refusal surfaces as the capture
//! failing with an error that says so (see `capture.rs`). Where the answer
//! is kept depends on the desktop (GNOME: Settings → Apps), so there is no
//! settings page to open.

use chartreuse_core::permission::{Permission, PermissionStatus};
use chartreuse_core::{Error, Result};

use crate::permissions::Permissions;

/// The Wayland [`Permissions`] backend.
#[derive(Debug, Default)]
pub struct WaylandPermissions;

impl WaylandPermissions {
    pub fn new() -> Self {
        Self
    }
}

impl Permissions for WaylandPermissions {
    fn status(&self, _permission: Permission) -> Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    fn request(&self, permission: Permission) -> Result<PermissionStatus> {
        self.status(permission)
    }

    fn open_settings(&self, _permission: Permission) -> Result<()> {
        Err(Error::Unsupported(
            "opening the screen capture permission settings",
        ))
    }
}
