//! Wayland: window enumeration, which Wayland does not offer. Clients see only
//! their own windows (the protocol has no global window list, and the
//! compositor-specific ones, such as wlr-foreign-toplevel, carry no
//! geometry), so window selection over a frozen screenshot is impossible.
//! Window selection instead hands over to the Screenshot portal's own window
//! picker (see [`WaylandCapture`](super::capture::WaylandCapture)).

use chartreuse_core::window::WindowInfo;
use chartreuse_core::{Error, Result};
use futures::future::{self, BoxFuture, FutureExt};

use crate::window_list::WindowList;

/// The Wayland [`WindowList`] backend.
#[derive(Debug, Default)]
pub struct WaylandWindowList;

impl WaylandWindowList {
    pub fn new() -> Self {
        Self
    }
}

impl WindowList for WaylandWindowList {
    fn windows(&self) -> BoxFuture<'static, Result<Vec<WindowInfo>>> {
        future::ready(Err(Error::Unsupported(
            "listing other applications' windows on Wayland",
        )))
        .boxed()
    }
}
