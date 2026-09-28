//! `cargo xtask release`: the release build for the host platform, archived
//! into `target/dist/` (which `cargo xtask upload-release` uploads).
//!
//! - macOS ([`crate::macos_release`]): a universal, release-flavor
//!   `Chartreuse.app` signed with the Developer ID identity and notarized, in
//!   a signed, notarized disk image (both stapled).
//! - Windows and Linux: the optimized, release-flavor executable with the
//!   license, the readme, and the app icon, in a `.zip` (Windows) or `.tar.gz`
//!   (Linux) holding one top-level directory. The icon is `chartreuse.ico` on
//!   Windows and an `icons/hicolor/<size>x<size>/apps/<app ID>.png` tree on
//!   Linux. Tracks 5B and 5C add an installer and packages, which install it.
//!
//! Archives are named `Chartreuse-<version>-<os>-<arch>` (the arch is
//! `universal` on macOS), with `-unsigned` appended for an ad-hoc signed macOS
//! app.

use std::path::{Path, PathBuf};
use std::process::Command;

use chartreuse_core::flavor::Flavor;

use crate::bundle::{self, Profile};
use crate::util::{run, target_dir, tool, workspace_root, Context, Error, Result};
use crate::{icon, macos_release};

/// The workspace version, which releases are built as and named after.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Files from the workspace root shipped next to the Windows and Linux
/// executable.
const DOCUMENTS: [&str; 2] = ["LICENSE", "README.md"];

/// The Windows app icon's name in the archive.
const WINDOWS_ICON: &str = "chartreuse.ico";

/// The file extension of an OS's release archive.
pub fn archive_extension(os: &str) -> Result<&'static str> {
    match os {
        "macos" => Ok("dmg"),
        "windows" => Ok("zip"),
        "linux" => Ok("tar.gz"),
        os => Err(Error(format!(
            "`cargo xtask release` has no steps for {os}"
        ))),
    }
}

/// A release archive's name without its extension, which is also the name of
/// the directory inside a Windows or Linux archive. `unsigned` marks an ad-hoc
/// signed build.
#[must_use]
pub fn archive_stem(version: &str, os: &str, arch: &str, unsigned: bool) -> String {
    let suffix = if unsigned { "-unsigned" } else { "" };
    format!("Chartreuse-{version}-{os}-{arch}{suffix}")
}

/// The directory `release` archives into and `upload-release` uploads from.
pub fn dist_dir() -> PathBuf {
    target_dir().join("dist")
}

/// `--allow-ad-hoc` applies to macOS, the only platform that signs so far.
pub fn release(allow_ad_hoc: bool) -> Result {
    let os = std::env::consts::OS;
    let extension = archive_extension(os)?;
    // Emptied first, so it holds this build's archive and nothing older.
    let dist = dist_dir();
    if dist.exists() {
        std::fs::remove_dir_all(&dist).context(|| format!("removing {}", dist.display()))?;
    }
    std::fs::create_dir_all(&dist).context(|| format!("creating {}", dist.display()))?;

    let archive = if os == "macos" {
        macos_release::release(allow_ad_hoc, &dist, extension)?
    } else {
        release_executable(os, &dist, extension)?
    };
    eprintln!("release archive: {}", archive.display());
    Ok(())
}

