//! macOS: `CGImage` → [`Image`] conversion, shared by the backends that receive
//! Core Graphics images.
//!
//! [`image_from_cg`] draws any image at a chosen size (screen capture uses it).
//! [`cg_image_to_image`] keeps the image's own pixel size and reads 8-bit packed
//! sRGB bitmaps directly, drawing only the rest (the clipboard uses it).

use chartreuse_core::geometry::PhysicalSize;
use chartreuse_core::image::Image;
use chartreuse_core::{Error, Result};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    kCGColorSpaceSRGB, CGBitmapContextCreate, CGBitmapInfo, CGColorSpace, CGContext,
    CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, CGImagePixelFormatInfo,
};

/// Converts a `CGImage` to straight-alpha sRGB RGBA8 at its pixel size.
///
/// 8-bit packed sRGB images are converted directly (and losslessly when their
/// alpha is straight). Anything else — other color spaces, 16-bit or float
/// components, grayscale, indexed — is drawn into an 8-bit sRGB bitmap first
/// ([`image_from_cg`]), which color-matches it but, because Core Graphics
/// contexts are premultiplied, rounds the color of translucent pixels.
pub(super) fn cg_image_to_image(cg_image: &CGImage) -> Result<Image> {
    let size = cg_image_size(cg_image)?;
    if let Some(layout) = direct_layout(cg_image) {
        let data = CGImage::data_provider(Some(cg_image))
            .and_then(|provider| CGDataProvider::data(Some(&provider)))
            .ok_or_else(|| Error::Decode("the image has no pixel data".into()))?;
        // SAFETY: the CFData is a private immutable copy, alive for this borrow.
        let bytes = unsafe { data.as_bytes_unchecked() };
        return rgba8_from_packed(size, layout, bytes);
    }
    image_from_cg(cg_image, size)
}

fn cg_image_size(cg_image: &CGImage) -> Result<PhysicalSize> {
    let width = CGImage::width(Some(cg_image));
    let height = CGImage::height(Some(cg_image));
    let too_large = || Error::Decode(format!("a {width}×{height} image is too large"));
    Ok(PhysicalSize::new(
        u32::try_from(width).map_err(|_| too_large())?,
        u32::try_from(height).map_err(|_| too_large())?,
    ))
}

/// The layout of `cg_image`'s own buffer, if [`rgba8_from_packed`] can read it
/// without color conversion.
fn direct_layout(cg_image: &CGImage) -> Option<PackedLayout> {
    let is_srgb = CGImage::color_space(Some(cg_image))
        .and_then(|space| CGColorSpace::name(Some(&space)))
        // SAFETY: a Core Graphics constant.
        .is_some_and(|name| *name == *unsafe { kCGColorSpaceSRGB });
    let bitmap_info = CGImage::bitmap_info(Some(cg_image));
    let is_plain = CGImage::bits_per_component(Some(cg_image)) == 8
        && CGImage::bits_per_pixel(Some(cg_image)) == 32
        && CGImage::decode(Some(cg_image)).is_null()
        && bitmap_info.0 & CGBitmapInfo::ComponentInfoMask.0 == 0
        && CGImage::pixel_format_info(Some(cg_image)) == CGImagePixelFormatInfo::Packed;
    if !(is_srgb && is_plain) {
        return None;
    }
    let (order, alpha) = packed_format(
        CGImage::alpha_info(Some(cg_image)),
        CGImage::byte_order_info(Some(cg_image)),
    )?;
    Some(PackedLayout {
        order,
        alpha,
        bytes_per_row: CGImage::bytes_per_row(Some(cg_image)),
    })
}

/// Draws `image` into a `size` sRGB RGBA8 bitmap and returns it as an [`Image`],
/// scaling if the sizes differ.
///
/// CoreGraphics bitmap contexts only support premultiplied alpha for 8-bit RGBA,
/// so the pixels are un-premultiplied afterwards ([`unpremultiply`]).
pub(super) fn image_from_cg(image: &CGImage, size: PhysicalSize) -> Result<Image> {
    let width = size.width as usize;
    let height = size.height as usize;
    let too_large = || Error::InvalidImage(format!("{}×{} is too large", size.width, size.height));
    let stride = width.checked_mul(4).ok_or_else(too_large)?;
    let mut pixels = vec![0; stride.checked_mul(height).ok_or_else(too_large)?];
    // SAFETY: `kCGColorSpaceSRGB` is an immutable CoreGraphics constant.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .ok_or_else(|| Error::Platform("the sRGB color space is unavailable".into()))?;
    // Byte order 32-big with alpha last: memory holds r, g, b, a.
    let bitmap_info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    {
        // SAFETY: `pixels` holds `stride × height` bytes and outlives the context,
        // which is dropped at the end of this block.
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                stride,
                Some(&srgb),
                bitmap_info,
            )
        }
        .ok_or_else(|| {
            Error::Platform(format!(
                "could not create a {}×{} bitmap context",
                size.width, size.height
            ))
        })?;
        let bounds = CGRect::new(CGPoint::ZERO, CGSize::new(width as f64, height as f64));
        CGContext::draw_image(Some(&context), bounds, Some(image));
    }
    unpremultiply(&mut pixels);
    Image::new(size, pixels)
}

