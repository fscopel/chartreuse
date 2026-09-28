//! The select tool.

use iced::mouse::Interaction;

use super::handles::{handle_at, Reshape};
use super::text::{text_at, TextEdit};
use super::{Context, Pointer, Preview, Tool, ToolKind};
use crate::model::{AnnotationId, Command, Document, Point, Vector};

/// The select tool: pick, move, and reshape annotations, and double-click
/// text to edit it.
///
/// - A click selects the topmost annotation under the pointer
///   ([`Document::annotation_at`]) and nothing else; a click on nothing,
///   including anywhere outside the crop, clears the selection. Shift-click
///   adds or removes one annotation.
/// - Dragging a selected annotation moves the whole selection (dragging an
///   unselected one selects it first). The move is previewed and recorded as
///   one [`Command::Translate`] on release.
/// - Dragging a [handle](super::Handle) of a lone selected annotation
///   reshapes it as one [`Command::Reshape`], keeping the grab offset; Shift
///   snaps a line's or arrow's end to 45° or makes a rectangle square.
/// - Double-clicking text edits it in place (see [`TextTool`](super::TextTool));
///   a click elsewhere commits the edit and then acts as a normal click.
#[derive(Debug, Default)]
pub struct SelectTool {
    state: State,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Idle,
    /// Pressed on a selected annotation; a drag from here moves `ids`.
    /// Released without dragging, the selection becomes just `only` (a plain
    /// click on one member of a multiple selection).
    Pressed {
        from: Point,
        ids: Vec<AnnotationId>,
        only: Option<AnnotationId>,
    },
    Moving {
        from: Point,
        delta: Vector,
        ids: Vec<AnnotationId>,
    },
    Reshaping(Reshape),
    Editing(TextEdit),
}

impl SelectTool {
    fn press(&mut self, at: Point, clicks: u8, cx: &mut Context<'_>) {
        if clicks >= 2
            && let Some(id) = text_at(cx.document, at, cx.tolerance())
            && let Some(edit) = TextEdit::existing(cx.document, id)
        {
            cx.document.set_selection([id]);
            self.state = State::Editing(edit);
            return;
        }
        if let Some((id, handle)) = handle_at(cx.document, at, cx.handle_reach())
            && let Some(reshape) = Reshape::start(cx.document, id, handle, at)
        {
            self.state = State::Reshaping(reshape);
            return;
        }
        let Some(id) = cx.document.annotation_at(at, cx.tolerance()) else {
            if !cx.shift {
                cx.document.clear_selection();
            }
            return;
        };
        let only = if cx.shift {
            if cx.document.deselect(id) {
                return;
            }
            cx.document.select(id);
            None
        } else if cx.document.is_selected(id) {
            Some(id)
        } else {
            cx.document.set_selection([id]);
            None
        };
        self.state = State::Pressed {
            from: at,
            ids: cx.document.selection().iter().copied().collect(),
            only,
        };
    }

    fn drag_to(&mut self, at: Point, cx: &Context<'_>) {
        match &mut self.state {
            State::Pressed { from, ids, .. } if at.distance(*from) > cx.drag_threshold() => {
                self.state = State::Moving {
                    from: *from,
                    delta: at - *from,
                    ids: std::mem::take(ids),
                };
            }
            State::Moving { from, delta, .. } => *delta = at - *from,
            State::Reshaping(reshape) => reshape.drag_to(at, cx.shift, cx.drag_threshold()),
            _ => {}
        }
    }
}