/// Windows and Linux: the executable, [`DOCUMENTS`], and the app icon, archived.
fn release_executable(os: &str, dist: &Path, extension: &str) -> Result<PathBuf> {
    run(&mut bundle::build_command(
        Profile::Release,
        Flavor::Release,
    ))?;
    let profile_dir = target_dir().join(Profile::Release.dir_name());
    let stem = archive_stem(VERSION, os, std::env::consts::ARCH, false);

    // Staged beside the build output: `dist` holds only archives.
    let staging = profile_dir.join(&stem);
    if staging.exists() {
        std::fs::remove_dir_all(&staging).context(|| format!("removing {}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging).context(|| format!("creating {}", staging.display()))?;
    let executable = format!("{}{}", bundle::EXECUTABLE, std::env::consts::EXE_SUFFIX);
    let files = std::iter::once((profile_dir.join(&executable), executable.as_str()))
        .chain(DOCUMENTS.map(|name| (workspace_root().join(name), name)));
    for (from, name) in files {
        std::fs::copy(&from, staging.join(name))
            .context(|| format!("copying {}", from.display()))?;
    }
    let flavor = Flavor::Release;
    match os {
        "windows" => icon::build_ico(flavor.accent(), &staging.join(WINDOWS_ICON))?,
        "linux" => icon::write_linux_icons(
            flavor.accent(),
            &staging.join("icons").join("hicolor"),
            flavor.bundle_id(),
        )?,
        _ => {}
    }

    let archive = dist.join(format!("{stem}.{extension}"));
    archive_dir(&staging, &archive)?;
    Ok(archive)
}

/// Archives `dir` as the single top-level directory of `archive`, in the
/// format its extension names (`.zip`, `.tar.gz`).
fn archive_dir(dir: &Path, archive: &Path) -> Result {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return Err(Error(format!("cannot archive {}", dir.display())));
    };
    run(tar()
        .args(["--auto-compress", "-c", "-f"])
        .arg(archive)
        .arg("-C")
        .arg(parent)
        .arg(name))
}

/// `tar`; on Windows, the bsdtar that ships with the OS, by path. The `tar`
/// first on a Windows `PATH` can be GNU tar (from Git, on CI runners), which
/// cannot write zip files and reads `C:\…` as a remote host.
fn tar() -> Command {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        tool(Path::new(&root).join("System32").join("tar.exe"))
    } else {
        tool("tar")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::capture;

    #[test]
    fn archive_names_tell_platforms_apart() {
        let name = |os, arch, unsigned| {
            format!(
                "{}.{}",
                archive_stem("0.2.0", os, arch, unsigned),
                archive_extension(os).unwrap()
            )
        };
        assert_eq!(
            name("macos", "universal", false),
            "Chartreuse-0.2.0-macos-universal.dmg"
        );
        assert_eq!(
            name("macos", "universal", true),
            "Chartreuse-0.2.0-macos-universal-unsigned.dmg"
        );
        assert_eq!(
            name("windows", "x86_64", false),
            "Chartreuse-0.2.0-windows-x86_64.zip"
        );
        assert_eq!(
            name("linux", "x86_64", false),
            "Chartreuse-0.2.0-linux-x86_64.tar.gz"
        );
    }

    #[test]
    fn unsupported_os_has_no_release() {
        let error = archive_extension("freebsd").unwrap_err();
        assert!(error.0.contains("freebsd"), "{error}");
    }

    /// Archives a staged directory with the host's `tar` and lists the result.
    fn archive_and_list(extension: &str) -> Vec<String> {
        let scratch = std::env::temp_dir().join(format!(
            "chartreuse-xtask-{}-archive-{extension}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        let staging = scratch.join("Chartreuse-0.2.0-test");
        std::fs::create_dir_all(&staging).unwrap();
        for name in ["chartreuse", "LICENSE"] {
            std::fs::write(staging.join(name), name).unwrap();
        }
        let archive = scratch.join(format!("out.{extension}"));

        archive_dir(&staging, &archive).unwrap();

        let listing = capture(tar().arg("-t").arg("-f").arg(&archive)).unwrap();
        std::fs::remove_dir_all(&scratch).unwrap();
        let mut entries: Vec<String> = String::from_utf8(listing.stdout)
            .unwrap()
            .lines()
            .map(|line| line.trim_end_matches('/').to_owned())
            .collect();
        entries.sort();
        entries
    }

    const EXPECTED_ENTRIES: [&str; 3] = [
        "Chartreuse-0.2.0-test",
        "Chartreuse-0.2.0-test/LICENSE",
        "Chartreuse-0.2.0-test/chartreuse",
    ];

    #[test]
    fn tar_gz_holds_one_top_level_directory() {
        assert_eq!(archive_and_list("tar.gz"), EXPECTED_ENTRIES);
    }

    // GNU tar (Linux) cannot write zip files; Windows and macOS ship bsdtar.
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn zip_holds_one_top_level_directory() {
        assert_eq!(archive_and_list("zip"), EXPECTED_ENTRIES);
    }
}
