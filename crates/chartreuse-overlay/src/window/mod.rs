//! Window selection: the frozen image, dimmed everywhere except the window under
//! the pointer, across every display. Owned by track 2H.
//!
//! # Design
//!
//! As in [`crate::rectangle`], there is one overlay window per display (see
//! [`crate::setup`]) but only one selection: a [`WindowSelection`] in **global
//! logical desktop coordinates**, owned by the app, built from the display
//! layout and the window list taken with the captures. Each window draws a
//! [`WindowOverlay`] canvas for its display, which leaves that display's part
//! of the hovered window undimmed and reports pointer, Escape and Space input,
//! already converted to global coordinates, back to the app. The hovered window
//! is the frontmost one under the pointer
//! ([`topmost_at`](chartreuse_core::window::topmost_at)), so the highlight
//! follows the pointer between windows and between displays.
//!
//! A click on a window commits its [`WindowId`](chartreuse_core::window::WindowId);
//! a click on no window is ignored; Escape cancels. Space is the app's to act
//! on: the app switches back to rectangle selection with it.
//!
//! # Using it from the app
//!
//! ```ignore
//! // Opening: after capturing every display and listing the windows, with
//! // `layout` from the same displays.
//! state.selection = WindowSelection::new(layout.clone(), windows);
//! state.images = captures.iter().map(|c| frozen_image(c.image.clone())).collect();
//!
//! // In each overlay window's `view`, for its display and capture:
//! WindowOverlay::new(&state.selection, &display, &image, accent, Message::Selection)
//!     .view()
//!
//! // In `update`:
//! Message::Selection(input) => match state.selection.apply(input) {
//!     Some(Outcome::Commit(id)) => {
//!         // close the overlays, then capture the window:
//!         // Task::perform(platform.capture.capture_window(id), ...)
//!     }
//!     Some(Outcome::Cancel) => { /* close the overlays */ }
//!     None => {}
//! }
//! ```
//!
//! `frozen_image` is [`crate::shared::frozen_image`].
//!
//! Try it without real capture: `cargo run -p chartreuse-overlay --example
//! window_harness`.

mod canvas;
mod selection;

pub use canvas::WindowOverlay;
pub use selection::{Input, Outcome, Phase, WindowSelection};
