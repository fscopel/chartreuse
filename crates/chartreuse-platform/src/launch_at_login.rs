//! Starting Chartreuse when the user logs in.

use chartreuse_core::Result;

/// Reads and changes whether the OS starts Chartreuse at login.
///
/// The OS keeps this setting, and the user can change it there too (macOS
/// System Settings > General > Login Items, the Windows Task Manager's Startup
/// apps, a Linux desktop's autostart settings), so [`status`](Self::status)
/// reads it afresh every time instead of remembering what `set` was told.
///
/// Call from the main thread (iced `boot`/`update`). Both calls are quick: at
/// most a registry value, a small file, or one ServiceManagement call.
pub trait LaunchAtLogin {
    /// Whether the OS will start this app at the next login. A login item
    /// the user has to approve, or has turned off in the OS's own settings,
    /// does not start, so it is `false`.
    fn status(&self) -> Result<bool>;

    /// Asks the OS to start this app at login (`true`), or not to (`false`).
    /// Setting the state the OS already has succeeds. The OS may refuse; the
    /// error then says what the user can do about it (on macOS, for example,
    /// approve the login item in System Settings).
    fn set(&self, enabled: bool) -> Result<()>;
}