/// Where the red, green, blue and alpha bytes sit within each 4-byte pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelOrder {
    Rgba,
    Argb,
    Bgra,
    Abgr,
}

impl ChannelOrder {
    /// The byte offsets of red, green, blue and alpha (or padding).
    const fn offsets(self) -> [usize; 4] {
        match self {
            Self::Rgba => [0, 1, 2, 3],
            Self::Argb => [1, 2, 3, 0],
            Self::Bgra => [2, 1, 0, 3],
            Self::Abgr => [3, 2, 1, 0],
        }
    }
}

/// What the fourth byte of each pixel means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlphaMode {
    /// Straight (non-premultiplied) alpha.
    Straight,
    /// Alpha, with the color channels already multiplied by it.
    Premultiplied,
    /// Padding; the image is opaque.
    Ignored,
}

/// An 8-bit-per-channel, 4-bytes-per-pixel bitmap layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PackedLayout {
    order: ChannelOrder,
    alpha: AlphaMode,
    /// Bytes from the start of one row to the next (at least `width × 4`).
    bytes_per_row: usize,
}

/// Maps Core Graphics' alpha and byte-order flags for a 32-bit, 8-bit-per-channel
/// pixel to its memory layout, or `None` for layouts without four 8-bit channels
/// (no alpha byte, alpha-only masks, 16-bit byte orders).
fn packed_format(
    alpha_info: CGImageAlphaInfo,
    byte_order: CGImageByteOrderInfo,
) -> Option<(ChannelOrder, AlphaMode)> {
    let (alpha_last, alpha) = match alpha_info {
        CGImageAlphaInfo::Last => (true, AlphaMode::Straight),
        CGImageAlphaInfo::PremultipliedLast => (true, AlphaMode::Premultiplied),
        CGImageAlphaInfo::NoneSkipLast => (true, AlphaMode::Ignored),
        CGImageAlphaInfo::First => (false, AlphaMode::Straight),
        CGImageAlphaInfo::PremultipliedFirst => (false, AlphaMode::Premultiplied),
        CGImageAlphaInfo::NoneSkipFirst => (false, AlphaMode::Ignored),
        _ => return None,
    };
    // The alpha flags name the channel order of a big-endian 32-bit word, which is
    // the memory order; a little-endian word stores it reversed.
    let order = match (byte_order, alpha_last) {
        (CGImageByteOrderInfo::OrderDefault | CGImageByteOrderInfo::Order32Big, true) => {
            ChannelOrder::Rgba
        }
        (CGImageByteOrderInfo::OrderDefault | CGImageByteOrderInfo::Order32Big, false) => {
            ChannelOrder::Argb
        }
        (CGImageByteOrderInfo::Order32Little, true) => ChannelOrder::Abgr,
        (CGImageByteOrderInfo::Order32Little, false) => ChannelOrder::Bgra,
        _ => return None,
    };
    Some((order, alpha))
}

/// Converts a packed 8-bit bitmap of `size` pixels to straight-alpha RGBA8,
/// skipping any row padding and un-premultiplying ([`unpremultiply`]) if needed.
fn rgba8_from_packed(size: PhysicalSize, layout: PackedLayout, data: &[u8]) -> Result<Image> {
    let width = size.width as usize;
    let height = size.height as usize;
    let row_len = width
        .checked_mul(4)
        .ok_or_else(|| Error::Decode(format!("a {width}-pixel row is too long")))?;
    let needed = match height {
        0 => 0,
        _ => layout
            .bytes_per_row
            .checked_mul(height - 1)
            .and_then(|start| start.checked_add(row_len))
            .ok_or_else(|| Error::Decode("the bitmap is too large".into()))?,
    };
    if layout.bytes_per_row < row_len || data.len() < needed {
        return Err(Error::Decode(format!(
            "a {width}×{height} bitmap with {} bytes per row does not fit in {} bytes",
            layout.bytes_per_row,
            data.len()
        )));
    }
    let [r, g, b, a] = layout.order.offsets();
    let mut pixels = Vec::with_capacity(row_len * height);
    for row in 0..height {
        let start = row * layout.bytes_per_row;
        for pixel in data[start..start + row_len].as_chunks::<4>().0 {
            let mut rgba = [pixel[r], pixel[g], pixel[b], pixel[a]];
            match layout.alpha {
                AlphaMode::Straight => {}
                AlphaMode::Premultiplied => unpremultiply(&mut rgba),
                AlphaMode::Ignored => rgba[3] = u8::MAX,
            }
            pixels.extend_from_slice(&rgba);
        }
    }
    Image::new(size, pixels)
}

