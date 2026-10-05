//! Windows: converting captured pixels to [`Image`]s (the portable part of
//! `capture.rs`).
//!
//! Direct3D and GDI hand out 32-bit BGRA rows, possibly padded to a row pitch.
//! Windows.Graphics.Capture's alpha is premultiplied (DWM composes with
//! premultiplied alpha); GDI leaves the alpha byte undefined, and a screen is
//! opaque anyway.

use chartreuse_core::geometry::{PhysicalPoint, PhysicalRect, PhysicalSize};
use chartreuse_core::image::Image;
use chartreuse_core::{Error, Result};

/// How to read the fourth byte of each captured pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Alpha {
    /// Ignore it: every pixel is opaque.
    Opaque,
    /// Premultiplied alpha, converted to straight alpha.
    Premultiplied,
}

/// Converts `size.height` rows of BGRA pixels, `row_pitch` bytes apart, to a
/// straight-alpha RGBA [`Image`].
pub(super) fn image_from_bgra(
    bgra: &[u8],
    row_pitch: usize,
    size: PhysicalSize,
    alpha: Alpha,
) -> Result<Image> {
    let row_bytes = size.width as usize * 4;
    let needed = match size.height as usize {
        0 => Some(0),
        rows => row_pitch
            .checked_mul(rows - 1)
            .and_then(|n| n.checked_add(row_bytes)),
    };
    if row_pitch < row_bytes || needed.is_none_or(|needed| bgra.len() < needed) {
        return Err(Error::Platform(format!(
            "captured buffer of {} bytes (pitch {row_pitch}) is too small for {}×{} pixels",
            bgra.len(),
            size.width,
            size.height
        )));
    }
    let mut rgba = Vec::with_capacity(row_bytes * size.height as usize);
    for row in bgra.chunks(row_pitch.max(1)).take(size.height as usize) {
        for &[b, g, r, a] in row[..row_bytes].as_chunks::<4>().0 {
            rgba.extend_from_slice(&match alpha {
                Alpha::Opaque => [r, g, b, 255],
                Alpha::Premultiplied => unpremultiply([r, g, b, a]),
            });
        }
    }
    Image::new(size, rgba)
}

