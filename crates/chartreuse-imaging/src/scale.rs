//! Resizing an image by resampling it.

use chartreuse_core::geometry::PhysicalSize;
use chartreuse_core::image::Image;
use chartreuse_core::{Error, Result};
use image::imageops::{self, FilterType};
use image::{ImageBuffer, Rgba, Rgba32FImage};

/// The resampling filter: Catmull-Rom (bicubic), sharp without the ringing
/// Lanczos leaves around the hard edges of text and UI.
const FILTER: FilterType = FilterType::CatmullRom;

/// `image` resampled to `size`, with a bicubic (Catmull-Rom) filter; each
/// axis is scaled independently, so the aspect ratio may change.
///
/// An opaque image is resampled as is. One with translucent pixels is
/// resampled with its colors premultiplied by their alpha, so the color of
/// fully transparent pixels (which carries no meaning) never bleeds into
/// their neighbors.
///
/// # Errors
///
/// [`Error::InvalidImage`] if `image` or `size` covers no pixels.
pub fn resize(image: &Image, size: PhysicalSize) -> Result<Image> {
    if image.size().is_empty() || size.is_empty() {
        return Err(Error::InvalidImage(format!(
            "cannot resize a {}×{} image to {}×{}",
            image.width(),
            image.height(),
            size.width,
            size.height
        )));
    }
    if size == image.size() {
        return Ok(image.clone());
    }
    let (width, height) = (image.width(), image.height());
    let pixels = if image
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| pixel[3] == u8::MAX)
    {
        let source = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, image.pixels())
            .expect("an Image's buffer holds exactly its pixels");
        imageops::resize(&source, size.width, size.height, FILTER).into_raw()
    } else {
        let premultiplied = image
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| {
                let [r, g, b, a] = pixel.map(|channel| f32::from(channel) / 255.0);
                [r * a, g * a, b * a, a]
            })
            .collect();
        let source = Rgba32FImage::from_raw(width, height, premultiplied)
            .expect("one premultiplied pixel per pixel");
        imageops::resize(&source, size.width, size.height, FILTER)
            .pixels()
            .flat_map(|&Rgba([r, g, b, a])| {
                let a = a.clamp(0.0, 1.0);
                let straight = |c: f32| {
                    if a > 0.0 {
                        (c / a).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                };
                [straight(r), straight(g), straight(b), a].map(|c| (c * 255.0).round() as u8)
            })
            .collect()
    };
    Image::new(size, pixels)
}

#[cfg(test)]
mod tests {
    use chartreuse_core::color::Rgba8;

    use super::*;

    #[test]
    fn each_axis_scales_to_the_size_asked_for() {
        let color = Rgba8::rgb(12, 200, 99);
        let resized = resize(
            &Image::filled(PhysicalSize::new(4, 4), color),
            PhysicalSize::new(7, 3),
        );
        assert_eq!(
            resized.unwrap(),
            Image::filled(PhysicalSize::new(7, 3), color)
        );
    }

    #[test]
    fn transparent_colors_do_not_bleed_into_their_neighbors() {
        // Opaque red on the left, fully transparent green on the right.
        let image = Image::from_fn(PhysicalSize::new(8, 2), |x, _| {
            if x < 4 {
                Rgba8::rgb(255, 0, 0)
            } else {
                Rgba8::new(0, 255, 0, 0)
            }
        });
        let resized = resize(&image, PhysicalSize::new(3, 1)).unwrap();
        let middle = resized.pixel(1, 0).unwrap();
        assert!(middle.a > 0 && middle.a < 255, "{middle:?}");
        assert_eq!((middle.r, middle.g, middle.b), (255, 0, 0));
    }

    #[test]
    fn empty_images_and_sizes_are_rejected() {
        let image = Image::filled(PhysicalSize::new(2, 2), Rgba8::WHITE);
        assert!(resize(&image, PhysicalSize::new(0, 2)).is_err());
        let empty = Image::filled(PhysicalSize::new(0, 0), Rgba8::WHITE);
        assert!(resize(&empty, PhysicalSize::new(2, 2)).is_err());
    }
}
