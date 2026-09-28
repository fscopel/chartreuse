//! `cargo xtask ci-install-tools`: installs the packaging tools that `cargo
//! xtask release` needs and a CI runner lacks, so the release workflow stays a
//! list of `cargo xtask` commands.
//!
//! - Windows: Inno Setup 6, with Chocolatey (preinstalled on GitHub's
//!   runners), unless it is installed already. GitHub dropped it from the
//!   Windows Server 2025 image.
//! - macOS: nothing; the release uses Xcode's tools.

use crate::util::{run, tool, Result};
use crate::windows_release;

pub fn ci_install_tools() -> Result {
    match std::env::consts::OS {
        "windows" => install_inno_setup(),
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