/// Converts RGBA8 pixels from premultiplied to straight alpha in place, rounding
/// to nearest. Fully transparent pixels become transparent black; color values
/// above their alpha (invalid premultiplied data) saturate at 255.
fn unpremultiply(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        match alpha {
            255 => {}
            0 => pixel[..3].fill(0),
            _ => {
                for channel in &mut pixel[..3] {
                    let straight = (u16::from(*channel) * 255 + alpha / 2) / alpha;
                    *channel = straight.min(255) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::CFRetained;

    use super::*;

    #[test]
    fn unpremultiply_restores_straight_alpha() {
        let mut pixels = [
            10, 20, 30, 255, // opaque: unchanged
            40, 50, 60, 0, // transparent: becomes transparent black
            64, 32, 0, 128, // half alpha: doubled (rounded)
            200, 100, 50, 100, // color above alpha: saturates
            1, 1, 1, 3, // rounds to nearest: 1 × 255 / 3 = 85
        ];
        unpremultiply(&mut pixels);
        assert_eq!(
            pixels,
            [
                10, 20, 30, 255, //
                0, 0, 0, 0, //
                128, 64, 0, 128, //
                255, 255, 128, 100, //
                85, 85, 85, 3,
            ]
        );
    }

    #[test]
    fn unpremultiply_round_trips_premultiplied_colors() {
        for alpha in 1..=255u16 {
            for straight in [0u16, 1, 77, 128, 254, 255] {
                let premultiplied = (straight * alpha + 127) / 255;
                let mut pixel = [premultiplied as u8, 0, 0, alpha as u8];
                unpremultiply(&mut pixel);
                let restored = u16::from(pixel[0]);
                // Premultiplying loses precision at low alpha; the round trip is
                // within one premultiplied step.
                let tolerance = 255 / alpha / 2 + 1;
                assert!(
                    restored.abs_diff(straight) <= tolerance,
                    "alpha {alpha}: {straight} → {premultiplied} → {restored}"
                );
            }
        }
    }

    /// A 2×2 sRGB `CGImage` from premultiplied RGBA bytes, top row first.
    fn cg_image(premultiplied: [u8; 16]) -> CFRetained<CGImage> {
        use objc2_core_graphics::{
            CGBitmapContextCreateImage, CGBitmapContextGetBytesPerRow, CGBitmapContextGetData,
        };

        // SAFETY: `kCGColorSpaceSRGB` is an immutable CoreGraphics constant.
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).unwrap();
        // SAFETY: a null buffer makes CoreGraphics allocate and own the memory, so
        // the image made from the context stays valid on its own.
        let context = unsafe {
            CGBitmapContextCreate(
                std::ptr::null_mut(),
                2,
                2,
                8,
                0,
                Some(&srgb),
                CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
            )
        }
        .unwrap();
        let data = CGBitmapContextGetData(Some(&context)).cast::<u8>();
        let stride = CGBitmapContextGetBytesPerRow(Some(&context));
        for (row, bytes) in premultiplied.as_chunks::<8>().0.iter().enumerate() {
            // SAFETY: the context's buffer has 2 rows of `stride` ≥ 8 bytes.
            unsafe {
                data.add(row * stride)
                    .copy_from_nonoverlapping(bytes.as_ptr(), 8)
            };
        }
        CGBitmapContextCreateImage(Some(&context)).unwrap()
    }

    #[test]
    fn cg_images_convert_to_straight_rgba_top_row_first() {
        let source = cg_image([
            255, 0, 0, 255, /**/ 0, 255, 0, 255, // top: red, green
            0, 0, 255, 255, /**/ 128, 128, 128,
            128, // bottom: blue, half-transparent white
        ]);
        let size = PhysicalSize::new(2, 2);
        let image = image_from_cg(&source, size).unwrap();
        assert_eq!(image.size(), size);
        assert_eq!(
            image.pixels(),
            [
                255, 0, 0, 255, /**/ 0, 255, 0, 255, //
                0, 0, 255, 255, /**/ 255, 255, 255, 128,
            ]
        );
    }

    fn layout(order: ChannelOrder, alpha: AlphaMode, bytes_per_row: usize) -> PackedLayout {
        PackedLayout {
            order,
            alpha,
            bytes_per_row,
        }
    }

    #[test]
    fn core_graphics_flags_map_to_memory_order() {
        use CGImageAlphaInfo as A;
        use CGImageByteOrderInfo as O;
        let cases = [
            (
                A::PremultipliedLast,
                O::OrderDefault,
                ChannelOrder::Rgba,
                AlphaMode::Premultiplied,
            ),
            (
                A::Last,
                O::Order32Big,
                ChannelOrder::Rgba,
                AlphaMode::Straight,
            ),
            (
                A::First,
                O::OrderDefault,
                ChannelOrder::Argb,
                AlphaMode::Straight,
            ),
            (
                A::Last,
                O::Order32Little,
                ChannelOrder::Abgr,
                AlphaMode::Straight,
            ),
            // The layout of ScreenCaptureKit frames and most window-server images.
            (
                A::PremultipliedFirst,
                O::Order32Little,
                ChannelOrder::Bgra,
                AlphaMode::Premultiplied,
            ),
            (
                A::NoneSkipFirst,
                O::Order32Little,
                ChannelOrder::Bgra,
                AlphaMode::Ignored,
            ),
            (
                A::NoneSkipLast,
                O::OrderDefault,
                ChannelOrder::Rgba,
                AlphaMode::Ignored,
            ),
        ];
        for (alpha_info, byte_order, order, alpha) in cases {
            assert_eq!(
                packed_format(alpha_info, byte_order),
                Some((order, alpha)),
                "{alpha_info:?} {byte_order:?}"
            );
        }
    }

    #[test]
    fn layouts_without_four_8_bit_channels_are_not_packed() {
        use CGImageAlphaInfo as A;
        use CGImageByteOrderInfo as O;
        assert_eq!(packed_format(A::None, O::OrderDefault), None);
        assert_eq!(packed_format(A::Only, O::OrderDefault), None);
        assert_eq!(packed_format(A::Last, O::Order16Little), None);
        assert_eq!(packed_format(A::First, O::Order16Big), None);
    }

    #[test]
    fn packed_conversion_reorders_channels_and_skips_row_padding() {
        // 2×2 BGRA with 4 bytes of padding at the end of each row.
        #[rustfmt::skip]
        let data = [
            3, 2, 1, 255,   6, 5, 4, 128,   0xee, 0xee, 0xee, 0xee,
            9, 8, 7, 0,     12, 11, 10, 1,  0xee, 0xee, 0xee, 0xee,
        ];
        let image = rgba8_from_packed(
            PhysicalSize::new(2, 2),
            layout(ChannelOrder::Bgra, AlphaMode::Straight, 12),
            &data,
        )
        .unwrap();
        assert_eq!(
            image.pixels(),
            [1, 2, 3, 255, 4, 5, 6, 128, 7, 8, 9, 0, 10, 11, 12, 1]
        );
    }

    #[test]
    fn packed_conversion_accepts_a_short_final_row() {
        // The last row needs only `width × 4` bytes, not a full stride.
        let data = [1, 2, 3, 4, 0, 0, 0, 0, 5, 6, 7, 8];
        let image = rgba8_from_packed(
            PhysicalSize::new(1, 2),
            layout(ChannelOrder::Rgba, AlphaMode::Straight, 8),
            &data,
        )
        .unwrap();
        assert_eq!(image.pixels(), [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn padding_bytes_become_opaque_alpha() {
        let data = [0, 10, 20, 30, 7, 40, 50, 60];
        let image = rgba8_from_packed(
            PhysicalSize::new(2, 1),
            layout(ChannelOrder::Argb, AlphaMode::Ignored, 8),
            &data,
        )
        .unwrap();
        assert_eq!(image.pixels(), [10, 20, 30, 255, 40, 50, 60, 255]);
    }

    #[test]
    fn premultiplied_layouts_are_unpremultiplied_after_reordering() {
        // ARGB: alpha is the first byte, so it must be found before dividing by it.
        let data = [128, 64, 32, 0, /**/ 255, 10, 20, 30];
        let image = rgba8_from_packed(
            PhysicalSize::new(2, 1),
            layout(ChannelOrder::Argb, AlphaMode::Premultiplied, 8),
            &data,
        )
        .unwrap();
        assert_eq!(image.pixels(), [128, 64, 0, 128, /**/ 10, 20, 30, 255]);
    }

    #[test]
    fn packed_conversion_rejects_buffers_that_are_too_short() {
        let straight =
            |bytes_per_row| layout(ChannelOrder::Rgba, AlphaMode::Straight, bytes_per_row);
        let size = PhysicalSize::new(2, 2);
        assert!(matches!(
            rgba8_from_packed(size, straight(8), &[0; 15]),
            Err(Error::Decode(_))
        ));
        // A stride shorter than a row is malformed even if the buffer is long enough.
        assert!(matches!(
            rgba8_from_packed(size, straight(4), &[0; 64]),
            Err(Error::Decode(_))
        ));
    }
}
