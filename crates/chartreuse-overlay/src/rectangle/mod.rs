//! Rectangle selection: the frozen image, a translucent dim, and a live selection
//! box, across every display. Owned by track 2D.
//!
//! # Design
//!
//! There is one overlay window per display (see [`crate::setup`]), but only one
//! selection: a [`Selection`] in **global logical desktop coordinates**, owned by
//! the app. Each window draws a [`RectangleOverlay`] canvas for its display,
//! which renders that display's slice of the shared selection and reports
//! pointer, Escape and Space input, already converted to global coordinates,
//! back to the app. A drag may start on one display and end on another; the
//! selection simply spans both. Space is the app's to act on: the app switches
//! to window selection with it.
//!
//! # Using it from the app
//!
//! ```ignore
//! // Opening: after capturing every display, with `layout` from the same displays.
//! // `frozen_image` is `crate::shared::frozen_image`.
//! state.selection = Selection::new(layout.clone());
//! state.images = captures.iter().map(|c| frozen_image(c.image.clone())).collect();
//!
//! // In each overlay window's `view`, for its display and capture:
//! RectangleOverlay::new(&state.selection, &display, &image, accent, Message::Selection)
//!     .view()
//!
//! // In `update`:
//! Message::Selection(input) => match state.selection.apply(input) {
//!     Some(Outcome::Commit(rect)) => {
//!         let grid = output_grid(state.selection.layout(), &rect)
//!             .expect("committed selections have an output grid");
//!         let image = chartreuse_imaging::composite_at(
//!             captures.iter().map(|c| (&c.display, &c.image)),
//!             grid,
//!         )?.image;
//!         // close the overlays, hand `image` off
//!     }
//!     Some(Outcome::Cancel) => { /* close the overlays */ }
//!     None => {}
//! }
//! ```
//!
//! # Mixed scale factors
//!
//! A selection's output image is cropped at the **largest** scale factor among
//! the displays it covers ([`output_grid`], built on
//! `DisplayLayout::capture_grid`): a selection spanning a 1× and a 2× display is
//! output at 2×, the 1× part upscaled. The size label shows that output size.
//!
//! Try it without real capture: `cargo run -p chartreuse-overlay --example
//! rectangle_harness`.

mod canvas;
mod selection;

pub use canvas::RectangleOverlay;
pub use selection::{output_grid, Input, Outcome, Phase, Selection, DRAG_THRESHOLD};