impl Tool for SelectTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Select
    }

    fn pointer(&mut self, pointer: Pointer, cx: &mut Context<'_>) {
        match pointer {
            Pointer::Press { at, clicks } => {
                self.finish(cx);
                self.press(at, clicks, cx);
            }
            Pointer::Move { at } => self.drag_to(at, cx),
            Pointer::Release { at } => {
                self.drag_to(at, cx);
                if let State::Pressed { only: Some(id), .. } = self.state {
                    cx.document.set_selection([id]);
                    self.state = State::Idle;
                }
                if !matches!(self.state, State::Editing(_)) {
                    self.finish(cx);
                }
            }
        }
    }

    fn escape(&mut self, cx: &mut Context<'_>) -> bool {
        match std::mem::take(&mut self.state) {
            State::Idle => false,
            State::Editing(edit) => {
                edit.commit(cx.document);
                true
            }
            State::Pressed { .. } | State::Moving { .. } | State::Reshaping(_) => true,
        }
    }

    fn finish(&mut self, cx: &mut Context<'_>) {
        match std::mem::take(&mut self.state) {
            State::Idle | State::Pressed { .. } => {}
            State::Moving { delta, ids, .. } => {
                cx.document.apply(Command::Translate { ids, delta });
            }
            State::Reshaping(reshape) => reshape.commit(cx.document),
            State::Editing(edit) => {
                edit.commit(cx.document);
            }
        }
    }

    fn is_active(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    fn preview(&self) -> Preview<'_> {
        match &self.state {
            State::Idle | State::Pressed { .. } => Preview::None,
            State::Moving { delta, ids, .. } => Preview::Moved(ids, *delta),
            State::Reshaping(reshape) => Preview::Reshaped(reshape.id(), reshape.shape()),
            State::Editing(edit) => Preview::Text(edit),
        }
    }

    fn text_edit(&mut self) -> Option<&mut TextEdit> {
        match &mut self.state {
            State::Editing(edit) => Some(edit),
            _ => None,
        }
    }

    fn cursor(&self, document: &Document, at: Point, pixel: f32) -> Interaction {
        match &self.state {
            State::Moving { .. } => Interaction::Grabbing,
            State::Reshaping(_) => Interaction::Crosshair,
            State::Editing(edit)
                if text_at(document, at, super::HIT_TOLERANCE * pixel)
                    == match edit.target() {
                        super::text::TextTarget::Existing(id) => Some(id),
                        super::text::TextTarget::New => None,
                    } =>
            {
                Interaction::Text
            }
            _ if handle_at(document, at, super::handles::HANDLE_REACH * pixel).is_some() => {
                Interaction::Crosshair
            }
            _ if document
                .annotation_at(at, super::HIT_TOLERANCE * pixel)
                .is_some() =>
            {
                Interaction::Grab
            }
            _ => Interaction::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::drag::testing::{document, send};
    use super::super::text::TextInput;
    use super::*;
    use crate::model::{Line, Shape, Style, Text};

    fn line(document: &mut Document, y: f32) -> AnnotationId {
        document.add(
            Shape::Line(Line {
                start: Point::new(10.0, y),
                end: Point::new(110.0, y),
            }),
            Style::default(),
        )
    }

    fn click(tool: &mut SelectTool, document: &mut Document, at: Point, shift: bool) {
        send(tool, document, shift, Pointer::Press { at, clicks: 1 });
        send(tool, document, shift, Pointer::Release { at });
    }

    fn drag(tool: &mut SelectTool, document: &mut Document, from: Point, to: Point) {
        send(
            tool,
            document,
            false,
            Pointer::Press {
                at: from,
                clicks: 1,
            },
        );
        send(tool, document, false, Pointer::Move { at: to });
        send(tool, document, false, Pointer::Release { at: to });
    }

    fn start_of(document: &Document, id: AnnotationId) -> Point {
        match &document.get(id).unwrap().shape {
            Shape::Line(line) => line.start,
            other => panic!("not a line: {other:?}"),
        }
    }

    #[test]
    fn a_click_selects_the_topmost_annotation_only() {
        let mut document = document();
        let bottom = line(&mut document, 50.0);
        let top = line(&mut document, 50.0);
        let other = line(&mut document, 90.0);
        let mut tool = SelectTool::default();
        document.set_selection([bottom, other]);

        click(&mut tool, &mut document, Point::new(60.0, 51.0), false);
        assert_eq!(
            document.selection().iter().copied().collect::<Vec<_>>(),
            [top]
        );
        click(&mut tool, &mut document, Point::new(60.0, 200.0), false);
        assert!(document.selection().is_empty(), "a click on nothing clears");
        assert!(!document.can_redo() && document.annotations().len() == 3);
    }

    #[test]
    fn shift_click_toggles_one_annotation() {
        let mut document = document();
        let a = line(&mut document, 50.0);
        let b = line(&mut document, 90.0);
        let mut tool = SelectTool::default();
        click(&mut tool, &mut document, Point::new(60.0, 50.0), false);
        click(&mut tool, &mut document, Point::new(60.0, 90.0), true);
        assert!(document.is_selected(a) && document.is_selected(b));
        click(&mut tool, &mut document, Point::new(60.0, 50.0), true);
        assert!(!document.is_selected(a) && document.is_selected(b));
        click(&mut tool, &mut document, Point::new(60.0, 200.0), true);
        assert!(
            document.is_selected(b),
            "shift-click on nothing keeps the selection"
        );
    }

    #[test]
    fn dragging_moves_the_selection_as_one_undo_step() {
        let mut document = document();
        let a = line(&mut document, 50.0);
        let b = line(&mut document, 90.0);
        let c = line(&mut document, 130.0);
        let mut tool = SelectTool::default();
        document.set_selection([a, b]);

        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Press {
                at: Point::new(60.0, 90.0),
                clicks: 1,
            },
        );
        for step in 1..=5 {
            let at = Point::new(60.0 + 4.0 * step as f32, 90.0 + 2.0 * step as f32);
            send(&mut tool, &mut document, false, Pointer::Move { at });
        }
        assert_eq!(
            tool.preview(),
            Preview::Moved(&[a, b], Vector::new(20.0, 10.0))
        );
        assert_eq!(
            start_of(&document, a),
            Point::new(10.0, 50.0),
            "only previewed"
        );
        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Release {
                at: Point::new(80.0, 100.0),
            },
        );

        assert_eq!(start_of(&document, a), Point::new(30.0, 60.0));
        assert_eq!(start_of(&document, b), Point::new(30.0, 100.0));
        assert_eq!(start_of(&document, c), Point::new(10.0, 130.0));
        assert!(document.undo());
        assert_eq!(start_of(&document, a), Point::new(10.0, 50.0));
        assert_eq!(start_of(&document, b), Point::new(10.0, 90.0));
        assert!(document.undo());
        assert_eq!(
            document.annotations().len(),
            2,
            "the step before is the last add"
        );
    }

    #[test]
    fn dragging_an_unselected_annotation_selects_and_moves_only_it() {
        let mut document = document();
        let a = line(&mut document, 50.0);
        let b = line(&mut document, 90.0);
        let mut tool = SelectTool::default();
        document.set_selection([a]);
        drag(
            &mut tool,
            &mut document,
            Point::new(60.0, 90.0),
            Point::new(60.0, 110.0),
        );
        assert_eq!(
            document.selection().iter().copied().collect::<Vec<_>>(),
            [b]
        );
        assert_eq!(start_of(&document, a), Point::new(10.0, 50.0));
        assert_eq!(start_of(&document, b), Point::new(10.0, 110.0));
    }

    #[test]
    fn clicking_one_of_several_selected_selects_just_it_but_dragging_keeps_all() {
        let mut document = document();
        let a = line(&mut document, 50.0);
        let b = line(&mut document, 90.0);
        let mut tool = SelectTool::default();
        document.set_selection([a, b]);
        drag(
            &mut tool,
            &mut document,
            Point::new(60.0, 50.0),
            Point::new(60.0, 40.0),
        );
        assert!(document.is_selected(a) && document.is_selected(b));
        click(&mut tool, &mut document, Point::new(60.0, 40.0), false);
        assert_eq!(
            document.selection().iter().copied().collect::<Vec<_>>(),
            [a]
        );
    }

    #[test]
    fn a_press_without_a_drag_or_an_escaped_drag_records_nothing() {
        let mut document = document();
        let a = line(&mut document, 50.0);
        let mut tool = SelectTool::default();
        drag(
            &mut tool,
            &mut document,
            Point::new(60.0, 50.0),
            Point::new(61.0, 51.0),
        );
        assert_eq!(
            start_of(&document, a),
            Point::new(10.0, 50.0),
            "within the threshold"
        );

        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Press {
                at: Point::new(60.0, 50.0),
                clicks: 1,
            },
        );
        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Move {
                at: Point::new(90.0, 90.0),
            },
        );
        let mut cx = Context {
            document: &mut document,
            style: Style::default(),
            pixel: 1.0,
            shift: false,
        };
        assert!(tool.escape(&mut cx));
        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Release {
                at: Point::new(90.0, 90.0),
            },
        );
        assert_eq!(start_of(&document, a), Point::new(10.0, 50.0));
        assert_eq!(document.annotations().len(), 1);
        document.undo();
        assert!(
            document.annotations().is_empty(),
            "only the add was recorded"
        );
    }

    #[test]
    fn dragging_a_handle_reshapes_the_lone_selection() {
        let mut document = document();
        let a = line(&mut document, 50.0);
        let mut tool = SelectTool::default();
        document.set_selection([a]);
        // Grabbed 2 right and 1 below the start handle: the start keeps that
        // offset from the pointer instead of jumping to it.
        drag(
            &mut tool,
            &mut document,
            Point::new(12.0, 51.0),
            Point::new(0.0, 0.0),
        );
        assert_eq!(
            document.get(a).unwrap().shape,
            Shape::Line(Line {
                start: Point::new(-2.0, -1.0),
                end: Point::new(110.0, 50.0),
            })
        );
        assert!(document.undo());
        assert_eq!(start_of(&document, a), Point::new(10.0, 50.0));
    }

    #[test]
    fn double_clicking_text_edits_it_and_clicking_away_commits() {
        let mut document = document();
        let text = document.add(
            Shape::Text(Text::new(Point::new(50.0, 50.0), "Hi")),
            Style::default(),
        );
        let mut tool = SelectTool::default();
        let on_text = Point::new(55.0, 60.0);
        click(&mut tool, &mut document, on_text, false);
        assert!(tool.text_edit().is_none(), "a single click only selects");
        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Press {
                at: on_text,
                clicks: 2,
            },
        );
        send(
            &mut tool,
            &mut document,
            false,
            Pointer::Release { at: on_text },
        );
        let edit = tool.text_edit().expect("editing");
        edit.input(TextInput::Insert("!".to_owned()));
        assert!(matches!(tool.preview(), Preview::Text(edit) if edit.content() == "Hi!"));

        click(&mut tool, &mut document, Point::new(300.0, 250.0), false);
        assert!(tool.text_edit().is_none());
        assert_eq!(
            document.get(text).unwrap().shape,
            Shape::Text(Text::new(Point::new(50.0, 50.0), "Hi!"))
        );
        assert!(
            document.selection().is_empty(),
            "the click then acted normally"
        );
        assert!(document.undo());
        assert!(matches!(&document.get(text).unwrap().shape, Shape::Text(t) if t.content == "Hi"));
    }
}
