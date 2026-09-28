//! `cargo xtask release`: the release build for the host platform, archived
//! into `target/dist/` (which `cargo xtask upload-release` uploads).
//!
//! - macOS ([`crate::macos_release`]): a universal, release-flavor
//!   `Chartreuse.app` signed with the Developer ID identity and notarized, in
//!   a signed, notarized disk image (both stapled).
//! - Windows ([`crate::windows_release`]) and Linux
//!   ([`crate::linux_release`]): the optimized, release-flavor executable
//!   with the license and the readme, in a `.zip` (Windows) or `.tar.gz`
//!   (Linux) holding one top-level directory. The Windows executable carries
//!   the app icon as a resource; the Linux archive adds an
//!   `icons/hicolor/<size>x<size>/apps/<app ID>.png` tree. Tracks 5B and 5C
//!   add an installer and packages, which install it.
//!
//! Archives are named `Chartreuse-<version>-<os>-<arch>` (the arch is
//! `universal` on macOS), with `-unsigned` appended for an ad-hoc signed macOS
//! app or an unsigned Windows executable.

use std::path::{Path, PathBuf};
use std::process::Command;

use chartreuse_core::flavor::Flavor;

use crate::bundle::{self, Profile};
use crate::util::{run, target_dir, tool, workspace_root, Context, Error, Result};
use crate::{linux_release, macos_release, windows_release};

/// The workspace version, which releases are built as and named after.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Files from the workspace root shipped next to the Windows and Linux
/// executable.
pub const DOCUMENTS: [&str; 2] = ["LICENSE", "README.md"];

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

/// `--allow-ad-hoc` applies to the platforms that sign: macOS and Windows.
pub fn release(allow_ad_hoc: bool) -> Result {
    let os = std::env::consts::OS;
    let extension = archive_extension(os)?;
    // Emptied first, so it holds this build's archive and nothing older.
    let dist = dist_dir();
    if dist.exists() {
        std::fs::remove_dir_all(&dist).context(|| format!("removing {}", dist.display()))?;
    }
    std::fs::create_dir_all(&dist).context(|| format!("creating {}", dist.display()))?;

    let artifacts = match os {
        "macos" => vec![macos_release::release(allow_ad_hoc, &dist, extension)?],
        "windows" => windows_release::release(allow_ad_hoc, &dist, extension)?,
        _ => linux_release::release(&dist, extension)?,
    };
    for artifact in artifacts {
        eprintln!("release artifact: {}", artifact.display());
    }
    Ok(())
}

/// Windows and Linux: builds the optimized, release-flavor executable and
/// returns its path.
pub fn build_executable() -> Result<PathBuf> {
    run(&mut bundle::build_command(
        Profile::Release,
        Flavor::Release,
    ))?;
    let executable = format!("{}{}", bundle::EXECUTABLE, std::env::consts::EXE_SUFFIX);
    Ok(target_dir()
        .join(Profile::Release.dir_name())
        .join(executable))
}

/// Windows and Linux: copies `executable` and [`DOCUMENTS`] into a fresh
/// directory named `stem`, beside the build output (`dist` holds only
/// release artifacts), and returns it.
pub fn stage(stem: &str, executable: &Path) -> Result<PathBuf> {
    let staging = target_dir().join(Profile::Release.dir_name()).join(stem);
    if staging.exists() {
        std::fs::remove_dir_all(&staging).context(|| format!("removing {}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging).context(|| format!("creating {}", staging.display()))?;
    let name = executable.file_name().unwrap_or_default();
    std::fs::copy(executable, staging.join(name))
        .context(|| format!("copying {}", executable.display()))?;
    for name in DOCUMENTS {
        let from = workspace_root().join(name);
        std::fs::copy(&from, staging.join(name))
            .context(|| format!("copying {}", from.display()))?;
    }
    Ok(staging)
}

/// Archives `dir` as the single top-level directory of `archive`, in the
/// format its extension names (`.zip`, `.tar.gz`).
pub fn archive_dir(dir: &Path, archive: &Path) -> Result {
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
