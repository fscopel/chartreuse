//! Blur regions' pixels for the canvas: what flatten puts in each region,
//! computed by [`flatten::obscured`] and kept as a renderer image until the
//! region, its mode, or what beneath it can affect it changes.

use std::cell::RefCell;

use chartreuse_core::geometry::PhysicalRect;
use iced::advanced::image;

use crate::flatten::{self, Drawn};
use crate::model::{AnnotationId, BlurMode, BlurRegion, Document, Shape, Style};

/// A blur region's pixels, ready to draw.
#[derive(Debug, Clone)]
pub(crate) struct Obscured {
    /// Where they are, in image pixels.
    pub(crate) pixels: PhysicalRect,
    /// Their contents, [`pixels`](Self::pixels)-sized.
    pub(crate) image: image::Handle,
}

/// The canvas's blur region pixels: one entry per region (by annotation id,
/// or `None` for a region being drawn), each kept while what it was computed
/// from stays the same, so the renderer keeps its upload too.
#[derive(Debug, Default)]
pub(crate) struct Rasters {
    entries: RefCell<Vec<Entry>>,
}

#[derive(Debug)]
struct Entry {
    id: Option<AnnotationId>,
    key: Key,
    obscured: Option<Obscured>,
}

/// Everything a region's pixels are computed from besides the base image,
/// which never changes in an editor.
#[derive(Debug, PartialEq)]
struct Key {
    region: BlurRegion,
    mode: BlurMode,
    /// The annotations beneath it that can affect it (see
    /// [`flatten::beneath`]): shape, style, and step number.
    beneath: Vec<(Shape, Style, Option<usize>)>,
}

impl Key {
    fn matches(&self, region: &BlurRegion, mode: BlurMode, beneath: &[Drawn<'_>]) -> bool {
        self.region == *region
            && self.mode == mode
            && self.beneath.len() == beneath.len()
            && self
                .beneath
                .iter()
                .zip(beneath)
                .all(|((shape, style, number), drawn)| {
                    shape == drawn.shape && style == drawn.style && *number == drawn.number
                })
    }
}

impl Rasters {
    /// The pixels of blur region `region`, in `mode`, over `below` (the
    /// annotations beneath it as displayed, bottom to top) in `document`.
    /// `id` is the region's annotation, or `None` for a new region being
    /// drawn. Computed afresh only if the region, the mode, or an annotation
    /// beneath it that can affect it has changed since the last call for the
    /// same `id`. `None` if the region covers none of the image.
    ///
    /// Also forgets the regions no longer in `document`.
    pub(crate) fn get(
        &self,
        document: &Document,
        id: Option<AnnotationId>,
        region: &BlurRegion,
        mode: BlurMode,
        below: &[Drawn<'_>],
    ) -> Option<Obscured> {
        let base = document.base();
        let beneath = flatten::beneath(base, below, region);
        let mut entries = self.entries.borrow_mut();
        entries.retain(|entry| entry.id.is_none_or(|id| document.get(id).is_some()));
        if let Some(entry) = entries.iter().find(|entry| entry.id == id)
            && entry.key.matches(region, mode, &beneath)
        {
            return entry.obscured.clone();
        }
        let obscured =
            flatten::obscured(base, &beneath, region, mode).map(|(pixels, image)| Obscured {
                pixels,
                image: image::Handle::from_rgba(image.width(), image.height(), image.into_pixels()),
            });
        let key = Key {
            region: region.clone(),
            mode,
            beneath: beneath
                .iter()
                .map(|drawn| (drawn.shape.clone(), *drawn.style, drawn.number))
                .collect(),
        };
        entries.retain(|entry| entry.id != id);
        entries.push(Entry {
            id,
            key,
            obscured: obscured.clone(),
        });
        obscured
    }
}

#[cfg(test)]
mod tests {
    use chartreuse_core::color::Rgba8;
    use chartreuse_core::geometry::PhysicalSize;
    use chartreuse_core::image::Image;

    use super::*;
    use crate::model::{Line, Point, Rect};

    fn line(y: f32) -> Shape {
        Shape::Line(Line {
            start: Point::new(0.0, y),
            end: Point::new(100.0, y),
        })
    }

    #[test]
    fn a_region_is_recomputed_only_when_what_it_shows_changes() {
        let mut document = Document::new(Image::filled(
            PhysicalSize::new(100, 100),
            Rgba8::rgb(255, 255, 255),
        ));
        let region = BlurRegion {
            rect: Rect::from_corners(Point::new(10.0, 10.0), Point::new(50.0, 50.0)),
        };
        let id = document.add(Shape::Blur(region.clone()), Style::default());
        let style = Style::default();
        let (inside, outside, moved) = (line(30.0), line(90.0), line(40.0));
        let drawn = |shape| Drawn {
            shape,
            style: &style,
            number: None,
        };
        let rasters = Rasters::default();
        let get = |region: &BlurRegion, mode, below: &[Drawn<'_>]| {
            rasters
                .get(&document, Some(id), region, mode, below)
                .unwrap()
                .image
                .id()
        };

        let first = get(&region, BlurMode::Pixelate, &[drawn(&inside)]);
        assert_eq!(
            get(
                &region,
                BlurMode::Pixelate,
                &[drawn(&inside), drawn(&outside)]
            ),
            first,
            "an annotation that can't reach the region changes nothing"
        );
        let changed = get(&region, BlurMode::Pixelate, &[drawn(&moved)]);
        assert_ne!(changed, first, "beneath changed");
        let blurred = get(&region, BlurMode::Gaussian, &[drawn(&moved)]);
        assert_ne!(blurred, changed, "mode changed");
        let bigger = BlurRegion {
            rect: region.rect.expand(5.0),
        };
        assert_ne!(get(&bigger, BlurMode::Gaussian, &[drawn(&moved)]), blurred);

        // Off the image: nothing to show.
        let off = BlurRegion {
            rect: Rect::from_corners(Point::new(200.0, 0.0), Point::new(300.0, 50.0)),
        };
        assert!(rasters
            .get(&document, None, &off, BlurMode::Pixelate, &[])
            .is_none());
    }
}