/// Converts one premultiplied RGBA pixel to straight alpha.
fn unpremultiply([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    match a {
        0 => [0, 0, 0, 0],
        255 => [r, g, b, 255],
        _ => {
            let channel = |c: u8| {
                let straight = (u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a);
                straight.min(255) as u8
            };
            [channel(r), channel(g), channel(b), a]
        }
    }
}

/// The part of a `PrintWindow` bitmap of the window rectangle `window` that shows
/// the visible frame `frame` (both in screen pixels), relative to the bitmap.
/// `None` if they do not overlap.
pub(super) fn frame_within(window: PhysicalRect, frame: PhysicalRect) -> Option<PhysicalRect> {
    let visible = window.intersection(&frame)?;
    Some(PhysicalRect {
        origin: PhysicalPoint::new(
            visible.min_x() - window.min_x(),
            visible.min_y() - window.min_y(),
        ),
        size: visible.size,
    })
}

/// The smallest rectangle holding every pixel of `image` that is not fully
/// transparent, or `None` if there is none.
pub(super) fn visible_bounds(image: &Image) -> Option<PhysicalRect> {
    let (width, height) = (image.width(), image.height());
    let visible = |x: u32, y: u32| image.pixel(x, y).is_some_and(|p| p.a != 0);
    let top = (0..height).find(|&y| (0..width).any(|x| visible(x, y)))?;
    let bottom = (top..height)
        .rev()
        .find(|&y| (0..width).any(|x| visible(x, y)))?;
    let left = (0..width).find(|&x| (top..=bottom).any(|y| visible(x, y)))?;
    let right = (left..width)
        .rev()
        .find(|&x| (top..=bottom).any(|y| visible(x, y)))?;
    Some(PhysicalRect::new(
        left as i32,
        top as i32,
        right - left + 1,
        bottom - top + 1,
    ))
}

#[cfg(test)]
mod tests {
    use chartreuse_core::color::Rgba8;

    use super::*;

    #[test]
    fn rows_are_swizzled_and_their_padding_skipped() {
        // 2×2 pixels, rows padded to 12 bytes.
        let bgra = [
            1, 2, 3, 255, 4, 5, 6, 255, 0xee, 0xee, 0xee, 0xee, //
            7, 8, 9, 255, 10, 11, 12, 255, 0xee, 0xee, 0xee, 0xee,
        ];
        let image =
            image_from_bgra(&bgra, 12, PhysicalSize::new(2, 2), Alpha::Premultiplied).unwrap();
        assert_eq!(
            image.pixels(),
            [3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]
        );
    }

    #[test]
    fn opaque_captures_ignore_the_alpha_byte() {
        let image =
            image_from_bgra(&[10, 20, 30, 0], 4, PhysicalSize::new(1, 1), Alpha::Opaque).unwrap();
        assert_eq!(image.pixel(0, 0), Some(Rgba8::new(30, 20, 10, 255)));
    }

    #[test]
    fn premultiplied_alpha_is_made_straight() {
        let convert = |bgra: [u8; 4]| {
            image_from_bgra(&bgra, 4, PhysicalSize::new(1, 1), Alpha::Premultiplied)
                .unwrap()
                .pixel(0, 0)
                .unwrap()
        };
        // Half-transparent white, and half-transparent 50% red.
        assert_eq!(
            convert([128, 128, 128, 128]),
            Rgba8::new(255, 255, 255, 128)
        );
        assert_eq!(convert([0, 0, 64, 128]), Rgba8::new(128, 0, 0, 128));
        // Fully transparent pixels carry no color.
        assert_eq!(convert([5, 5, 5, 0]), Rgba8::new(0, 0, 0, 0));
        // Out-of-range premultiplied values saturate.
        assert_eq!(convert([200, 0, 0, 100]), Rgba8::new(0, 0, 255, 100));
    }

    #[test]
    fn short_buffers_are_rejected() {
        let size = PhysicalSize::new(2, 2);
        // The last row needs no padding, but a pitch below the row size is invalid.
        assert!(image_from_bgra(&[0; 20], 12, size, Alpha::Opaque).is_ok());
        assert!(image_from_bgra(&[0; 19], 12, size, Alpha::Opaque).is_err());
        assert!(image_from_bgra(&[0; 64], 4, size, Alpha::Opaque).is_err());
    }

    #[test]
    fn the_frame_is_located_inside_the_window_bitmap() {
        // Windows 10 windows extend 7 px past their frame on the left, right and
        // bottom (invisible resize borders).
        let window = PhysicalRect::new(93, 50, 814, 607);
        let frame = PhysicalRect::new(100, 50, 800, 600);
        assert_eq!(
            frame_within(window, frame),
            Some(PhysicalRect::new(7, 0, 800, 600))
        );
        assert_eq!(
            frame_within(window, PhysicalRect::new(2000, 0, 10, 10)),
            None
        );
    }

    #[test]
    fn visible_bounds_trim_transparent_edges() {
        let image = Image::from_fn(PhysicalSize::new(6, 5), |x, y| {
            if (1..=3).contains(&x) && (2..=3).contains(&y) {
                Rgba8::new(1, 2, 3, if x == 1 { 1 } else { 255 })
            } else {
                Rgba8::new(9, 9, 9, 0)
            }
        });
        assert_eq!(visible_bounds(&image), Some(PhysicalRect::new(1, 2, 3, 2)));
        let blank = Image::filled(PhysicalSize::new(3, 3), Rgba8::new(0, 0, 0, 0));
        assert_eq!(visible_bounds(&blank), None);
    }
}
