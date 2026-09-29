//! The per-display `Canvas` program.

use chartreuse_core::display::DisplayInfo;
use chartreuse_core::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use iced::widget::canvas::{Action, Canvas, Event, Frame, Geometry, Program};
use iced::widget::image::Handle;
use iced::{keyboard, mouse, Color, Element, Fill, Rectangle, Renderer, Size, Theme};

use super::selection::{output_grid, Input, Selection};
use crate::shared::{self, PointerState, Projection, LABEL_TEXT_SIZE};

/// The distance between the size label and the selection, in canvas units.
const LABEL_GAP: f32 = 6.0;

/// The rectangle-selection overlay for one display: the display's frozen
/// capture, a translucent dim over everything but the selection, the selection
/// border in the accent color, and a label with the output size in pixels.
///
/// Build one per overlay window from the shared [`Selection`] in each `view` and
/// show it with [`RectangleOverlay::view`]. It turns pointer, Escape and Space
/// events into [`Input`] in global logical coordinates and publishes them through
/// `on_input`; route them all to the one [`Selection::apply`]. The window that
/// receives the press keeps receiving the drag (the OS grabs the pointer), even
/// over other displays: its positions simply map outside its own display.
///
/// Only the primary (left) button selects. Escape and Space are heard by the
/// focused overlay window, so give an overlay focus when opening them. Space
/// repeats while held are not published.
///
/// As a [`Program`] it draws only what goes over the capture:
/// [`RectangleOverlay::view`] puts the capture in a layer of its own underneath
/// the canvas.
pub struct RectangleOverlay<'a, F> {
    selection: &'a Selection,
    display: &'a DisplayInfo,
    image: &'a Handle,
    accent: Color,
    size_label: bool,
    on_input: F,
}

impl<F> std::fmt::Debug for RectangleOverlay<'_, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RectangleOverlay")
            .field("selection", self.selection)
            .field("display", &self.display.id)
            .field("accent", &self.accent)
            .field("size_label", &self.size_label)
            .finish_non_exhaustive()
    }
}

impl<'a, F> RectangleOverlay<'a, F> {
    /// The overlay for `display`, showing `image` (its capture, from
    /// [`frozen_image`](crate::shared::frozen_image)) under `selection`, with the
    /// size label shown.
    pub const fn new(
        selection: &'a Selection,
        display: &'a DisplayInfo,
        image: &'a Handle,
        accent: Color,
        on_input: F,
    ) -> Self {
        Self {
            selection,
            display,
            image,
            accent,
            size_label: true,
            on_input,
        }
    }

    /// Shows or hides the output-size label.
    #[must_use]
    pub const fn size_label(mut self, show: bool) -> Self {
        self.size_label = show;
        self
    }

    /// The frozen capture stretched over the whole window, with the selection
    /// canvas on top.
    pub fn view<Message>(self) -> Element<'a, Message>
    where
        F: Fn(Input) -> Message + 'a,
        Message: 'a,
    {
        let image = self.image;
        shared::over_capture(image, Canvas::new(self).width(Fill).height(Fill))
    }
}

impl<Message, F> Program<Message> for RectangleOverlay<'_, F>
where
    F: Fn(Input) -> Message,
{
    type State = PointerState;

    fn update(
        &self,
        state: &mut PointerState,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        let projection = Projection::new(self.display, bounds);
        let input = match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let position = cursor.position().filter(|p| bounds.contains(*p))?;
                state.last = Some(position);
                state.pressed = true;
                Input::Press(projection.to_global(position))
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                state.last = Some(*position);
                if !(state.pressed || self.selection.is_active()) {
                    return None;
                }
                Input::Move(projection.to_global(*position))
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if !(std::mem::take(&mut state.pressed) || self.selection.is_active()) {
                    return None;
                }
                let point = cursor
                    .position()
                    .or(state.last)
                    .map(|position| projection.to_global(position))
                    .or_else(|| self.selection.pointer())?;
                Input::Release(point)
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => {
                state.pressed = false;
                Input::Escape
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Space),
                repeat: false,
                ..
            }) => Input::Space(
                cursor
                    .position()
                    .filter(|p| bounds.contains(*p))
                    .map(|position| projection.to_global(position)),
            ),
            _ => return None,
        };
        Some(Action::publish((self.on_input)(input)).and_capture())
    }

    fn draw(
        &self,
        _state: &PointerState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let projection = Projection::new(self.display, bounds);
        let area = Rectangle::with_size(bounds.size());
        let mut frame = Frame::new(renderer, bounds.size());

        let rect = self.selection.rect();
        let hole = rect.map(|rect| projection.rect_to_frame(&rect));
        shared::fill_dim(&mut frame, area, hole);
        if let Some(hole) = hole {
            shared::border_around(&mut frame, hole, self.accent);
        }
        if self.size_label
            && let Some(rect) = rect
        {
            self.draw_size_label(&mut frame, &projection, &rect);
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &PointerState,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        mouse::Interaction::Crosshair
    }
}

