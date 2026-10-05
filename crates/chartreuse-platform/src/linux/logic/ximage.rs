//! Converting X11 `ZPixmap` image data (what `GetImage` and `XShmGetImage`
//! return) to RGBA8.
//!
//! A `ZPixmap` stores each pixel as one `bits_per_pixel`-bit unit in the
//! server's image byte order, rows padded to the pixmap format's scanline pad.
//! Which bits hold which channel comes from the drawable's visual: the red,
//! green, and blue masks, plus, for 32-bit ARGB visuals (windows with an alpha
//! channel), the remaining 8 bits as premultiplied alpha.

use chartreuse_core::geometry::PhysicalSize;
use chartreuse_core::image::Image;
use chartreuse_core::{Error, Result};

/// The memory layout of `ZPixmap` data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    /// 16, 24, or 32.
    pub bits_per_pixel: u8,
    /// Bytes per row, including padding.
    pub stride: usize,
    /// The server's image byte order is most significant byte first.
    pub big_endian: bool,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    /// The bits holding premultiplied alpha, or 0 for opaque pixels.
    pub alpha_mask: u32,
}

impl PixelFormat {
    /// The layout of a `width`-pixel-wide image in a pixmap format with
    /// `bits_per_pixel` and `scanline_pad` (both in bits), opaque, with the
    /// visual's channel masks.
    #[must_use]
    pub fn new(
        width: u32,
        bits_per_pixel: u8,
        scanline_pad: u8,
        big_endian: bool,
        [red_mask, green_mask, blue_mask]: [u32; 3],
    ) -> Self {
        let pad = usize::from(scanline_pad.max(8));
        let bits = width as usize * usize::from(bits_per_pixel);
        Self {
            bits_per_pixel,
            stride: bits.div_ceil(pad) * pad / 8,
            big_endian,
            red_mask,
            green_mask,
            blue_mask,
            alpha_mask: 0,
        }
    }

    /// The same layout, with the bits no color channel uses read as
    /// premultiplied alpha (a depth-32 ARGB visual).
    #[must_use]
    pub fn with_alpha(self) -> Self {
        let color = self.red_mask | self.green_mask | self.blue_mask;
        let all = if self.bits_per_pixel >= 32 {
            u32::MAX
        } else {
            (1 << self.bits_per_pixel) - 1
        };
        Self {
            alpha_mask: all & !color,
            ..self
        }
    }

    /// The fewest bytes that hold a `size` image: every row but the last is
    /// padded to the stride.
    #[must_use]
    pub fn len(&self, size: PhysicalSize) -> usize {
        if size.height == 0 {
            return 0;
        }
        let last_row = size.width as usize * usize::from(self.bits_per_pixel).div_ceil(8);
        (size.height as usize - 1) * self.stride + last_row
    }
}

