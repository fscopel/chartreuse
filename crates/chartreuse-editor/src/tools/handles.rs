//! Selection handles: the points of a lone selected annotation that can be
//! dragged to reshape it (a line's or arrow's ends, the corners of a
//! rectangle, a blur region, or an ellipse's bounding box). Text has none;
//! its size follows its font size.

use super::line::snap_45;
use super::rectangle::square;
use crate::model::{
    AnnotationId, BlurRegion, Command, Document, Ellipse, Point, Rect, Rectangle, Shape, Vector,
};

/// A handle's drawn size (a square), in canvas pixels.
pub const HANDLE_SIZE: f32 = 8.0;

/// How close to a handle's center a press grabs it, in canvas pixels.
pub const HANDLE_REACH: f32 = 7.0;

/// A draggable point of a lone selected annotation (see [`handles`]): a
/// line's or arrow's ends, or the corners of a rectangle or of an ellipse's
/// bounding box. Text has none; its size follows its font size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handle {
    /// A line's or arrow's `start`.
    Start,
    /// A line's or arrow's `end` (an arrow's tip).
    End,
    /// A corner of a rectangle, a blur region, or an ellipse's bounding box, as an index
    /// into [`Rect::corners`] (clockwise from the top-left).
    Corner(usize),
}

/// The handles of `shape` and where they are.
#[must_use]
pub fn handles(shape: &Shape) -> Vec<(Handle, Point)> {
    match shape {
        Shape::Line(line) => vec![(Handle::Start, line.start), (Handle::End, line.end)],
        Shape::Arrow(arrow) => vec![(Handle::Start, arrow.start), (Handle::End, arrow.end)],
        Shape::Rectangle(Rectangle { rect })
        | Shape::Ellipse(Ellipse { rect })
        | Shape::Blur(BlurRegion { rect }) => corners(*rect),
        Shape::Pen(_) | Shape::Highlighter(_) | Shape::Step(_) | Shape::Text(_) => Vec::new(),
    }
}

fn corners(rect: Rect) -> Vec<(Handle, Point)> {
    rect.corners()
        .into_iter()
        .enumerate()
        .map(|(index, corner)| (Handle::Corner(index), corner))
        .collect()
}

/// `rect` with corner `index` dragged to `to`, the opposite corner fixed;
/// `constrain` makes it square.
fn drag_corner(rect: Rect, index: usize, to: Point, constrain: bool) -> Rect {
    let opposite = rect.corners()[(index + 2) % 4];
    let to = if constrain { square(opposite, to) } else { to };
    Rect::from_corners(opposite, to)
}

/// `shape` with `handle` dragged to `to`. A line's or arrow's other end stays
/// put, as does the opposite corner of a rectangle or an ellipse's bounding
/// box. `constrain` (Shift) snaps a line or arrow to 45° and makes a
/// rectangle square or an ellipse a circle.
#[must_use]
pub fn reshaped(shape: &Shape, handle: Handle, to: Point, constrain: bool) -> Shape {
    let mut shape = shape.clone();
    match (&mut shape, handle) {
        (Shape::Line(line), Handle::Start) => {
            line.start = if constrain { snap_45(line.end, to) } else { to };
        }
        (Shape::Line(line), Handle::End) => {
            line.end = if constrain {
                snap_45(line.start, to)
            } else {
                to
            };
        }
        (Shape::Arrow(arrow), Handle::Start) => {
            arrow.start = if constrain {
                snap_45(arrow.end, to)
            } else {
                to
            };
        }
        (Shape::Arrow(arrow), Handle::End) => {
            arrow.end = if constrain {
                snap_45(arrow.start, to)
            } else {
                to
            };
        }
        (
            Shape::Rectangle(Rectangle { rect })
            | Shape::Ellipse(Ellipse { rect })
            | Shape::Blur(BlurRegion { rect }),
            Handle::Corner(index),
        ) => {
            *rect = drag_corner(*rect, index, to, constrain);
        }
        _ => {}
    }
    shape
}

/// The handle under `at` (within `reach`, in document units) of the selected
/// annotation, if exactly one is selected. The nearest wins.
#[must_use]
pub fn handle_at(document: &Document, at: Point, reach: f32) -> Option<(AnnotationId, Handle)> {
    let mut selected = document.selected();
    let (Some(annotation), None) = (selected.next(), selected.next()) else {
        return None;
    };
    handles(&annotation.shape)
        .into_iter()
        .map(|(handle, point)| (handle, point.distance(at)))
        .filter(|&(_, distance)| distance <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(handle, _)| (annotation.id(), handle))
}

/// Dragging a handle: previewed until released, then one
/// [`Command::Reshape`]. The handle keeps its offset from the pointer, so
/// grabbing it slightly off-center does not make it jump.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Reshape {
    id: AnnotationId,
    handle: Handle,
    from: Point,
    /// From the pointer to the handle when grabbed.
    grab: Vector,
    original: Shape,
    shape: Shape,
    /// Whether the pointer has left the drag threshold around `from`.
    moved: bool,
}

