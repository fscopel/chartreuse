//! Encoding and decoding image files.
//!
//! Built on the [`image`] crate, compiled with only the formats listed in
//! [`Format`]. Chartreuse can save PNG, JPEG, and WebP ([`Format::ENCODABLE`], the
//! save-as choices), and open those plus GIF, BMP, and TIFF ([`Format::ALL`]).
//! Formats are recognized by their content, never trusted from a file extension.
//! Decoded images are converted to RGBA8, upright: an EXIF/TIFF orientation in
//! the file (as cameras write) is applied. For animated GIFs the result is the
//! first frame.

use std::fmt;
use std::io::Cursor;
use std::path::Path;

use chartreuse_core::geometry::PhysicalSize;
use chartreuse_core::image::Image;
use chartreuse_core::{Error, Result};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader,
};

/// An image file format Chartreuse can read, and possibly write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Png,
    Jpeg,
    WebP,
    Gif,
    Bmp,
    Tiff,
}

/// The JPEG quality [`encode`] uses, on the usual 1–100 scale.
pub const JPEG_QUALITY: u8 = 90;

impl Format {
    /// Every format [`decode`] accepts, most common first.
    pub const ALL: [Self; 6] = [
        Self::Png,
        Self::Jpeg,
        Self::WebP,
        Self::Gif,
        Self::Bmp,
        Self::Tiff,
    ];

    /// The formats [`encode`] can write.
    pub const ENCODABLE: [Self; 3] = [Self::Png, Self::Jpeg, Self::WebP];

    /// The format's usual name, such as `"PNG"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP",
            Self::Gif => "GIF",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
        }
    }

    /// The file extensions of the format, lowercase and without the dot. The first
    /// one is preferred for new files.
    #[must_use]
    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Png => &["png"],
            Self::Jpeg => &["jpg", "jpeg", "jpe"],
            Self::WebP => &["webp"],
            Self::Gif => &["gif"],
            Self::Bmp => &["bmp"],
            Self::Tiff => &["tiff", "tif"],
        }
    }

    /// The preferred file extension for new files, without the dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        self.extensions()[0]
    }

    /// The MIME type, such as `"image/png"`.
    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::WebP => "image/webp",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Tiff => "image/tiff",
        }
    }

    /// True if [`encode`] can write this format.
    #[must_use]
    pub const fn can_encode(self) -> bool {
        matches!(self, Self::Png | Self::Jpeg | Self::WebP)
    }

    /// The format with the file extension `extension` (without the dot, any case).
    #[must_use]
    pub fn from_extension(extension: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| {
            format
                .extensions()
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
    }

    /// The format that `path`'s extension names, for choosing how to save a file.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        Self::from_extension(path.extension()?.to_str()?)
    }

    /// The format of encoded image data, recognized from its signature bytes.
    #[must_use]
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        match image::guess_format(bytes).ok()? {
            ImageFormat::Png => Some(Self::Png),
            ImageFormat::Jpeg => Some(Self::Jpeg),
            ImageFormat::WebP => Some(Self::WebP),
            ImageFormat::Gif => Some(Self::Gif),
            ImageFormat::Bmp => Some(Self::Bmp),
            ImageFormat::Tiff => Some(Self::Tiff),
            _ => None,
        }
    }

    const fn to_image_format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::WebP => ImageFormat::WebP,
            Self::Gif => ImageFormat::Gif,
            Self::Bmp => ImageFormat::Bmp,
            Self::Tiff => ImageFormat::Tiff,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The names of every readable format, for error messages.
fn supported_list() -> String {
    let names: Vec<_> = Format::ALL.iter().map(|f| f.name()).collect();
    names.join(", ")
}

/// Decodes an image file's contents, recognizing the format from the data.
///
/// # Errors
///
/// [`Error::Decode`] if the data is not in one of [`Format::ALL`], or is corrupt,
/// or is too large to decode.
pub fn decode(bytes: &[u8]) -> Result<Image> {
    let format = Format::detect(bytes).ok_or_else(|| {
        Error::Decode(format!(
            "the data is not a supported image format ({})",
            supported_list()
        ))
    })?;
    decode_as(bytes, format)
}

