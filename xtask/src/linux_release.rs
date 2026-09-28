//! `cargo xtask release` on Linux: the release-flavor executable with the
//! license, the readme, and the app icon in the freedesktop `hicolor` layout,
//! in a `.tar.gz`.

use std::path::{Path, PathBuf};

use chartreuse_core::flavor::Flavor;

use crate::icon;
use crate::release::{archive_dir, archive_stem, build_executable, stage, VERSION};
use crate::util::Result;

pub fn release(dist: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    let executable = build_executable()?;
    let stem = archive_stem(VERSION, "linux", std::env::consts::ARCH, false);
    let staging = stage(&stem, &executable)?;
    let flavor = Flavor::Release;
    icon::write_linux_icons(
        flavor.accent(),
        &staging.join("icons").join("hicolor"),
        flavor.bundle_id(),
    )?;
    let archive = dist.join(format!("{stem}.{extension}"));
    archive_dir(&staging, &archive)?;
    Ok(vec![archive])
}
