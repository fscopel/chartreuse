//! Pixelate and blur, for obscuring part of an image.
//!
//! Both kernels work in place on a [`PhysicalRect`] of the image, clipped to the
//! image. They only read pixels inside that rectangle, so nothing outside it leaks
//! in, and they never write outside it. Averages are taken on premultiplied colors
//! (weighting each pixel's color by its alpha), so transparent pixels do not darken
//! or tint the result; a result that is fully transparent is stored as
//! transparent black.

use std::num::NonZeroU32;

use chartreuse_core::geometry::PhysicalRect;
use chartreuse_core::image::Image;

use crate::region::clip;

/// Replaces `region` with a mosaic of `block` × `block` squares, each filled with
/// the average of the pixels it covers.
///
/// The grid starts at `region`'s top-left corner, so blocks along its right and
/// bottom edges (and any cut off by the image's edges) can be smaller.
pub fn pixelate(image: &mut Image, region: PhysicalRect, block: NonZeroU32) {
    let Some(area) = clip(image, region) else {
        return;
    };
    let block = i64::from(block.get());
    let stride = image.width() as usize * 4;
    let pixels = image.pixels_mut();
    // The grid cells overlapping `area`, in coordinates relative to `region`.
    let cells = |min: i32, max: i64, origin: i32| {
        let first = (i64::from(min) - i64::from(origin)) / block;
        let last = (max - 1 - i64::from(origin)) / block;
        (first..=last).map(move |cell| {
            let start = i64::from(origin) + cell * block;
            // Clipped to `area`, which lies inside the image: valid indices.
            let from = start.max(i64::from(min)) as usize;
            let to = (start + block).min(max) as usize;
            from..to
        })
    };
    for rows in cells(area.min_y(), area.max_y(), region.min_y()) {
        for columns in cells(area.min_x(), area.max_x(), region.min_x()) {
            let mut sum = [0_u64; 4];
            for y in rows.clone() {
                for x in columns.clone() {
                    let p = &pixels[y * stride + x * 4..][..4];
                    let alpha = u64::from(p[3]);
                    for k in 0..3 {
                        sum[k] += u64::from(p[k]) * alpha;
                    }
                    sum[3] += alpha;
                }
            }
            let count = (rows.len() * columns.len()) as u64;
            let color = unpremultiply_sums(sum, count);
            for y in rows.clone() {
                for x in columns.clone() {
                    pixels[y * stride + x * 4..][..4].copy_from_slice(&color);
                }
            }
        }
    }
}

/// The straight-alpha average of pixels whose color channels were summed
/// premultiplied (`Σ c·a`) and whose alphas were summed plainly (`Σ a`).
fn unpremultiply_sums(sum: [u64; 4], count: u64) -> [u8; 4] {
    let alpha = sum[3];
    if alpha == 0 {
        return [0; 4];
    }
    let channel = |c: u64| ((c + alpha / 2) / alpha).min(255) as u8;
    [
        channel(sum[0]),
        channel(sum[1]),
        channel(sum[2]),
        ((alpha + count / 2) / count) as u8,
    ]
}

/// Blurs `region` with an approximately Gaussian kernel of standard deviation
/// about `radius` pixels (three passes of a box blur `2 × radius + 1` wide, in each
/// direction). A radius of 0 changes nothing.
///
/// Near the edges of `region` each output pixel averages only the pixels inside
/// it, so a uniform area stays exactly uniform right up to the edge.
pub fn blur(image: &mut Image, region: PhysicalRect, radius: u32) {
    let Some(area) = clip(image, region) else {
        return;
    };
    if radius == 0 {
        return;
    }
    let (width, height) = (area.size.width as usize, area.size.height as usize);
    let (left, top) = (area.min_x() as usize, area.min_y() as usize);
    let stride = image.width() as usize * 4;
    let pixels = image.pixels_mut();

    // Premultiplied and scaled by 255, which fits a u16 exactly: each color channel
    // holds c·a and the alpha channel a·255, all in 0..=65025.
    let mut buffer: Vec<[u16; 4]> = Vec::with_capacity(width * height);
    for y in 0..height {
        let row = &pixels[(top + y) * stride + left * 4..][..width * 4];
        buffer.extend(row.as_chunks::<4>().0.iter().map(|p| {
            let alpha = u16::from(p[3]);
            [
                u16::from(p[0]) * alpha,
                u16::from(p[1]) * alpha,
                u16::from(p[2]) * alpha,
                alpha * 255,
            ]
        }));
    }

    let radius = radius as usize;
    let mut line = Vec::new();
    let mut prefix = Vec::new();
    for _ in 0..3 {
        for row in buffer.chunks_exact_mut(width) {
            box_blur_line(row, radius, &mut prefix);
        }
    }
    for x in 0..width {
        line.clear();
        line.extend((0..height).map(|y| buffer[y * width + x]));
        for _ in 0..3 {
            box_blur_line(&mut line, radius, &mut prefix);
        }
        for (y, value) in line.iter().enumerate() {
            buffer[y * width + x] = *value;
        }
    }

    for y in 0..height {
        let row = &mut pixels[(top + y) * stride + left * 4..][..width * 4];
        for (out, value) in row
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(&buffer[y * width..][..width])
        {
            *out = unpremultiply_scaled(*value);
        }
    }
}

