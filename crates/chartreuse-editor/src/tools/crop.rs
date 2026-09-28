//! The crop tool: frames the part of the image to export.

use iced::mouse::Interaction;

use super::rectangle::square;
use super::{Context, Pointer, Preview, Tool, ToolKind, HANDLE_REACH};
use crate::model::{Command, Document, Point, Rect, Vector};

/// The crop tool. While it is active the canvas shows the whole image, with
/// the area outside the crop being edited dimmed; the crop starts as the
/// document's.
///
/// - Dragging outside the crop (or anywhere, with none) draws a new one;
///   Shift makes it square.
/// - Dragging a corner resizes it, the opposite corner fixed; dragging inside
///   it moves it, within the image.
/// - Enter, or double-clicking inside it, applies it ([`Command::SetCrop`],
///   one undo step) and is done, so the editor returns to the select tool
///   and shows the cropped image. Switching tools applies it too.
/// - Escape abandons a drag in progress; otherwise it cancels the whole edit,
///   leaving the document's crop as it was, and is done.
///
/// The crop always lies on whole pixels within the image: its edges are
/// rounded to the nearest pixel and it is clipped to the image. A crop that
/// would be empty (drawn entirely outside the image) leaves the previous one.
#[derive(Debug)]
pub struct CropTool {
    /// The crop being edited (`None`: the whole image).
    crop: Option<Rect>,
    gesture: Option<Gesture>,
    done: bool,
}

/// A drag of the crop, with the crop as it was when the drag started.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Gesture {
    kind: GestureKind,
    from: Point,
    original: Option<Rect>,
    /// Whether the pointer has left the drag threshold around `from`.
    moved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum GestureKind {
    /// Drawing a new crop from `from`.
    Draw,
    /// Dragging corner `index` of [`Rect::corners`]; `grab` is from the
    /// pointer to the corner when grabbed.
    Corner { index: usize, grab: Vector },
    /// Moving the whole crop.
    Move,
}

impl CropTool {
    /// The crop tool for `document`, editing its crop.
    #[must_use]
    pub fn new(document: &Document) -> Self {
        Self {
            crop: document.crop(),
            gesture: None,
            done: false,
        }
    }

    /// The crop as edited so far (`None`: the whole image).
    #[must_use]
    pub const fn crop(&self) -> Option<Rect> {
        self.crop
    }

    fn press(&mut self, at: Point, clicks: u8, cx: &mut Context<'_>) {
        let inside = self.crop.is_some_and(|crop| crop.contains(at));
        if clicks >= 2 && inside {
            self.apply(cx);
            return;
        }
        let corner = self.crop.and_then(|crop| {
            let reach = HANDLE_REACH * cx.pixel;
            crop.corners()
                .into_iter()
                .enumerate()
                .map(|(index, corner)| (index, corner, corner.distance(at)))
                .filter(|&(_, _, distance)| distance <= reach)
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .map(|(index, corner, _)| GestureKind::Corner {
                    index,
                    grab: corner - at,
                })
        });
        let kind = match corner {
            Some(corner) => corner,
            None if inside => GestureKind::Move,
            None => GestureKind::Draw,
        };
        self.gesture = Some(Gesture {
            kind,
            from: at,
            original: self.crop,
            moved: false,
        });
    }

    fn drag_to(&mut self, at: Point, cx: &Context<'_>) {
        let Some(gesture) = &mut self.gesture else {
            return;
        };
        gesture.moved |= at.distance(gesture.from) > cx.drag_threshold();
        if !gesture.moved {
            return;
        }
        let image = cx.document.bounds();
        let dragged = match (gesture.kind, gesture.original) {
            (GestureKind::Draw, _) => {
                let end = if cx.shift {
                    square(gesture.from, at)
                } else {
                    at
                };
                snapped(Rect::from_corners(gesture.from, end), image)
            }
            (GestureKind::Corner { index, grab }, Some(original)) => {
                let opposite = original.corners()[(index + 2) % 4];
                let to = at + grab;
                let to = if cx.shift { square(opposite, to) } else { to };
                snapped(Rect::from_corners(opposite, to), image)
            }
            (GestureKind::Move, Some(original)) => {
                Some(moved_within(original, at - gesture.from, image))
            }
            (GestureKind::Corner { .. } | GestureKind::Move, None) => None,
        };
        if let Some(crop) = dragged {
            self.crop = Some(crop);
        }
    }