impl<F> RectangleOverlay<'_, F> {
    /// Draws "W × H" (output pixels) by the selection's bottom-right corner.
    ///
    /// The label is placed in global coordinates, on the display holding that
    /// corner, so every display's canvas draws its part of the same label.
    fn draw_size_label(&self, frame: &mut Frame, projection: &Projection, rect: &LogicalRect) {
        let layout = self.selection.layout();
        let Some(grid) = output_grid(layout, rect) else {
            return;
        };
        let size = grid.pixel_size();
        let content = format!("{} × {}", size.width, size.height);
        // An estimate: iced's canvas has no text measurement, and digits are
        // about 0.6 em wide in the default fonts.
        let text_width = content.chars().count() as f32 * LABEL_TEXT_SIZE * 0.6;
        let pill = shared::pill_size(Size::new(text_width, LABEL_TEXT_SIZE * 1.3));
        let corner = LogicalPoint::new(rect.max_x(), rect.max_y());
        let area = layout.nearest_display(corner).logical_bounds;
        let gap = projection
            .size_to_logical(Size::new(LABEL_GAP, LABEL_GAP))
            .height;
        let origin = label_origin(rect, projection.size_to_logical(pill), &area, gap);
        shared::draw_pill(
            frame,
            projection.to_frame(origin),
            pill,
            content,
            self.accent,
        );
    }
}

/// Where to put a label of `label` size for `selection`, within `area` (the
/// display under the selection's bottom-right corner): right-aligned with the
/// selection, `gap` below it, or inside its bottom edge when there is no room
/// below, and always kept on `area`.
fn label_origin(
    selection: &LogicalRect,
    label: LogicalSize,
    area: &LogicalRect,
    gap: f64,
) -> LogicalPoint {
    let fit = |value: f64, min: f64, max: f64| value.min(max).max(min);
    let x = fit(
        selection.max_x() - label.width,
        area.min_x(),
        area.max_x() - label.width,
    );
    let below = selection.max_y() + gap;
    let y = if below + label.height <= area.max_y() {
        below
    } else {
        selection.max_y() - gap - label.height
    };
    let y = fit(y, area.min_y(), area.max_y() - label.height);
    LogicalPoint::new(x, y)
}

#[cfg(test)]
mod tests {
    use chartreuse_core::display::{DisplayId, DisplayLayout};
    use chartreuse_platform::fake::default_displays;
    use iced::keyboard::key::{Named, Physical};
    use iced::keyboard::{Location, Modifiers};
    use iced::Point;

    use super::*;
    use crate::rectangle::Phase;

    fn layout() -> DisplayLayout {
        DisplayLayout::new(default_displays()).expect("the fake displays form a layout")
    }

    fn display(id: u64) -> DisplayInfo {
        layout()
            .get(DisplayId(id))
            .cloned()
            .expect("a fake display")
    }

    /// A canvas filling a window sized to the display, as in an overlay.
    fn overlay_bounds(display: &DisplayInfo) -> Rectangle {
        let size = display.logical_bounds.size;
        Rectangle::new(
            Point::ORIGIN,
            Size::new(size.width as f32, size.height as f32),
        )
    }

    fn pt(x: f64, y: f64) -> LogicalPoint {
        LogicalPoint::new(x, y)
    }

    #[test]
    fn the_label_sits_below_the_selection_and_stays_on_its_display() {
        let area = LogicalRect::new(0.0, 0.0, 1000.0, 800.0);
        let label = LogicalSize::new(100.0, 20.0);
        // Room below: right-aligned, under the bottom edge.
        let rect = LogicalRect::new(200.0, 100.0, 300.0, 200.0);
        assert_eq!(label_origin(&rect, label, &area, 6.0), pt(400.0, 306.0));
        // At the bottom of the display: inside the selection's bottom edge.
        let rect = LogicalRect::new(200.0, 600.0, 300.0, 200.0);
        assert_eq!(label_origin(&rect, label, &area, 6.0), pt(400.0, 774.0));
        // Narrower than the label at the left edge: pinned to the display.
        let rect = LogicalRect::new(0.0, 100.0, 40.0, 40.0);
        assert_eq!(label_origin(&rect, label, &area, 6.0), pt(0.0, 146.0));
    }

    #[derive(Debug, Clone, PartialEq)]
    struct Received(Input);

    /// Runs `event` through the overlay for `display_id`, returning the published
    /// input.
    fn send(
        selection: &Selection,
        display_id: u64,
        state: &mut PointerState,
        event: &Event,
        cursor: Option<Point>,
    ) -> Option<Input> {
        let display = display(display_id);
        let handle = Handle::from_rgba(1, 1, vec![0; 4]);
        let overlay = RectangleOverlay::new(selection, &display, &handle, Color::WHITE, Received);
        let cursor = cursor.map_or(mouse::Cursor::Unavailable, mouse::Cursor::Available);
        let action =
            Program::<Received>::update(&overlay, state, event, overlay_bounds(&display), cursor)?;
        let (message, _, status) = action.into_inner();
        assert_eq!(status, iced::event::Status::Captured);
        message.map(|Received(input)| input)
    }