/// Replaces each value with the average of the values within `radius` of it that
/// lie inside `values`. `prefix` is scratch space.
fn box_blur_line(values: &mut [[u16; 4]], radius: usize, prefix: &mut Vec<[u64; 4]>) {
    let n = values.len();
    prefix.clear();
    prefix.push([0; 4]);
    let mut running = [0_u64; 4];
    for value in values.iter() {
        for k in 0..4 {
            running[k] += u64::from(value[k]);
        }
        prefix.push(running);
    }
    for (i, value) in values.iter_mut().enumerate() {
        let from = i.saturating_sub(radius);
        let to = (i + radius + 1).min(n);
        let count = (to - from) as u64;
        for k in 0..4 {
            let sum = prefix[to][k] - prefix[from][k];
            // An average of values ≤ 65025 is ≤ 65025.
            value[k] = ((sum + count / 2) / count) as u16;
        }
    }
}

/// Converts a `[c·a, c·a, c·a, a·255]` value back to straight-alpha RGBA8.
fn unpremultiply_scaled(value: [u16; 4]) -> [u8; 4] {
    let alpha = u32::from(value[3]);
    if alpha == 0 {
        return [0; 4];
    }
    // c = (c·a) / a = (c·a) · 255 / (a·255).
    let channel = |c: u16| ((u32::from(c) * 255 + alpha / 2) / alpha).min(255) as u8;
    [
        channel(value[0]),
        channel(value[1]),
        channel(value[2]),
        ((alpha + 127) / 255) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use chartreuse_core::color::Rgba8;
    use chartreuse_core::geometry::PhysicalSize;

    use super::*;
    use crate::region::bounds;

    fn block(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).unwrap()
    }

    /// Pixel `(x, y)` has red `10·x` and green `10·y`.
    fn gradient(width: u32, height: u32) -> Image {
        Image::from_fn(PhysicalSize::new(width, height), |x, y| {
            Rgba8::rgb((x * 10) as u8, (y * 10) as u8, 0)
        })
    }

    fn px(image: &Image, x: u32, y: u32) -> Rgba8 {
        image.pixel(x, y).unwrap()
    }

    #[test]
    fn pixelate_fills_each_block_with_its_average() {
        let mut image = gradient(4, 4);
        pixelate(&mut image, PhysicalRect::new(0, 0, 4, 4), block(2));
        // The top-left block holds red 0 and 10, green 0 and 10.
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(px(&image, x, y), Rgba8::rgb(5, 5, 0));
        }
        assert_eq!(px(&image, 3, 3), Rgba8::rgb(25, 25, 0));
        assert_eq!(px(&image, 2, 1), Rgba8::rgb(25, 5, 0));
    }

    #[test]
    fn pixelate_grid_follows_the_region_and_stays_inside_it() {
        let original = gradient(5, 5);
        let mut image = original.clone();
        // Starts off the image: the grid cells are x (and y) -1..1 and 1..3, and
        // column and row 3.. are outside the region.
        pixelate(&mut image, PhysicalRect::new(-1, -1, 4, 4), block(2));
        assert_eq!(px(&image, 0, 0), px(&original, 0, 0), "a one-pixel cell");
        for (x, y) in [(1, 1), (2, 1), (1, 2), (2, 2)] {
            assert_eq!(px(&image, x, y), Rgba8::rgb(15, 15, 0));
        }
        // Cells cut by the image edge: (1..3, 0) and (0, 1..3).
        assert_eq!(px(&image, 2, 0), Rgba8::rgb(15, 0, 0));
        assert_eq!(px(&image, 0, 2), Rgba8::rgb(0, 15, 0));
        for i in 0..5 {
            assert_eq!(px(&image, 3, i), px(&original, 3, i));
            assert_eq!(px(&image, i, 3), px(&original, i, 3));
        }
    }

    #[test]
    fn pixelate_ignores_the_color_of_transparent_pixels() {
        let mut image = Image::from_fn(PhysicalSize::new(2, 1), |x, _| {
            if x == 0 {
                Rgba8::rgb(200, 0, 0)
            } else {
                Rgba8::new(0, 255, 0, 0)
            }
        });
        let all = bounds(&image);
        pixelate(&mut image, all, block(2));
        assert_eq!(px(&image, 0, 0), Rgba8::new(200, 0, 0, 128));
        assert_eq!(px(&image, 1, 0), Rgba8::new(200, 0, 0, 128));
    }

    #[test]
    fn blur_spreads_detail_no_further_than_its_support() {
        // A white pixel at x = 5 on black. Radius 1, three passes: it reaches
        // exactly 3 pixels each way.
        let mut image = Image::from_fn(PhysicalSize::new(11, 1), |x, _| {
            if x == 5 {
                Rgba8::WHITE
            } else {
                Rgba8::BLACK
            }
        });
        let all = bounds(&image);
        blur(&mut image, all, 1);
        let reds: Vec<u8> = (0..11).map(|x| px(&image, x, 0).r).collect();
        assert_eq!(reds[0..2], [0, 0]);
        assert_eq!(reds[9..11], [0, 0]);
        assert!(reds[2] > 0 && reds[8] > 0);
        assert_eq!(reds[4], reds[6], "symmetric");
        assert!(reds[5] > reds[4] && reds[4] > reds[3] && reds[3] > reds[2]);
        assert!(reds[5] < 255, "the peak is flattened");
    }

    #[test]
    fn blur_reads_and_writes_only_inside_the_region() {
        // Black left half, white right half; blur only the left half.
        let original = Image::from_fn(PhysicalSize::new(8, 4), |x, _| {
            if x < 4 {
                Rgba8::BLACK
            } else {
                Rgba8::WHITE
            }
        });
        let mut image = original.clone();
        blur(&mut image, PhysicalRect::new(0, 0, 4, 4), 3);
        assert_eq!(image, original, "no white bleeds into the region");
        // Blurring across the boundary does mix them, but only inside the region.
        blur(&mut image, PhysicalRect::new(2, 1, 4, 2), 3);
        assert!(px(&image, 3, 1).r > 0);
        assert!(px(&image, 4, 1).r < 255);
        assert_eq!(px(&image, 3, 0), Rgba8::BLACK);
        assert_eq!(px(&image, 6, 1), Rgba8::WHITE);
        assert_eq!(px(&image, 1, 1), Rgba8::BLACK);
    }

    #[test]
    fn blur_keeps_uniform_areas_exact_up_to_the_image_edge() {
        let color = Rgba8::new(12, 34, 56, 78);
        let mut image = Image::filled(PhysicalSize::new(6, 5), color);
        blur(&mut image, PhysicalRect::new(-3, -3, 20, 20), 4);
        assert_eq!(image, Image::filled(PhysicalSize::new(6, 5), color));
    }

    #[test]
    fn blur_ignores_the_color_of_transparent_pixels() {
        let mut image = Image::from_fn(PhysicalSize::new(4, 1), |x, _| {
            if x < 2 {
                Rgba8::rgb(0, 0, 255)
            } else {
                Rgba8::new(255, 255, 0, 0)
            }
        });
        let all = bounds(&image);
        blur(&mut image, all, 1);
        for x in 0..4 {
            let p = px(&image, x, 0);
            assert!(p.a > 0 && p.a < 255, "x = {x}: {p:?}");
            assert_eq!((p.r, p.g, p.b), (0, 0, 255), "x = {x}");
        }
    }

    #[test]
    fn zero_radius_empty_or_off_image_regions_change_nothing() {
        let original = gradient(4, 4);
        let mut image = original.clone();
        blur(&mut image, bounds(&original), 0);
        blur(&mut image, PhysicalRect::new(1, 1, 0, 3), 5);
        blur(&mut image, PhysicalRect::new(4, 0, 3, 3), 5);
        pixelate(&mut image, PhysicalRect::new(-5, -5, 5, 5), block(3));
        pixelate(&mut image, bounds(&original), block(1));
        assert_eq!(image, original);
    }
}