/// Converts `size` pixels of `ZPixmap` `data` laid out as `format` to an RGBA8
/// image with straight alpha.
///
/// # Errors
///
/// [`Error::InvalidImage`] for an unsupported pixel size, a format without
/// color masks, or data shorter than the image.
pub fn to_image(size: PhysicalSize, format: &PixelFormat, data: &[u8]) -> Result<Image> {
    let bytes_per_pixel = match format.bits_per_pixel {
        16 => 2,
        24 => 3,
        32 => 4,
        other => {
            return Err(Error::InvalidImage(format!(
                "{other}-bit X11 pixels are not supported"
            )));
        }
    };
    if format.red_mask == 0 || format.green_mask == 0 || format.blue_mask == 0 {
        return Err(Error::InvalidImage(
            "the X11 visual has no color channel masks".into(),
        ));
    }
    let needed = format.len(size);
    if data.len() < needed {
        return Err(Error::InvalidImage(format!(
            "{}×{} X11 image needs {needed} bytes, got {}",
            size.width,
            size.height,
            data.len()
        )));
    }

    let width = size.width as usize;
    let mut pixels = Vec::with_capacity(width * size.height as usize * 4);
    let common_bgrx = bytes_per_pixel == 4
        && !format.big_endian
        && format.red_mask == 0x00ff_0000
        && format.green_mask == 0x0000_ff00
        && format.blue_mask == 0x0000_00ff
        && matches!(format.alpha_mask, 0 | 0xff00_0000);
    for row in data.chunks(format.stride).take(size.height as usize) {
        let row = &row[..width * bytes_per_pixel];
        if common_bgrx {
            // The layout of nearly every X server: B, G, R, then X or alpha.
            for &[b, g, r, x] in row.as_chunks::<4>().0 {
                let alpha = if format.alpha_mask == 0 { u8::MAX } else { x };
                pixels.extend_from_slice(&straight([r, g, b], alpha));
            }
        } else {
            for unit in row.chunks_exact(bytes_per_pixel) {
                let value = unit_value(unit, format.big_endian);
                let alpha = if format.alpha_mask == 0 {
                    u8::MAX
                } else {
                    channel(value, format.alpha_mask)
                };
                let rgb = [
                    channel(value, format.red_mask),
                    channel(value, format.green_mask),
                    channel(value, format.blue_mask),
                ];
                pixels.extend_from_slice(&straight(rgb, alpha));
            }
        }
    }
    Image::new(size, pixels)
}

/// One pixel unit read in the image byte order.
fn unit_value(unit: &[u8], big_endian: bool) -> u32 {
    let fold = |value: u32, byte: &u8| (value << 8) | u32::from(*byte);
    if big_endian {
        unit.iter().fold(0, fold)
    } else {
        unit.iter().rev().fold(0, fold)
    }
}

/// The channel under `mask` (contiguous bits), scaled to 8 bits.
fn channel(value: u32, mask: u32) -> u8 {
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones();
    let raw = u64::from((value & mask) >> shift);
    if bits >= 8 {
        (raw >> (bits - 8)) as u8
    } else {
        let max = (1u64 << bits) - 1;
        ((raw * 255 + max / 2) / max) as u8
    }
}

/// Undoes alpha premultiplication.
fn straight([r, g, b]: [u8; 3], alpha: u8) -> [u8; 4] {
    match alpha {
        u8::MAX => [r, g, b, alpha],
        0 => [0, 0, 0, 0],
        _ => {
            let a = u16::from(alpha);
            let unpremultiply =
                |c: u8| ((u16::from(c) * 255 + a / 2) / a).min(u16::from(u8::MAX)) as u8;
            [unpremultiply(r), unpremultiply(g), unpremultiply(b), alpha]
        }
    }
}

#[cfg(test)]
mod tests {
    use chartreuse_core::color::Rgba8;

    use super::*;

    const RGB_888: [u32; 3] = [0x00ff_0000, 0x0000_ff00, 0x0000_00ff];

    fn pixels(image: &Image) -> Vec<Rgba8> {
        (0..image.height())
            .flat_map(|y| (0..image.width()).map(move |x| (x, y)))
            .map(|(x, y)| image.pixel(x, y).unwrap())
            .collect()
    }

    #[test]
    fn stride_rounds_rows_up_to_the_scanline_pad() {
        assert_eq!(PixelFormat::new(3, 32, 32, false, RGB_888).stride, 12);
        assert_eq!(PixelFormat::new(3, 16, 32, false, RGB_888).stride, 8);
        assert_eq!(PixelFormat::new(3, 24, 32, false, RGB_888).stride, 12);
        assert_eq!(PixelFormat::new(5, 24, 8, false, RGB_888).stride, 15);
    }

    #[test]
    fn bgrx_rows_become_opaque_rgba_and_padding_is_skipped() {
        // 1×2 image, rows padded to 8 bytes; the X byte is garbage.
        let format = PixelFormat {
            stride: 8,
            ..PixelFormat::new(1, 32, 32, false, RGB_888)
        };
        let data = [3, 2, 1, 0x55, 9, 9, 9, 9, 30, 20, 10, 0xaa];
        let image = to_image(PhysicalSize::new(1, 2), &format, &data).unwrap();
        assert_eq!(
            pixels(&image),
            [Rgba8::new(1, 2, 3, 255), Rgba8::new(10, 20, 30, 255)]
        );
    }

