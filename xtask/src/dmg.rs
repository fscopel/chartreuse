//! The macOS release disk image: the app beside a link to `/Applications`, so
//! it installs by dragging one onto the other.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::util::{run, tool, Context, Error, Result};

/// The name of the link to `/Applications` at the top of the disk image.
const APPLICATIONS_LINK: &str = "Applications";

/// The disk image's contents, staged in a folder that `hdiutil` images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub staging: PathBuf,
    /// Where the app is copied, under its own name.
    pub app: PathBuf,
    /// The symbolic link to `/Applications`.
    pub applications: PathBuf,
}

impl Layout {
    /// The layout for `app` staged in `staging`.
    pub fn new(staging: &Path, app: &Path) -> Result<Self> {
        let name = app
            .file_name()
            .ok_or_else(|| Error(format!("{} has no file name", app.display())))?;
        Ok(Self {
            staging: staging.to_owned(),
            app: staging.join(name),
            applications: staging.join(APPLICATIONS_LINK),
        })
    }
}

/// The `hdiutil` arguments that image `staging` as the compressed, read-only
/// (UDZO) disk image `dmg`, replacing any older one, mounted as `volume`.
#[must_use]
pub fn create_args(volume: &str, staging: &Path, dmg: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["create", "-volname", volume, "-srcfolder"]
        .map(OsString::from)
        .into();
    args.push(staging.into());
    args.extend(["-ov", "-format", "UDZO"].map(OsString::from));
    args.push(dmg.into());
    args
}

/// Builds the disk image `dmg` holding `app` (copied with `ditto`, which keeps
/// its signature and any stapled ticket) and the `/Applications` link, staged
/// in `staging` (emptied first).
pub fn create(app: &Path, volume: &str, staging: &Path, dmg: &Path) -> Result {
    let layout = Layout::new(staging, app)?;
    if staging.exists() {
        std::fs::remove_dir_all(staging).context(|| format!("removing {}", staging.display()))?;
    }
    std::fs::create_dir_all(staging).context(|| format!("creating {}", staging.display()))?;
    run(tool("ditto").arg(app).arg(&layout.app))?;
    symlink_applications(&layout.applications)?;
    run(tool("hdiutil").args(create_args(volume, staging, dmg)))
}

#[cfg(unix)]
fn symlink_applications(link: &Path) -> Result {
    std::os::unix::fs::symlink("/Applications", link)
        .context(|| format!("linking {} to /Applications", link.display()))
}

#[cfg(not(unix))]
fn symlink_applications(_link: &Path) -> Result {
    Err(Error("disk images can only be built on macOS".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_sits_beside_a_link_to_applications() {
        let layout = Layout::new(
            Path::new("/t/dmg"),
            Path::new("/t/universal-apple-darwin/release/Chartreuse.app"),
        )
        .unwrap();
        assert_eq!(layout.app, Path::new("/t/dmg/Chartreuse.app"));
        assert_eq!(layout.applications, Path::new("/t/dmg/Applications"));
    }

    #[test]
    fn the_image_is_compressed_read_only_and_replaces_an_older_one() {
        let args = create_args(
            "Chartreuse",
            Path::new("/t/dmg"),
            Path::new("/t/dist/Chartreuse-0.2.0-macos-universal.dmg"),
        );
        assert_eq!(
            args,
            [
                "create",
                "-volname",
                "Chartreuse",
                "-srcfolder",
                "/t/dmg",
                "-ov",
                "-format",
                "UDZO",
                "/t/dist/Chartreuse-0.2.0-macos-universal.dmg"
            ]
            .map(OsString::from)
        );
    }
}
