//! `cargo xtask ci-install-tools`: installs the packaging tools that `cargo
//! xtask release` needs and a CI runner lacks, so the release workflow stays a
//! list of `cargo xtask` commands.
//!
//! - Windows: Inno Setup 6, with Chocolatey (preinstalled on GitHub's
//!   runners), unless it is installed already. GitHub dropped it from the
//!   Windows Server 2025 image.
//! - Linux (Debian or Ubuntu, with passwordless `sudo`, as on GitHub's
//!   runners): `flatpak` and `flatpak-builder` with apt, unless installed.
//!   Ubuntu 24.04's AppArmor also stops unprivileged programs from creating
//!   user namespaces, which `bwrap`, flatpak-builder's sandbox, needs; on a
//!   machine where that restriction is on, it is turned off until reboot.
//! - macOS: nothing; the release uses Xcode's tools.

use std::path::Path;

use crate::util::{capture, run, tool, Result};
use crate::windows_release;

/// Ubuntu's switch for AppArmor's restriction on unprivileged user
/// namespaces (`1`: restricted).
const USERNS_RESTRICTION: &str = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";

pub fn ci_install_tools() -> Result {
    match std::env::consts::OS {
        "windows" => install_inno_setup(),
        "linux" => install_flatpak_builder(),
        os => {
            eprintln!("ci-install-tools: nothing to install on {os}");
            Ok(())
        }
    }
}

fn install_inno_setup() -> Result {
    if let Ok(iscc) = windows_release::iscc() {
        eprintln!(
            "ci-install-tools: Inno Setup is installed: {}",
            iscc.display()
        );
        return Ok(());
    }
    run(tool("choco").args(["install", "innosetup", "--yes", "--no-progress"]))?;
    let iscc = windows_release::iscc()?;
    eprintln!("ci-install-tools: installed {}", iscc.display());
    Ok(())
}

fn install_flatpak_builder() -> Result {
    if capture(tool("flatpak-builder").arg("--version")).is_ok() {
        eprintln!("ci-install-tools: flatpak-builder is installed");
    } else {
        run(tool("sudo").args(["apt-get", "update", "--quiet"]))?;
        run(tool("sudo").args([
            "apt-get",
            "install",
            "--yes",
            "--quiet",
            "flatpak",
            "flatpak-builder",
        ]))?;
    }
    let restricted = std::fs::read_to_string(Path::new(USERNS_RESTRICTION))
        .is_ok_and(|value| value.trim() == "1");
    if restricted {
        eprintln!(
            "ci-install-tools: allowing unprivileged user namespaces for bwrap \
             (flatpak-builder's sandbox)"
        );
        run(tool("sudo").args([
            "sysctl",
            "--write",
            "kernel.apparmor_restrict_unprivileged_userns=0",
        ]))?;
    }
    Ok(())
}