/// Decodes data known to be in `format`, applying any orientation the file
/// records.
///
/// # Errors
///
/// [`Error::Decode`] if the data is not valid `format` data, or is too large to
/// decode.
pub fn decode_as(bytes: &[u8], format: Format) -> Result<Image> {
    let invalid = |e: image::ImageError| Error::Decode(format!("invalid {format} data: {e}"));
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format.to_image_format())
        .into_decoder()
        .map_err(invalid)?;
    let orientation = decoder.orientation().map_err(invalid)?;
    let mut dynamic = DynamicImage::from_decoder(decoder).map_err(invalid)?;
    dynamic.apply_orientation(orientation);
    let decoded = dynamic.into_rgba8();
    let size = PhysicalSize::new(decoded.width(), decoded.height());
    Image::new(size, decoded.into_raw())
}

/// Reads and decodes the image file at `path`. The format is recognized from the
/// file's contents, whatever its extension says.
///
/// # Errors
///
/// [`Error::Io`] if the file cannot be read, [`Error::Decode`] as for [`decode`].
pub fn decode_file(path: &Path) -> Result<Image> {
    let bytes =
        std::fs::read(path).map_err(|e| Error::io(format!("reading {}", path.display()), e))?;
    decode(&bytes)
}

/// Encodes `image` in `format`, with default settings: PNG and WebP are lossless,
/// JPEG uses [`JPEG_QUALITY`] (see [`encode_jpeg`]).
///
/// # Errors
///
/// [`Error::Encode`] if `format` cannot be written ([`Format::can_encode`]), the
/// image has no pixels, or the encoder fails.
pub fn encode(image: &Image, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Png => write_with(image, format, |out| {
            PngEncoder::new(out).write_image(
                image.pixels(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgba8,
            )
        }),
        Format::Jpeg => encode_jpeg(image, JPEG_QUALITY),
        Format::WebP => write_with(image, format, |out| {
            WebPEncoder::new_lossless(out).write_image(
                image.pixels(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgba8,
            )
        }),
        Format::Gif | Format::Bmp | Format::Tiff => Err(Error::Encode(format!(
            "Chartreuse cannot save {format} files"
        ))),
    }
}

/// Encodes `image` as JPEG at `quality` (1–100, clamped). JPEG has no
/// transparency, so translucent pixels are composited over white first.
///
/// # Errors
///
/// [`Error::Encode`] if the image has no pixels or the encoder fails.
pub fn encode_jpeg(image: &Image, quality: u8) -> Result<Vec<u8>> {
    let rgb: Vec<u8> = image
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|px| {
            let alpha = u32::from(px[3]);
            // Straight alpha over white: c·a + 255·(1 − a), in 0..=255 units.
            let over_white =
                |c: u8| ((u32::from(c) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
            [over_white(px[0]), over_white(px[1]), over_white(px[2])]
        })
        .collect();
    write_with(image, Format::Jpeg, |out| {
        JpegEncoder::new_with_quality(out, quality.clamp(1, 100)).write_image(
            &rgb,
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
    })
}

/// Runs an encoder into a new buffer, mapping its errors.
fn write_with(
    image: &Image,
    format: Format,
    encode: impl FnOnce(&mut Cursor<Vec<u8>>) -> image::ImageResult<()>,
) -> Result<Vec<u8>> {
    if image.size().is_empty() {
        return Err(Error::Encode(format!(
            "a {}×{} image has no pixels to save as {format}",
            image.width(),
            image.height()
        )));
    }
    let mut out = Cursor::new(Vec::new());
    encode(&mut out).map_err(|e| Error::Encode(format!("{format}: {e}")))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use chartreuse_core::color::Rgba8;
    use image::RgbaImage;

    use super::*;

    /// A small image with varied colors and every kind of alpha.
    fn sample() -> Image {
        Image::from_fn(PhysicalSize::new(7, 5), |x, y| {
            Rgba8::new(
                (x * 36) as u8,
                (y * 60) as u8,
                ((x + y) * 20) as u8,
                [255, 128, 1, 0, 200][y as usize],
            )
        })
    }

    /// `image` written by the `image` crate itself, for formats we only read.
    fn foreign(image: &Image, format: ImageFormat) -> Vec<u8> {
        let buffer =
            RgbaImage::from_raw(image.width(), image.height(), image.pixels().to_vec()).unwrap();
        let mut out = Cursor::new(Vec::new());
        let dynamic = DynamicImage::ImageRgba8(buffer);
        // BMP and GIF writers here want RGB(A) as given; TIFF too.
        dynamic.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn opaque_two_color() -> Image {
        Image::from_fn(PhysicalSize::new(4, 3), |x, y| {
            if (x + y) % 2 == 0 {
                Rgba8::rgb(255, 0, 0)
            } else {
                Rgba8::rgb(0, 0, 255)
            }
        })
    }

    #[test]
    fn png_round_trips_exactly() {
        let image = sample();
        let bytes = encode(&image, Format::Png).unwrap();
        assert_eq!(Format::detect(&bytes), Some(Format::Png));
        assert_eq!(decode(&bytes).unwrap(), image);
    }

    #[test]
    fn webp_round_trips_exactly() {
        let image = sample();
        let bytes = encode(&image, Format::WebP).unwrap();
        assert_eq!(Format::detect(&bytes), Some(Format::WebP));
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.size(), image.size());
        // Lossless, except that the color of fully transparent pixels is free.
        for (a, b) in image.pixels().chunks(4).zip(decoded.pixels().chunks(4)) {
            if a[3] == 0 {
                assert_eq!(b[3], 0);
            } else {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn jpeg_round_trips_closely_and_flattens_alpha_onto_white() {
        let image = Image::from_fn(PhysicalSize::new(16, 16), |x, _| {
            if x < 8 {
                Rgba8::rgb(40, 120, 200)
            } else {
                Rgba8::TRANSPARENT
            }
        });
        let bytes = encode(&image, Format::Jpeg).unwrap();
        assert_eq!(Format::detect(&bytes), Some(Format::Jpeg));
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.size(), image.size());
        let close = |p: Rgba8, q: Rgba8| {
            p.a == 255 && p.r.abs_diff(q.r) <= 6 && p.g.abs_diff(q.g) <= 6 && p.b.abs_diff(q.b) <= 6
        };
        assert!(close(
            decoded.pixel(2, 2).unwrap(),
            Rgba8::rgb(40, 120, 200)
        ));
        assert!(close(decoded.pixel(13, 2).unwrap(), Rgba8::WHITE));
    }

    /// `jpeg` with an EXIF APP1 segment recording `orientation` inserted after the
    /// start-of-image marker.
    fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut exif = b"Exif\0\0".to_vec();
        // Big-endian TIFF header, then one IFD with a single Orientation (0x0112)
        // SHORT entry and no next IFD.
        exif.extend_from_slice(b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01");
        exif.extend_from_slice(&orientation.to_be_bytes());
        exif.extend_from_slice(&[0; 2 + 4]);
        let length = u16::try_from(exif.len() + 2).unwrap();
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(&exif);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn exif_orientation_is_applied_on_decode() {
        // 16×8, red on the left and blue on the right, tagged "rotate 90° clockwise
        // to display": upright it is 8×16, red on top and blue below.
        let image = Image::from_fn(PhysicalSize::new(16, 8), |x, _| {
            if x < 8 {
                Rgba8::rgb(255, 0, 0)
            } else {
                Rgba8::rgb(0, 0, 255)
            }
        });
        let bytes = with_exif_orientation(&encode(&image, Format::Jpeg).unwrap(), 6);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.size(), PhysicalSize::new(8, 16));
        let top = decoded.pixel(4, 3).unwrap();
        let bottom = decoded.pixel(4, 12).unwrap();
        assert!(top.r > 200 && top.b < 60, "top is red: {top:?}");
        assert!(
            bottom.b > 200 && bottom.r < 60,
            "bottom is blue: {bottom:?}"
        );
    }

    #[test]
    fn jpeg_quality_trades_size_for_fidelity() {
        let image = sample();
        let low = encode_jpeg(&image, 10).unwrap();
        let high = encode_jpeg(&image, 100).unwrap();
        assert!(low.len() < high.len());
    }

    #[test]
    fn gif_bmp_and_tiff_decode() {
        let image = opaque_two_color();
        for format in [ImageFormat::Gif, ImageFormat::Bmp, ImageFormat::Tiff] {
            let bytes = foreign(&image, format);
            assert_eq!(decode(&bytes).unwrap(), image, "{format:?}");
        }
    }

    #[test]
    fn formats_are_detected_from_content() {
        let image = opaque_two_color();
        for (format, ours) in [
            (ImageFormat::Gif, Format::Gif),
            (ImageFormat::Bmp, Format::Bmp),
            (ImageFormat::Tiff, Format::Tiff),
        ] {
            assert_eq!(Format::detect(&foreign(&image, format)), Some(ours));
        }
        assert_eq!(Format::detect(b"not an image at all"), None);
        assert_eq!(Format::detect(&[]), None);
        // A format `image` knows but Chartreuse does not read.
        assert_eq!(Format::detect(b"qoif\0\0\0\x01\0\0\0\x01\x04\0"), None);
    }

    #[test]
    fn formats_are_found_by_extension_in_any_case() {
        assert_eq!(Format::from_extension("PNG"), Some(Format::Png));
        assert_eq!(Format::from_extension("jpeg"), Some(Format::Jpeg));
        assert_eq!(Format::from_extension("Jpg"), Some(Format::Jpeg));
        assert_eq!(Format::from_extension("tif"), Some(Format::Tiff));
        assert_eq!(Format::from_extension("heic"), None);
        assert_eq!(
            Format::from_path(Path::new("/tmp/Shot 1.WebP")),
            Some(Format::WebP)
        );
        assert_eq!(Format::from_path(Path::new("/tmp/png")), None);
        for format in Format::ALL {
            assert_eq!(Format::from_extension(format.extension()), Some(format));
        }
    }

    #[test]
    fn exactly_the_encodable_formats_encode() {
        let image = opaque_two_color();
        for format in Format::ALL {
            let result = encode(&image, format);
            assert_eq!(result.is_ok(), format.can_encode(), "{format}");
            assert_eq!(Format::ENCODABLE.contains(&format), format.can_encode());
            if let Err(error) = result {
                assert!(matches!(error, Error::Encode(_)));
            }
        }
    }

    #[test]
    fn empty_images_do_not_encode() {
        let empty = Image::new(PhysicalSize::new(0, 3), Vec::new()).unwrap();
        for format in Format::ENCODABLE {
            assert!(matches!(encode(&empty, format), Err(Error::Encode(_))));
        }
    }

    #[test]
    fn unknown_corrupt_or_mislabelled_data_fails_to_decode() {
        assert!(matches!(decode(b"hello"), Err(Error::Decode(_))));
        let png = encode(&sample(), Format::Png).unwrap();
        assert!(matches!(
            decode(&png[..png.len() / 2]),
            Err(Error::Decode(_))
        ));
        assert!(matches!(
            decode_as(&png, Format::Jpeg),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn files_decode_by_content_not_extension() {
        let dir = std::env::temp_dir().join(format!("chartreuse-imaging-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("actually-a-png.jpg");
        std::fs::write(&path, encode(&sample(), Format::Png).unwrap()).unwrap();
        let decoded = decode_file(&path);
        let missing = decode_file(&dir.join("missing.png"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(decoded.unwrap(), sample());
        assert!(matches!(missing, Err(Error::Io { .. })));
    }
}
