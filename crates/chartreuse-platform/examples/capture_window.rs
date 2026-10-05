//! Lists the on-screen windows with the current platform backend, then captures
//! the frontmost one that is not Chartreuse's to a PNG, for checking real window
//! captures by eye.
//!
//! ```sh
//! cargo run -p chartreuse-platform --example capture_window [OUTPUT_DIR]
//! ```
//!
//! `OUTPUT_DIR` defaults to a fresh directory under the system temp dir. On macOS
//! the process that runs this (the terminal app) needs the Screen Recording
//! permission; without it the example reports `PermissionDenied` and exits with
//! status 2.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use chartreuse_core::image::Image;
use chartreuse_core::Error;
use chartreuse_imaging::codec::{self, Format};
use futures::executor::block_on;

fn main() -> ExitCode {
    let dir = std::env::args_os().nth(1).map_or_else(
        || {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs());
            std::env::temp_dir().join(format!("chartreuse-window-{stamp}"))
        },
        PathBuf::from,
    );

    // Called on the main thread, as the platform traits require; the futures are
    // then driven here while ScreenCaptureKit answers on its own queues.
    let platform = chartreuse_platform::current();
    let windows = match block_on(platform.window_list.windows()) {
        Ok(windows) => windows,
        Err(error) => return fail("listing windows", &error),
    };
    for window in &windows {
        let bounds = window.bounds;
        println!(
            "#{:<2} {:>6}  {} (pid {}) {:?}  at ({}, {}) {}×{}",
            window.z_order,
            window.id.0,
            window.owner.name,
            window
                .owner
                .pid
                .map_or_else(|| "?".to_owned(), |pid| pid.to_string()),
            window.title.as_deref().unwrap_or("(untitled)"),
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        );
    }

    // The list already leaves out this process's windows; a running Chartreuse
    // app is another process, so skip it by name too.
    let Some(target) = windows
        .iter()
        .find(|window| !window.owner.name.starts_with("Chartreuse"))
    else {
        eprintln!("no window to capture");
        return ExitCode::FAILURE;
    };
    let image = match block_on(platform.capture.capture_window(target.id)) {
        Ok(image) => image,
        Err(error) => return fail("capturing the window", &error),
    };

    if let Err(error) = fs::create_dir_all(&dir) {
        eprintln!("could not create {}: {error}", dir.display());
        return ExitCode::FAILURE;
    }
    let path = dir.join(format!("window-{}.png", target.id.0));
    let png = match codec::encode(&image, Format::Png) {
        Ok(png) => png,
        Err(error) => return fail("encoding the capture", &error),
    };
    if let Err(error) = fs::write(&path, png) {
        eprintln!("could not write {}: {error}", path.display());
        return ExitCode::FAILURE;
    }
    let size = image.size();
    println!(
        "captured {} ({}): {}×{} px for a {}×{} pt frame → {}",
        target.id.0,
        target.owner.name,
        size.width,
        size.height,
        target.bounds.size.width,
        target.bounds.size.height,
        path.display()
    );
    println!("{}", alpha_summary(&image));
    ExitCode::SUCCESS
}

/// Reports an error; `PermissionDenied` exits with status 2.
fn fail(action: &str, error: &Error) -> ExitCode {
    eprintln!("{action} failed: {error}");
    if matches!(error, Error::PermissionDenied(_)) {
        eprintln!("grant Screen Recording to the app running this example and retry");
        return ExitCode::from(2);
    }
    ExitCode::FAILURE
}

/// The alpha at the corners and edge midpoints, and how many pixels are opaque,
/// translucent (shadow, rounded-corner edges) or fully transparent.
fn alpha_summary(image: &Image) -> String {
    let size = image.size();
    let (w, h) = (size.width - 1, size.height - 1);
    let alpha = |x, y| image.pixel(x, y).map_or(0, |pixel| pixel.a);
    let (mut opaque, mut translucent, mut transparent) = (0_usize, 0_usize, 0_usize);
    for pixel in image.pixels().as_chunks::<4>().0 {
        match pixel[3] {
            0 => transparent += 1,
            255 => opaque += 1,
            _ => translucent += 1,
        }
    }
    format!(
        "alpha: corners {} {} {} {}, edge midpoints top {} bottom {} left {} right {}; \
         pixels opaque {opaque}, translucent {translucent}, transparent {transparent}",
        alpha(0, 0),
        alpha(w, 0),
        alpha(0, h),
        alpha(w, h),
        alpha(w / 2, 0),
        alpha(w / 2, h),
        alpha(0, h / 2),
        alpha(w, h / 2),
    )
}
