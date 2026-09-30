//! Drop shadows' pixels for the canvas: what flatten draws beneath each shape
//! that casts one, computed by [`flatten::cast_shadow`] and kept as a
//! renderer image until the shape or its style changes.

use std::cell::RefCell;

use chartreuse_core::geometry::PhysicalRect;
use iced::advanced::image;

use crate::flatten;
use crate::model::{AnnotationId, Document, Shape, Style};

/// A shadow's pixels, ready to draw.
#[derive(Debug, Clone)]
pub(crate) struct Shadow {
    /// Where they are, in image pixels.
    pub(crate) pixels: PhysicalRect,
    /// Their contents, [`pixels`](Self::pixels)-sized.
    pub(crate) image: image::Handle,
}

/// The canvas's shadow pixels: one entry per shape (by annotation id, or
/// `None` for a shape being drawn), each kept while the shape and its style
/// stay the same, so the renderer keeps its upload too.
#[derive(Debug, Default)]
pub(crate) struct Shadows {
    entries: RefCell<Vec<Entry>>,
}

#[derive(Debug)]
struct Entry {
    id: Option<AnnotationId>,
    shape: Shape,
    style: Style,
    shadow: Option<Shadow>,
}

impl Shadows {
    /// The pixels of the shadow `shape` casts in `style` on `document`'s
    /// image. `id` is the shape's annotation, or `None` for a new shape being
    /// drawn. Computed afresh only if the shape or its style has changed
    /// since the last call for the same `id`. `None` if it casts none on the
    /// image.
    ///
    /// Also forgets the annotations no longer in `document`.
    pub(crate) fn get(
        &self,
        document: &Document,
        id: Option<AnnotationId>,
        shape: &Shape,
        style: &Style,
    ) -> Option<Shadow> {
        let mut entries = self.entries.borrow_mut();
        entries.retain(|entry| entry.id.is_none_or(|id| document.get(id).is_some()));
        if let Some(entry) = entries.iter().find(|entry| entry.id == id)
            && entry.shape == *shape
            && entry.style == *style
        {
            return entry.shadow.clone();
        }
        let shadow =
            flatten::cast_shadow(shape, style, document.base().size()).map(|(pixels, pixmap)| {
                Shadow {
                    pixels,
                    // Black: the same bytes premultiplied or straight.
                    image: image::Handle::from_rgba(
                        pixels.size.width,
                        pixels.size.height,
                        pixmap.take(),
                    ),
                }
            });
        entries.retain(|entry| entry.id != id);
        entries.push(Entry {
            id,
            shape: shape.clone(),
            style: *style,
            shadow: shadow.clone(),
        });
        shadow
    }
}

#[cfg(test)]
mod tests {
    use chartreuse_core::color::Rgba8;
    use chartreuse_core::geometry::PhysicalSize;
    use chartreuse_core::image::Image;

    use super::*;
    use crate::model::{Line, Point};

    fn line(y: f32) -> Shape {
        Shape::Line(Line {
            start: Point::new(0.0, y),
            end: Point::new(100.0, y),
        })
    }

    #[test]
    fn a_shadow_is_recomputed_only_when_its_shape_or_style_changes() {
        let mut document = Document::new(Image::filled(
            PhysicalSize::new(100, 100),
            Rgba8::rgb(255, 255, 255),
        ));
        let style = Style::default();
        let id = document.add(line(30.0), style);
        let shadows = Shadows::default();
        let get = |shape: &Shape, style: &Style| {
            shadows
                .get(&document, Some(id), shape, style)
                .unwrap()
                .image
                .id()
        };

        let first = get(&line(30.0), &style);
        assert_eq!(get(&line(30.0), &style), first, "unchanged: kept");
        let moved = get(&line(40.0), &style);
        assert_ne!(moved, first, "moved");
        let thicker = Style {
            stroke_width: 20.0,
            ..style
        };
        assert_ne!(get(&line(40.0), &thicker), moved, "restyled");

        // Off the image, or a kind that casts none: nothing to show.
        assert!(shadows.get(&document, None, &line(300.0), &style).is_none());
        let highlighter = Shape::Highlighter(crate::model::Polyline {
            points: vec![Point::new(50.0, 50.0)],
        });
        assert!(shadows.get(&document, None, &highlighter, &style).is_none());
    }
}