    /// Records the crop, finishing any drag, and is done.
    fn apply(&mut self, cx: &mut Context<'_>) {
        self.gesture = None;
        cx.document.apply(Command::SetCrop(self.crop));
        self.done = true;
    }
}

/// `rect` on whole pixels (edges rounded to the nearest) within `image`, or
/// `None` if that leaves nothing.
fn snapped(rect: Rect, image: Rect) -> Option<Rect> {
    let pixels = rect.pixels()?.intersection(&image.pixels()?)?;
    Some(Rect::from_pixels(pixels))
}

/// `rect` (on whole pixels within `image`) moved by `delta`, rounded to
/// whole pixels, as far as it goes without leaving `image`.
fn moved_within(rect: Rect, delta: Vector, image: Rect) -> Rect {
    let axis = |delta: f32, min: f32, max: f32, lower: f32, upper: f32| {
        delta
            .round()
            .clamp(lower - min, (upper - max).max(lower - min))
    };
    let (min, max) = (rect.min(), rect.max());
    rect.translate(Vector::new(
        axis(delta.x, min.x, max.x, image.min().x, image.max().x),
        axis(delta.y, min.y, max.y, image.min().y, image.max().y),
    ))
}

impl Tool for CropTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Crop
    }

    fn pointer(&mut self, pointer: Pointer, cx: &mut Context<'_>) {
        match pointer {
            Pointer::Press { at, clicks } => self.press(at, clicks, cx),
            Pointer::Move { at } => self.drag_to(at, cx),
            Pointer::Release { at } => {
                self.drag_to(at, cx);
                self.gesture = None;
            }
        }
    }

    fn escape(&mut self, cx: &mut Context<'_>) -> bool {
        match self.gesture.take() {
            Some(gesture) => self.crop = gesture.original,
            None => {
                self.crop = cx.document.crop();
                self.done = true;
            }
        }
        true
    }

    fn confirm(&mut self, cx: &mut Context<'_>) {
        self.apply(cx);
    }

    fn finish(&mut self, cx: &mut Context<'_>) {
        self.gesture = None;
        cx.document.apply(Command::SetCrop(self.crop));
    }

    fn is_active(&self) -> bool {
        self.gesture.is_some()
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn preview(&self) -> Preview<'_> {
        Preview::Crop(self.crop)
    }

    fn cursor(&self, _document: &Document, at: Point, pixel: f32) -> Interaction {
        let near_corner = |crop: Rect| {
            crop.corners()
                .into_iter()
                .any(|corner| corner.distance(at) <= HANDLE_REACH * pixel)
        };
        match (self.gesture, self.crop) {
            (
                Some(Gesture {
                    kind: GestureKind::Move,
                    ..
                }),
                _,
            ) => Interaction::Grabbing,
            (None, Some(crop)) if !near_corner(crop) && crop.contains(at) => Interaction::Grab,
            _ => Interaction::Crosshair,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::drag::testing::{document, send};
    use super::*;

    fn rect(ax: f32, ay: f32, bx: f32, by: f32) -> Rect {
        Rect::from_corners(Point::new(ax, ay), Point::new(bx, by))
    }

    fn drag(tool: &mut CropTool, document: &mut Document, from: Point, to: Point, shift: bool) {
        send(
            tool,
            document,
            shift,
            Pointer::Press {
                at: from,
                clicks: 1,
            },
        );
        send(tool, document, shift, Pointer::Move { at: to });
        send(tool, document, shift, Pointer::Release { at: to });
    }

    fn cx(document: &mut Document) -> Context<'_> {
        Context {
            document,
            style: crate::model::Style::default(),
            pixel: 1.0,
            shift: false,
        }
    }

    #[test]
    fn a_drag_frames_a_crop_on_whole_pixels_within_the_image() {
        // The image is 400 × 300.
        let mut document = document();
        let mut tool = CropTool::new(&document);
        let p = Point::new;
        drag(
            &mut tool,
            &mut document,
            p(350.6, 20.4),
            p(450.0, 100.2),
            false,
        );
        assert_eq!(tool.crop(), Some(rect(351.0, 20.0, 400.0, 100.0)));
        assert_eq!(tool.preview(), Preview::Crop(tool.crop()));
        assert_eq!(document.crop(), None, "not applied yet");

        // Shift makes a new one square; one outside the image changes nothing.
        drag(&mut tool, &mut document, p(10.0, 10.0), p(50.0, 30.0), true);
        assert_eq!(tool.crop(), Some(rect(10.0, 10.0, 50.0, 50.0)));
        drag(
            &mut tool,
            &mut document,
            p(-50.0, -50.0),
            p(-10.0, -10.0),
            false,
        );
        assert_eq!(tool.crop(), Some(rect(10.0, 10.0, 50.0, 50.0)));
    }

    #[test]
    fn corners_resize_and_the_inside_moves_within_the_image() {
        let mut document = document();
        let mut tool = CropTool::new(&document);
        let p = Point::new;
        drag(
            &mut tool,
            &mut document,
            p(100.0, 100.0),
            p(200.0, 150.0),
            false,
        );
        // Grabbing the bottom-right corner a little off keeps the offset.
        drag(
            &mut tool,
            &mut document,
            p(203.0, 152.0),
            p(253.0, 202.0),
            false,
        );
        assert_eq!(tool.crop(), Some(rect(100.0, 100.0, 250.0, 200.0)));
        // Moving it far past the top-left stops at the image's edge, keeping
        // its size.
        drag(
            &mut tool,
            &mut document,
            p(150.0, 150.0),
            p(-500.0, 120.4),
            false,
        );
        assert_eq!(tool.crop(), Some(rect(0.0, 70.0, 150.0, 170.0)));
    }

    #[test]
    fn enter_or_a_double_click_inside_applies_as_one_step_and_is_done() {
        let mut document = document();
        let mut tool = CropTool::new(&document);
        let p = Point::new;
        drag(
            &mut tool,
            &mut document,
            p(10.0, 10.0),
            p(110.0, 60.0),
            false,
        );
        tool.confirm(&mut cx(&mut document));
        assert!(tool.is_done());
        assert_eq!(document.crop(), Some(rect(10.0, 10.0, 110.0, 60.0)));

        // A new tool starts from the document's crop; a double-click outside
        // it does nothing, inside it applies.
        let mut tool = CropTool::new(&document);
        assert_eq!(tool.crop(), document.crop());
        drag(
            &mut tool,
            &mut document,
            p(110.0, 60.0),
            p(210.0, 160.0),
            false,
        );
        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Press {
                at: p(300.0, 250.0),
                clicks: 2,
            },
        );
        assert!(!tool.is_done());
        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Press {
                at: p(50.0, 50.0),
                clicks: 2,
            },
        );
        assert!(tool.is_done());
        assert_eq!(document.crop(), Some(rect(10.0, 10.0, 210.0, 160.0)));
        assert!(document.undo());
        assert_eq!(document.crop(), Some(rect(10.0, 10.0, 110.0, 60.0)));
    }

    #[test]
    fn escape_abandons_a_drag_then_cancels_the_edit() {
        let mut document = document();
        let mut tool = CropTool::new(&document);
        let p = Point::new;
        drag(
            &mut tool,
            &mut document,
            p(10.0, 10.0),
            p(110.0, 60.0),
            false,
        );
        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Press {
                at: p(50.0, 30.0),
                clicks: 1,
            },
        );
        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Move { at: p(90.0, 30.0) },
        );
        assert!(tool.escape(&mut cx(&mut document)));
        assert_eq!(
            tool.crop(),
            Some(rect(10.0, 10.0, 110.0, 60.0)),
            "drag undone"
        );
        assert!(!tool.is_done());

        assert!(tool.escape(&mut cx(&mut document)));
        assert!(tool.is_done());
        assert_eq!(tool.crop(), None, "back to the document's");
        tool.finish(&mut cx(&mut document));
        assert!(!document.can_undo(), "nothing recorded");
    }
}
