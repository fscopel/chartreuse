//! Launch at login for both Linux backends: an XDG autostart entry,
//! `$XDG_CONFIG_HOME/autostart/<bundle id>.desktop` (see
//! [`logic::autostart`](super::logic::autostart)), which every desktop that
//! follows the Desktop Application Autostart Specification runs at login,
//! under X11 and Wayland alike.
//!
//! The entry starts the executable that turned it on, by its absolute path;
//! turning it off and on again points it at the running copy. The user can
//! turn it off in their desktop's startup settings, which set `Hidden=true`
//! or `X-GNOME-Autostart-enabled=false` in it (or delete it): it then counts
//! as off. Turning it on rewrites it, and turning it off deletes it.
//!
//! Inside a Flatpak, the sandbox has its own `$XDG_CONFIG_HOME`, which the
//! host's session never reads, and the executable's path is the sandbox's.
//! There the Background portal writes the entry instead, on the host, running
//! the app with `flatpak run` (`background.rs`). The sandbox cannot read that
//! entry back, so [`status`](crate::LaunchAtLogin::status) fails and the app
//! keeps the setting as last chosen in it: turning the entry off in the
//! desktop's startup settings does not show in Chartreuse.
//!
//! Apart from the trait implementation, which calls the portal, only std file
//! I/O, so test builds on every host compile this and its tests run
//! everywhere.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use chartreuse_core::{flavor, Error, Result};

use super::logic::autostart;

/// Present in every Flatpak sandbox, and nowhere else.
const FLATPAK_INFO: &str = "/.flatpak-info";

/// The [`LaunchAtLogin`](crate::LaunchAtLogin) backend of X11 and Wayland
/// sessions.
#[derive(Debug)]
pub struct LinuxLaunchAtLogin {
    /// Whether Chartreuse runs inside a Flatpak sandbox.
    flatpak: bool,
}

impl LinuxLaunchAtLogin {
    pub fn new() -> Self {
        Self {
            flatpak: Path::new(FLATPAK_INFO).exists(),
        }
    }
}

impl Default for LinuxLaunchAtLogin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
impl crate::LaunchAtLogin for LinuxLaunchAtLogin {
    fn status(&self) -> Result<bool> {
        if self.flatpak {
            return Err(Error::Platform(
                "inside a Flatpak, the autostart entry is outside the sandbox and cannot be \
                 read"
                    .into(),
            ));
        }
        status(&entry_path()?)
    }

    fn set(&self, enabled: bool) -> Result<()> {
        let exe = || {
            std::env::current_exe()
                .map_err(|error| Error::io("finding the Chartreuse executable", error))
        };
        if self.flatpak {
            return super::background::request_autostart(enabled, utf8(&exe()?)?);
        }
        let path = entry_path()?;
        if enabled {
            enable(&path, &exe()?)
        } else {
            disable(&path)
        }
    }
}

/// This app's autostart entry, from the environment.
fn entry_path() -> Result<std::path::PathBuf> {
    autostart::entry_path(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        flavor::BUNDLE_ID,
    )
    .ok_or_else(|| {
        Error::Platform(
            "cannot find the autostart folder: neither XDG_CONFIG_HOME nor HOME is an absolute \
             path"
                .into(),
        )
    })
}

/// Whether the entry at `path` exists and is turned on.
fn status(path: &Path) -> Result<bool> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(autostart::enabled(&contents)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::io(format!("reading {}", path.display()), error)),
    }
}

/// `exe` as UTF-8, which autostart entries (and the portal) need.
fn utf8(exe: &Path) -> Result<&str> {
    exe.to_str().ok_or_else(|| {
        Error::Platform(format!(
            "the path of the Chartreuse executable is not valid UTF-8, which autostart entries \
             need: {}",
            exe.display()
        ))
    })
}

/// Writes the entry at `path`, starting `exe`, creating its folder if need be.
fn enable(path: &Path, exe: &Path) -> Result<()> {
    let exe = utf8(exe)?;
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)
            .map_err(|error| Error::io(format!("creating {}", folder.display()), error))?;
    }
    fs::write(path, autostart::entry(flavor::DISPLAY_NAME, exe))
        .map_err(|error| Error::io(format!("writing {}", path.display()), error))
}

/// Deletes the entry at `path`, if there is one.
fn disable(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::io(format!("deleting {}", path.display()), error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabling_and_disabling_round_trips_through_the_entry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("autostart").join("chartreuse.desktop");
        assert!(!status(&path).unwrap(), "no entry yet");
        disable(&path).unwrap();

        enable(&path, Path::new("/opt/chartreuse/chartreuse")).unwrap();
        assert!(status(&path).unwrap());
        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("Exec=\"/opt/chartreuse/chartreuse\"\n"),
            "{contents}"
        );

        disable(&path).unwrap();
        assert!(!path.exists());
        assert!(!status(&path).unwrap());
    }

    #[test]
    fn enabling_turns_an_entry_the_user_turned_off_back_on() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("chartreuse.desktop");
        let exe = Path::new("/usr/bin/chartreuse");
        enable(&path, exe).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("{contents}Hidden=true\n")).unwrap();
        assert!(!status(&path).unwrap());

        enable(&path, exe).unwrap();
        assert!(status(&path).unwrap());
    }
}
