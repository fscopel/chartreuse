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
//! Only std file I/O, so test builds on every host compile this and its tests
//! run everywhere.
//!
//! Limitation: inside a Flatpak sandbox the entry would name the sandbox's
//! path; a Flatpak build needs the Background portal instead.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use chartreuse_core::{flavor, Error, Result};

use super::logic::autostart;
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
        status(&entry_path()?)
    }

    fn set(&self, enabled: bool) -> Result<()> {
        let path = entry_path()?;
        if enabled {
            let exe = std::env::current_exe()
                .map_err(|error| Error::io("finding the Chartreuse executable", error))?;
            enable(&path, &exe)
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

/// Writes the entry at `path`, starting `exe`, creating its folder if need be.
fn enable(path: &Path, exe: &Path) -> Result<()> {
    let exe = exe.to_str().ok_or_else(|| {
        Error::Platform(format!(
            "the path of the Chartreuse executable is not valid UTF-8, which autostart entries \
             need: {}",
            exe.display()
        ))
    })?;
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
