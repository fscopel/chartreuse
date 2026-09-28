//! `cargo xtask release` on Windows: the release-flavor executable, which
//! carries the app icon as a resource, with the license and the readme in a
//! `.zip`.

use std::path::{Path, PathBuf};

use crate::release::{archive_dir, archive_stem, build_executable, stage, VERSION};
use crate::util::Result;

pub fn release(dist: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    let executable = build_executable()?;
    let stem = archive_stem(VERSION, "windows", std::env::consts::ARCH, false);
    let staging = stage(&stem, &executable)?;
    let archive = dist.join(format!("{stem}.{extension}"));
    archive_dir(&staging, &archive)?;
    Ok(vec![archive])
}
