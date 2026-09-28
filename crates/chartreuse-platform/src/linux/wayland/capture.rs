//! Wayland: capture through the Screenshot portal. Wayland lets no client
//! read another's pixels, so the desktop's portal backend takes the picture.
//!
//! - Displays: a non-interactive screenshot of the whole desktop, split into
//!   one capture per display (see [`crate::linux::logic::screenshot`]). The
//!   portal asks the user for permission the first time (GNOME remembers the
//!   answer per app); a refusal is [`Error::PermissionDenied`].
//! - Interactively ([`Capture::capture_interactively`]): the portal's own
//!   dialog lets the user pick a window (GNOME, KDE), and on GNOME an area
//!   or a display as well; wlroots portals capture the whole desktop instead.
//!   The app uses it for window captures, since Wayland cannot list windows
//!   (see [`WaylandWindowList`](super::window_list::WaylandWindowList)), and
//!   for rectangle captures on several outputs, which the overlays cannot
//!   cover (see [`WaylandOverlayStyle`](super::overlay_style::WaylandOverlayStyle)).
//!   A listed window ([`Capture::capture_window`]) is never asked for.
//!
//! The portal saves each screenshot as a PNG file (GNOME in the Pictures
//! folder); Chartreuse reads it and deletes it, since it only asked for the
//! pixels.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use ashpd::desktop::screenshot::Screenshot;
use chartreuse_core::display::DisplayLayout;
use chartreuse_core::image::Image;
use chartreuse_core::permission::Permission;
use chartreuse_core::window::WindowId;
use chartreuse_core::{Error, Result};
use futures::future::{self, BoxFuture, FutureExt};

use super::displays;
use crate::capture::{Capture, DisplayCapture};
use crate::linux::logic::file_chooser::file_uri_path;
use crate::linux::logic::screenshot;
use crate::linux::{blocking, portal};

/// The Wayland [`Capture`] backend.
#[derive(Debug, Default)]
pub struct WaylandCapture;

impl WaylandCapture {
    pub fn new() -> Self {
        Self
    }
}

impl Capture for WaylandCapture {
    fn capture_displays(&self) -> BoxFuture<'static, Result<Vec<DisplayCapture>>> {
        async {
            let displays = blocking::run("display enumeration", displays::query).await?;
            let layout = DisplayLayout::new(displays)?;
            let Some(shot) = take(false).await? else {
                return Err(Error::PermissionDenied(Permission::ScreenRecording));
            };
            let images = screenshot::split(&layout, &shot)?;
            Ok(layout
                .displays()
                .iter()
                .cloned()
                .zip(images)
                .map(|(display, image)| DisplayCapture { display, image })
                .collect())
        }
        .boxed()
    }

    fn capture_window(&self, _window: WindowId) -> BoxFuture<'static, Result<Image>> {
        future::ready(Err(Error::Unsupported(
            "capturing a listed window on Wayland",
        )))
        .boxed()
    }

    fn capture_interactively(&self) -> Option<BoxFuture<'static, Result<Option<Image>>>> {
        Some(take(true).boxed())
    }
}

/// Takes a screenshot, letting the user choose what it shows if
/// `interactive`. `None` if the user cancelled, which a non-interactive
/// request means as a refusal to let Chartreuse capture the screen.
async fn take(interactive: bool) -> Result<Option<Image>> {
    portal::register_app().await;
    let response = Screenshot::request()
        .interactive(interactive)
        .modal(true)
        .send()
        .await
        .and_then(|request| request.response());
    let shot = match response {
        Ok(shot) => shot,
        Err(error) if portal::cancelled(&error) => return Ok(None),
        Err(error) => return Err(portal::error("the screenshot", &error)),
    };
    let uri = shot.uri().as_str();
    let path = file_uri_path(uri)
        .map(|bytes| PathBuf::from(OsString::from_vec(bytes)))
        .ok_or_else(|| {
            Error::Platform(format!(
                "the screenshot portal saved to {uri}, not a local file"
            ))
        })?;
    blocking::run("reading the screenshot", move || {
        let image = chartreuse_imaging::decode_file(&path);
        if let Err(error) = fs::remove_file(&path) {
            tracing::debug!(
                "could not delete the screenshot {}: {error}",
                path.display()
            );
        }
        image
    })
    .await
    .map(Some)
}
