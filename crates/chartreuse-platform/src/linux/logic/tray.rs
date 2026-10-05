//! The StatusNotifierItem tray's icon pixmaps and menu labels.

use chartreuse_core::image::Image;

/// `image` as a StatusNotifierItem `IconPixmap`: ARGB32 in network byte order
/// (alpha, red, green, blue per pixel), straight alpha.
#[must_use]
pub fn argb32(image: &Image) -> Vec<u8> {
    image
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect()
}

/// `label` for a com.canonical.dbusmenu item: a single underscore marks the
/// next character as the access key, so literal underscores are doubled.
#[must_use]
pub fn menu_label(label: &str) -> String {
    label.replace('_', "__")
}

#[cfg(test)]
mod tests {
    use chartreuse_core::color::Rgba8;
    use chartreuse_core::geometry::PhysicalSize;

    use super::*;

    #[test]
    fn pixmaps_put_alpha_first_in_row_order() {
        let image = Image::from_fn(PhysicalSize::new(2, 1), |x, _| {
            if x == 0 {
                Rgba8::new(1, 2, 3, 4)
            } else {
                Rgba8::new(5, 6, 7, 255)
            }
        });
        assert_eq!(argb32(&image), [4, 1, 2, 3, 255, 5, 6, 7]);
    }

    #[test]
    fn underscores_are_not_taken_as_access_keys() {
        assert_eq!(menu_label("Open File…"), "Open File…");
        assert_eq!(menu_label("snake_case_name"), "snake__case__name");
    }
}
