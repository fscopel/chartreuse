//! Pixel operations, image encoding, and decoding, on
//! [`chartreuse_core::image::Image`] buffers.
//!
//! Owned by track 1E.
//!
//! - [`region`]: crop and copy rectangles in physical pixels.
//! - [`codec`]: encode and decode PNG, JPEG, WebP, GIF, BMP, and TIFF files.
//! - [`composite`]: combine per-display captures into one image on a
//!   [`PixelGrid`](chartreuse_core::display::PixelGrid) of the desktop or of a
//!   selection.
//! - [`kernels`]: pixelate and blur a region, for redaction.
//! - [`scale`]: resize an image by resampling it.

pub mod codec;
pub mod composite;
pub mod kernels;
pub mod region;
pub mod scale;

pub use codec::{decode, decode_file, encode, Format};
pub use composite::{composite_at, Composite};
pub use kernels::{blur, pixelate};
pub use region::{copy_region, crop};
pub use scale::resize;