impl Reshape {
    /// Starts dragging `handle` of annotation `id` from `from`.
    pub(super) fn start(
        document: &Document,
        id: AnnotationId,
        handle: Handle,
        from: Point,
    ) -> Option<Self> {
        let original = document.get(id)?.shape.clone();
        let (_, point) = handles(&original)
            .into_iter()
            .find(|&(candidate, _)| candidate == handle)?;
        Some(Self {
            id,
            handle,
            from,
            grab: point - from,
            shape: original.clone(),
            original,
            moved: false,
        })
    }

    pub(super) fn drag_to(&mut self, to: Point, constrain: bool, threshold: f32) {
        self.moved |= to.distance(self.from) > threshold;
        if self.moved {
            self.shape = reshaped(&self.original, self.handle, to + self.grab, constrain);
        }
    }

    pub(super) const fn id(&self) -> AnnotationId {
        self.id
    }

    /// The shape as dragged so far.
    pub(super) const fn shape(&self) -> &Shape {
        &self.shape
    }

    /// Records the new shape; nothing if it did not change.
    pub(super) fn commit(self, document: &mut Document) {
        document.apply(Command::Reshape {
            id: self.id,
            shape: self.shape,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::drag::testing::document;
    use super::*;
    use crate::model::{Line, Rectangle, Style, Text};

    fn rectangle(a: Point, b: Point) -> Shape {
        Shape::Rectangle(Rectangle {
            rect: Rect::from_corners(a, b),
        })
    }

    #[test]
    fn dragging_a_corner_keeps_the_opposite_corner() {
        let shape = rectangle(Point::new(10.0, 10.0), Point::new(50.0, 30.0));
        // Top-right corner (index 1) dragged past the left edge: the rectangle
        // flips around the fixed bottom-left corner.
        assert_eq!(
            reshaped(&shape, Handle::Corner(1), Point::new(0.0, 0.0), false),
            rectangle(Point::new(0.0, 0.0), Point::new(10.0, 30.0))
        );
        assert_eq!(
            reshaped(&shape, Handle::Corner(2), Point::new(70.0, 40.0), true),
            rectangle(Point::new(10.0, 10.0), Point::new(70.0, 70.0))
        );
    }

    #[test]
    fn an_ellipse_reshapes_by_its_bounding_box_corners() {
        let rect = Rect::from_corners(Point::new(10.0, 10.0), Point::new(50.0, 30.0));
        let shape = Shape::Ellipse(Ellipse { rect });
        assert_eq!(
            handles(&shape),
            handles(&Shape::Rectangle(Rectangle { rect })),
        );
        // Shift makes it a circle, the opposite corner fixed.
        assert_eq!(
            reshaped(&shape, Handle::Corner(0), Point::new(-10.0, 0.0), true),
            Shape::Ellipse(Ellipse {
                rect: Rect::from_corners(Point::new(-10.0, -30.0), Point::new(50.0, 30.0)),
            })
        );
    }

    #[test]
    fn dragging_an_end_keeps_the_other_end() {
        let shape = Shape::Line(Line {
            start: Point::new(0.0, 0.0),
            end: Point::new(10.0, 0.0),
        });
        assert_eq!(
            reshaped(&shape, Handle::Start, Point::new(-5.0, 20.0), false),
            Shape::Line(Line {
                start: Point::new(-5.0, 20.0),
                end: Point::new(10.0, 0.0),
            })
        );
        assert_eq!(
            reshaped(&shape, Handle::End, Point::new(30.0, 2.0), true),
            Shape::Line(Line {
                start: Point::new(0.0, 0.0),
                end: Point::new(30.0, 0.0),
            })
        );
    }

    #[test]
    fn handles_belong_to_a_lone_selection_and_the_nearest_wins() {
        let mut document = document();
        let line = document.add(
            Shape::Line(Line {
                start: Point::new(0.0, 0.0),
                end: Point::new(10.0, 0.0),
            }),
            Style::default(),
        );
        let text = document.add(
            Shape::Text(Text::new(Point::new(100.0, 100.0), "t")),
            Style::default(),
        );
        assert_eq!(
            handle_at(&document, Point::ORIGIN, 7.0),
            None,
            "nothing selected"
        );
        document.set_selection([line]);
        assert_eq!(
            handle_at(&document, Point::new(6.0, 0.0), 7.0),
            Some((line, Handle::End))
        );
        assert_eq!(
            handle_at(&document, Point::new(4.0, 0.0), 7.0),
            Some((line, Handle::Start))
        );
        assert_eq!(
            handle_at(&document, Point::new(5.0, 9.0), 7.0),
            None,
            "out of reach"
        );
        document.set_selection([line, text]);
        assert_eq!(
            handle_at(&document, Point::ORIGIN, 7.0),
            None,
            "two selected"
        );
        document.set_selection([text]);
        assert_eq!(
            handle_at(&document, Point::new(100.0, 100.0), 7.0),
            None,
            "text has none"
        );
    }
}