    #[test]
    fn big_endian_servers_store_the_channels_the_other_way_round() {
        let format = PixelFormat::new(1, 32, 32, true, RGB_888);
        let image = to_image(PhysicalSize::new(1, 1), &format, &[0, 1, 2, 3]).unwrap();
        assert_eq!(pixels(&image), [Rgba8::new(1, 2, 3, 255)]);
    }

    #[test]
    fn packed_24_bit_pixels_are_read_three_bytes_at_a_time() {
        let format = PixelFormat::new(2, 24, 8, false, RGB_888);
        let data = [3, 2, 1, 6, 5, 4];
        let image = to_image(PhysicalSize::new(2, 1), &format, &data).unwrap();
        assert_eq!(
            pixels(&image),
            [Rgba8::new(1, 2, 3, 255), Rgba8::new(4, 5, 6, 255)]
        );
    }

    #[test]
    fn rgb565_channels_are_scaled_to_the_full_8_bit_range() {
        let format = PixelFormat::new(4, 16, 32, false, [0xf800, 0x07e0, 0x001f]);
        let units: [u16; 4] = [0xf800, 0x07e0, 0x001f, 16 << 11];
        let data: Vec<u8> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
        let image = to_image(PhysicalSize::new(4, 1), &format, &data).unwrap();
        assert_eq!(
            pixels(&image),
            [
                Rgba8::new(255, 0, 0, 255),
                Rgba8::new(0, 255, 0, 255),
                Rgba8::new(0, 0, 255, 255),
                // 16 of 31 steps.
                Rgba8::new(132, 0, 0, 255),
            ]
        );
    }

    #[test]
    fn argb_visuals_carry_premultiplied_alpha() {
        let format = PixelFormat::new(3, 32, 32, false, RGB_888).with_alpha();
        assert_eq!(format.alpha_mask, 0xff00_0000);
        // Half-transparent red at half intensity, fully transparent, opaque.
        let data = [0, 0, 64, 128, 7, 7, 7, 0, 3, 2, 1, 255];
        let image = to_image(PhysicalSize::new(3, 1), &format, &data).unwrap();
        assert_eq!(
            pixels(&image),
            [
                Rgba8::new(128, 0, 0, 128),
                Rgba8::new(0, 0, 0, 0),
                Rgba8::new(1, 2, 3, 255),
            ]
        );
    }

    #[test]
    fn unusual_masks_take_the_generic_path() {
        // An RGBX (red in the low byte) little-endian layout.
        let format = PixelFormat::new(1, 32, 32, false, [0xff, 0xff00, 0xff_0000]);
        let image = to_image(PhysicalSize::new(1, 1), &format, &[1, 2, 3, 0]).unwrap();
        assert_eq!(pixels(&image), [Rgba8::new(1, 2, 3, 255)]);
    }

    #[test]
    fn short_data_and_unsupported_formats_are_rejected() {
        let format = PixelFormat::new(2, 32, 32, false, RGB_888);
        // The last row needs no padding, but every pixel must be there.
        assert_eq!(format.len(PhysicalSize::new(2, 2)), 16);
        assert!(matches!(
            to_image(PhysicalSize::new(2, 2), &format, &[0; 15]),
            Err(Error::InvalidImage(_))
        ));
        let eight_bit = PixelFormat::new(2, 8, 32, false, RGB_888);
        assert!(matches!(
            to_image(PhysicalSize::new(2, 1), &eight_bit, &[0; 4]),
            Err(Error::InvalidImage(_))
        ));
        let no_masks = PixelFormat::new(2, 32, 32, false, [0; 3]);
        assert!(matches!(
            to_image(PhysicalSize::new(2, 1), &no_masks, &[0; 8]),
            Err(Error::InvalidImage(_))
        ));
    }
}
