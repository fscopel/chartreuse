//! Windows only: embeds the application manifest (`chartreuse.exe.manifest`)
//! and the app icon into the executable. Other targets are left alone.
//!
//! - The manifest: MSVC's linker embeds it itself (`/MANIFEST:EMBED`).
//! - The icon: the flavor's `app-<flavor>.ico`, checked in under
//!   `assets/icon/generated` by `cargo xtask icons` (so this crate needs no SVG
//!   renderer), compiled into a resource with the Windows SDK's `rc.exe` by the
//!   `embed-resource` crate, which finds the SDK the way the MSVC toolchain
//!   does and links the result into the binaries only. The resource script
//!   is written here, so it names the icon by its full path.

use std::path::{Path, PathBuf};

fn main() {
    const MANIFEST: &str = "chartreuse.exe.manifest";
    println!("cargo::rerun-if-changed={MANIFEST}");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" {
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    embed_icon(&app_icon(&manifest_dir));
    if target_env != "msvc" {
        println!(
            "cargo::warning=not embedding {MANIFEST}: only the MSVC linker is supported, so \
             Windows falls back to the DPI awareness winit sets at startup"
        );
        return;
    }
    let manifest = manifest_dir.join(MANIFEST);
    println!("cargo::rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo::rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}

/// The checked-in app icon for the flavor being built (the `release-flavor`
/// feature selects the release flavor, as in `chartreuse_core::flavor`).
fn app_icon(manifest_dir: &Path) -> PathBuf {
    let flavor = if std::env::var_os("CARGO_FEATURE_RELEASE_FLAVOR").is_some() {
        "release"
    } else {
        "development"
    };
    let icon = manifest_dir
        .join("../../assets/icon/generated")
        .join(format!("app-{flavor}.ico"));
    println!("cargo::rerun-if-changed={}", icon.display());
    icon
}

/// Compiles and links a resource script holding `icon` as the executable's
/// first icon, which Explorer, the taskbar, and shortcuts show. A missing
/// resource compiler fails the build: a release must not ship without it.
#[cfg(windows)]
fn embed_icon(icon: &Path) {
    assert!(
        icon.is_file(),
        "{} is missing: run `cargo xtask icons`",
        icon.display()
    );
    // rc.exe strings escape backslashes.
    let path = icon.display().to_string().replace('\\', r"\\");
    let script = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("chartreuse.rc");
    std::fs::write(&script, format!("1 ICON \"{path}\"\n"))
        .unwrap_or_else(|error| panic!("writing {}: {error}", script.display()));
    embed_resource::compile(&script, embed_resource::NONE)
        .manifest_required()
        .unwrap_or_else(|error| panic!("embedding the app icon: {error}"));
}

/// `embed-resource` is a build dependency of Windows hosts only (build
/// dependencies follow the host): a Windows build from another OS, such as
/// cross-checking with clippy, gets no icon.
#[cfg(not(windows))]
fn embed_icon(icon: &Path) {
    println!(
        "cargo::warning=not embedding {}: the Windows resource compiler only runs on \
         Windows",
        icon.display()
    );
}
