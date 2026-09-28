//! The blur tool: drag out a region, from one corner to the opposite one,
//! that pixelates or blurs what is beneath it (as the style's
//! [`BlurMode`](crate::model::BlurMode) says). Shift makes it a square.

use super::rectangle::square;
use super::{DragShape, DragTool, ToolKind};
use crate::model::{BlurRegion, Point, Rect, Shape};

/// The blur tool: drag out a region that obscures what is beneath it; Shift
/// makes it a square.
pub type BlurTool = DragTool<Blur>;

/// The blur tool's geometry (see [`DragShape`]).
#[derive(Debug)]
pub struct Blur;

impl DragShape for Blur {
    const KIND: ToolKind = ToolKind::Blur;

    fn shape(start: Point, end: Point, constrain: bool) -> Shape {
        let end = if constrain { square(start, end) } else { end };
        Shape::Blur(BlurRegion {
            rect: Rect::from_corners(start, end),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::drag::testing::{document, drag};
    use super::*;

    #[test]
    fn a_drag_adds_a_region_that_reshapes_by_its_corners() {
        let mut document = document();
        let mut tool = BlurTool::default();
        drag(
            &mut tool,
            &mut document,
            Point::new(50.0, 40.0),
            Point::new(10.0, 10.0),
        );
        let rect = Rect::from_corners(Point::new(10.0, 10.0), Point::new(50.0, 40.0));
        let [region] = document.annotations() else {
            panic!("expected one annotation");
        };
        assert_eq!(region.shape, Shape::Blur(BlurRegion { rect }));
        assert!(document.is_selected(region.id()), "selected, with handles");

        // Dragging its bottom-right corner handle resizes it.
        let id = region.id();
        drag(
            &mut tool,
            &mut document,
            Point::new(50.0, 40.0),
            Point::new(80.0, 60.0),
        );
        assert_eq!(
            document.get(id).unwrap().shape,
            Shape::Blur(BlurRegion {
                rect: Rect::from_corners(Point::new(10.0, 10.0), Point::new(80.0, 60.0)),
            })
        );
        assert!(document.undo());
        assert_eq!(
            document.get(id).unwrap().shape,
            Shape::Blur(BlurRegion { rect })
        );
    }
}