    fn press() -> Event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
    }

    fn release() -> Event {
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
    }

    fn moved(x: f32, y: f32) -> Event {
        Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(x, y),
        })
    }

    #[test]
    fn a_drag_from_one_window_drives_the_shared_selection_across_displays() {
        // Press on the external display, drag right past its window onto the
        // primary, release there. Every event reaches the external window only.
        let mut selection = Selection::new(layout());
        let mut state = PointerState::default();
        let press_at = Point::new(1800.0, 300.0);
        let input = send(&selection, 2, &mut state, &press(), Some(press_at));
        assert_eq!(input, Some(Input::Press(pt(-120.0, -100.0))));
        selection.apply(input.expect("pressed"));

        let input = send(&selection, 2, &mut state, &moved(2120.0, 700.0), None);
        assert_eq!(input, Some(Input::Move(pt(200.0, 300.0))));
        selection.apply(input.expect("moved"));

        let release_at = Point::new(2120.0, 700.0);
        let input = send(&selection, 2, &mut state, &release(), Some(release_at));
        assert_eq!(input, Some(Input::Release(pt(200.0, 300.0))));
        selection.apply(input.expect("released"));
        assert_eq!(
            selection.phase(),
            Phase::Committed(LogicalRect::new(-120.0, -100.0, 320.0, 400.0))
        );
    }

    #[test]
    fn a_release_in_the_same_batch_as_its_press_is_not_lost() {
        // The selection is still idle (the app has not seen the press yet), but
        // this canvas saw the press, so it reports the release.
        let selection = Selection::new(layout());
        let mut state = PointerState::default();
        let at = Some(Point::new(50.0, 60.0));
        assert!(send(&selection, 1, &mut state, &press(), at).is_some());
        assert_eq!(
            send(&selection, 1, &mut state, &release(), at),
            Some(Input::Release(pt(50.0, 60.0)))
        );
    }

    #[test]
    fn a_release_without_a_cursor_uses_the_last_seen_position() {
        let mut selection = Selection::new(layout());
        let mut state = PointerState::default();
        let at = Some(Point::new(50.0, 60.0));
        selection.apply(send(&selection, 1, &mut state, &press(), at).expect("pressed"));
        let _ = send(&selection, 1, &mut state, &moved(400.0, 300.0), None);
        assert_eq!(
            send(&selection, 1, &mut state, &release(), None),
            Some(Input::Release(pt(400.0, 300.0)))
        );
    }

    #[test]
    fn idle_hover_and_presses_elsewhere_publish_nothing() {
        let selection = Selection::new(layout());
        let mut state = PointerState::default();
        assert_eq!(
            send(&selection, 1, &mut state, &moved(10.0, 10.0), None),
            None
        );
        // A press outside this canvas belongs to another window.
        let outside = Some(Point::new(-5.0, 10.0));
        assert_eq!(send(&selection, 1, &mut state, &press(), outside), None);
        assert_eq!(send(&selection, 1, &mut state, &release(), outside), None);
        let right = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right));
        assert_eq!(
            send(
                &selection,
                1,
                &mut state,
                &right,
                Some(Point::new(5.0, 5.0))
            ),
            None
        );
    }

    #[test]
    fn escape_is_published() {
        let selection = Selection::new(layout());
        let mut state = PointerState::default();
        let escape = Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(Named::Escape),
            modified_key: keyboard::Key::Named(Named::Escape),
            physical_key: Physical::Code(keyboard::key::Code::Escape),
            location: Location::Standard,
            modifiers: Modifiers::default(),
            text: None,
            repeat: false,
        });
        assert_eq!(
            send(&selection, 3, &mut state, &escape, None),
            Some(Input::Escape)
        );
    }

    fn space(repeat: bool) -> Event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(Named::Space),
            modified_key: keyboard::Key::Named(Named::Space),
            physical_key: Physical::Code(keyboard::key::Code::Space),
            location: Location::Standard,
            modifiers: Modifiers::default(),
            text: Some(" ".into()),
            repeat,
        })
    }

    #[test]
    fn space_is_published_with_the_pointer_over_this_display() {
        let selection = Selection::new(layout());
        let mut state = PointerState::default();
        let over = Some(Point::new(50.0, 60.0));
        assert_eq!(
            send(&selection, 1, &mut state, &space(false), over),
            Some(Input::Space(Some(pt(50.0, 60.0))))
        );
        let outside = Some(Point::new(-5.0, 10.0));
        assert_eq!(
            send(&selection, 1, &mut state, &space(false), outside),
            Some(Input::Space(None))
        );
        // Holding Space down publishes it once.
        assert_eq!(send(&selection, 1, &mut state, &space(true), over), None);
    }
}
