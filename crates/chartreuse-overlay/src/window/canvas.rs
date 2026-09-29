//! The per-display `Canvas` program.

use chartreuse_core::display::{DisplayInfo, DisplayLayout};
use chartreuse_core::geometry::{LogicalPoint, LogicalRect, LogicalSize};
use chartreuse_core::window::WindowInfo;
use iced::widget::canvas::{Action, Canvas, Event, Frame, Geometry, Program};
use iced::widget::image::Handle;
use iced::{keyboard, mouse, Color, Element, Fill, Rectangle, Renderer, Theme};

use super::selection::{Input, WindowSelection};
use crate::shared::{self, PointerState, Projection};

/// The most characters of a title the label shows; longer ones are cut short
/// with an ellipsis.
const LABEL_MAX_CHARS: usize = 60;

/// The window-selection overlay for one display: the display's frozen capture,
/// a translucent dim over everything but the hovered window, an accent-colored
/// border just inside that window's bounds, and a label naming it.
///
/// Build one per overlay window from the shared [`WindowSelection`] in each
/// `view` and show it with [`WindowOverlay::view`]. It turns pointer, Escape and
/// Space events into [`Input`] in global logical coordinates and publishes them
/// through `on_input`; route them all to the one [`WindowSelection::apply`].
/// Every pointer move is published, so the highlight follows the pointer from
/// window to window and from display to display. A window spanning displays is
/// left undimmed, and bordered, on each of them.
///
/// The undimmed area is the hovered window's whole frame, including any parts
/// that other windows cover: the commit captures the window itself, not what
/// the frozen image shows there.
///
/// A click is a press and release of the primary (left) button on the same
/// canvas; it resolves at the release point. Escape and Space are heard by the
/// focused overlay window, so give an overlay focus when opening them. Space
/// repeats while held are not published.
///
/// As a [`Program`] it draws only what goes over the capture:
/// [`WindowOverlay::view`] puts the capture in a layer of its own underneath the
/// canvas.
pub struct WindowOverlay<'a, F> {
    selection: &'a WindowSelection,
    display: &'a DisplayInfo,
    image: &'a Handle,
    accent: Color,
    label: bool,
    on_input: F,
}

impl<F> std::fmt::Debug for WindowOverlay<'_, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowOverlay")
            .field("selection", self.selection)
            .field("display", &self.display.id)
            .field("accent", &self.accent)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl<'a, F> WindowOverlay<'a, F> {
    /// The overlay for `display`, showing `image` (its capture, from
    /// [`frozen_image`](crate::shared::frozen_image)) under `selection`, with the
    /// label shown.
    pub const fn new(
        selection: &'a WindowSelection,
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
            label: true,
            on_input,
        }
    }

    /// Shows or hides the label naming the hovered window.
    #[must_use]
    pub const fn label(mut self, show: bool) -> Self {
        self.label = show;
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

impl<Message, F> Program<Message> for WindowOverlay<'_, F>
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
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                state.last = Some(*position);
                Input::Move(projection.to_global(*position))
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // Hover follows the press too, in case the pointer has not moved
                // since the overlay opened.
                let position = cursor.position().filter(|p| bounds.contains(*p))?;
                state.last = Some(position);
                state.pressed = true;
                Input::Move(projection.to_global(position))
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if !std::mem::take(&mut state.pressed) {
                    return None;
                }
                let position = cursor.position().or(state.last)?;
                Input::Click(projection.to_global(position))
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

        let hovered = self.selection.hovered();
        let hole = hovered.map(|window| projection.rect_to_frame(&window.bounds));
        shared::fill_dim(&mut frame, area, hole);
        if let Some(hole) = hole {
            shared::border_within(&mut frame, hole, self.accent);
        }
        if self.label
            && let Some(window) = hovered
        {
            self.draw_label(&mut frame, &projection, window);
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &PointerState,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.selection.hovered().is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::Crosshair
        }
    }
}

impl<F> WindowOverlay<'_, F> {
    /// Draws the window's name on a pill centered on its visible part.
    ///
    /// The label is placed in global coordinates, on one display, so every
    /// display's canvas draws its part of the same label.
    fn draw_label(&self, frame: &mut Frame, projection: &Projection, window: &WindowInfo) {
        let Some(content) = label_text(window) else {
            return;
        };
        let pill = shared::pill_size(shared::measure_label(&content));
        let Some(origin) = label_origin(
            self.selection.layout(),
            &window.bounds,
            projection.size_to_logical(pill),
        ) else {
            return;
        };
        shared::draw_pill(
            frame,
            projection.to_frame(origin),
            pill,
            content,
            self.accent,
        );
    }
}

/// What the label calls `window`: its title, or its application's name when it
/// has no title (macOS hides titles without Screen Recording permission), cut
/// to [`LABEL_MAX_CHARS`]. `None` when both are blank.
fn label_text(window: &WindowInfo) -> Option<String> {
    let name = window
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| window.owner.name.trim());
    if name.is_empty() {
        return None;
    }
    if name.chars().count() <= LABEL_MAX_CHARS {
        return Some(name.to_owned());
    }
    let mut cut: String = name.chars().take(LABEL_MAX_CHARS - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    Some(cut)
}

/// Where to put a label of `label` size for a window with `bounds`: centered on
/// the window's largest part on one display, and kept on that display. `None`
/// if the window is on no display.
fn label_origin(
    layout: &DisplayLayout,
    bounds: &LogicalRect,
    label: LogicalSize,
) -> Option<LogicalPoint> {
    let area = |rect: &LogicalRect| rect.size.width * rect.size.height;
    let part = layout
        .intersections(bounds)
        .into_iter()
        .max_by(|a, b| area(&a.logical).total_cmp(&area(&b.logical)))?;
    let (visible, display) = (part.logical, part.display.logical_bounds);
    let fit = |value: f64, min: f64, max: f64| value.min(max).max(min);
    Some(LogicalPoint::new(
        fit(
            visible.min_x() + (visible.size.width - label.width) / 2.0,
            display.min_x(),
            display.max_x() - label.width,
        ),
        fit(
            visible.min_y() + (visible.size.height - label.height) / 2.0,
            display.min_y(),
            display.max_y() - label.height,
        ),
    ))
}

#[cfg(test)]
mod tests {
    use chartreuse_core::display::DisplayId;
    use chartreuse_core::window::{WindowId, WindowOwner};
    use chartreuse_platform::fake::{default_displays, default_windows};
    use iced::keyboard::key::{Named, Physical};
    use iced::keyboard::{Location, Modifiers};
    use iced::{Point, Size};

    use super::*;
    use crate::window::{Outcome, Phase};

    fn layout() -> DisplayLayout {
        DisplayLayout::new(default_displays()).expect("the fake displays form a layout")
    }

    fn selection() -> WindowSelection {
        WindowSelection::new(layout(), default_windows())
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

    fn window(title: Option<&str>, owner: &str) -> WindowInfo {
        WindowInfo {
            id: WindowId(1),
            title: title.map(str::to_owned),
            owner: WindowOwner {
                name: owner.into(),
                pid: None,
            },
            bounds: LogicalRect::new(0.0, 0.0, 100.0, 100.0),
            z_order: 0,
        }
    }

    #[test]
    fn the_label_names_the_title_or_else_the_application() {
        assert_eq!(
            label_text(&window(Some("  Notes — todo  "), "Notes")).as_deref(),
            Some("Notes — todo")
        );
        assert_eq!(
            label_text(&window(None, "Editor")).as_deref(),
            Some("Editor")
        );
        assert_eq!(
            label_text(&window(Some(" "), "Editor")).as_deref(),
            Some("Editor")
        );
        assert_eq!(label_text(&window(None, "")), None);

        // Long titles keep LABEL_MAX_CHARS characters, the last an ellipsis.
        let long = "é".repeat(LABEL_MAX_CHARS + 5);
        let label = label_text(&window(Some(&long), "Browser")).expect("a label");
        assert_eq!(label.chars().count(), LABEL_MAX_CHARS);
        assert!(label.ends_with('…'));
        let exact = "x".repeat(LABEL_MAX_CHARS);
        assert_eq!(
            label_text(&window(Some(&exact), "Browser")).as_deref(),
            Some(exact.as_str())
        );
    }

    #[test]
    fn the_label_is_centered_on_the_windows_largest_part_on_one_display() {
        let label = LogicalSize::new(100.0, 20.0);
        // Within one display: centered on the window.
        assert_eq!(
            label_origin(
                &layout(),
                &LogicalRect::new(100.0, 100.0, 800.0, 500.0),
                label
            ),
            Some(pt(450.0, 340.0))
        );
        // Spanning the external display (600 × 630 of it) and the primary
        // (600 × 700): centered on the part on the primary.
        assert_eq!(
            label_origin(
                &layout(),
                &LogicalRect::new(-600.0, 50.0, 1200.0, 700.0),
                label
            ),
            Some(pt(250.0, 390.0))
        );
        // Hanging off the desktop's left edge with only 40 points showing:
        // pinned to the display rather than centered off it.
        assert_eq!(
            label_origin(
                &layout(),
                &LogicalRect::new(-2000.0, 0.0, 120.0, 300.0),
                label
            ),
            Some(pt(-1920.0, 140.0))
        );
        // In a gap between displays: nowhere to show it.
        assert_eq!(
            label_origin(
                &layout(),
                &LogicalRect::new(-100.0, 700.0, 50.0, 20.0),
                label
            ),
            None
        );
    }

    #[derive(Debug, Clone, PartialEq)]
    struct Received(Input);

    /// Runs `event` through the overlay for `display_id`, returning the published
    /// input.
    fn send(
        selection: &WindowSelection,
        display_id: u64,
        state: &mut PointerState,
        event: &Event,
        cursor: Option<Point>,
    ) -> Option<Input> {
        let display = display(display_id);
        let handle = Handle::from_rgba(1, 1, vec![0; 4]);
        let overlay = WindowOverlay::new(selection, &display, &handle, Color::WHITE, Received);
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
    fn hover_follows_the_pointer_across_displays() {
        // Over the external display's canvas (origin -1920, -400), then the
        // primary's: the spanning window 102 stays hovered, and 101 takes over.
        let mut selection = selection();
        let mut state = PointerState::default();
        for (display_id, window, global, hovered) in [
            (2, (1620.0, 500.0), pt(-300.0, 100.0), Some(102)),
            (1, (50.0, 300.0), pt(50.0, 300.0), Some(102)),
            (1, (300.0, 300.0), pt(300.0, 300.0), Some(101)),
            (1, (1000.0, 900.0), pt(1000.0, 900.0), None),
        ] {
            let input = send(
                &selection,
                display_id,
                &mut state,
                &moved(window.0, window.1),
                None,
            );
            assert_eq!(input, Some(Input::Move(global)));
            selection.apply(input.expect("moved"));
            assert_eq!(selection.hovered().map(|w| w.id.0), hovered);
        }
    }

    #[test]
    fn a_press_and_release_commit_the_window_under_the_release() {
        // On the portrait display (origin 1512, 100), before any move.
        let mut selection = selection();
        let mut state = PointerState::default();
        let at = Some(Point::new(188.0, 300.0));
        let input = send(&selection, 3, &mut state, &press(), at);
        assert_eq!(input, Some(Input::Move(pt(1700.0, 400.0))));
        selection.apply(input.expect("pressed"));
        assert_eq!(selection.hovered().map(|w| w.id), Some(WindowId(103)));

        // The release arrives without a cursor position: the last one is used.
        let input = send(&selection, 3, &mut state, &release(), None);
        assert_eq!(input, Some(Input::Click(pt(1700.0, 400.0))));
        assert_eq!(
            selection.apply(input.expect("released")),
            Some(Outcome::Commit(WindowId(103)))
        );
        assert_eq!(selection.phase(), Phase::Committed(WindowId(103)));
    }

    #[test]
    fn releases_without_a_press_here_and_other_buttons_publish_nothing() {
        let selection = selection();
        let mut state = PointerState::default();
        let at = Some(Point::new(300.0, 300.0));
        // A release whose press this canvas never saw (it went elsewhere).
        assert_eq!(send(&selection, 1, &mut state, &release(), at), None);
        // A press outside this canvas belongs to another window.
        let outside = Some(Point::new(-5.0, 10.0));
        assert_eq!(send(&selection, 1, &mut state, &press(), outside), None);
        assert_eq!(send(&selection, 1, &mut state, &release(), outside), None);
        let right = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right));
        assert_eq!(send(&selection, 1, &mut state, &right, at), None);
        let right = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Right));
        assert_eq!(send(&selection, 1, &mut state, &right, at), None);
    }

    #[test]
    fn escape_is_published_and_forgets_a_press() {
        let selection = selection();
        let mut state = PointerState::default();
        let at = Some(Point::new(300.0, 300.0));
        assert!(send(&selection, 1, &mut state, &press(), at).is_some());
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
            send(&selection, 1, &mut state, &escape, None),
            Some(Input::Escape)
        );
        assert_eq!(send(&selection, 1, &mut state, &release(), at), None);
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
        let selection = selection();
        let mut state = PointerState::default();
        let over = Some(Point::new(300.0, 300.0));
        assert_eq!(
            send(&selection, 1, &mut state, &space(false), over),
            Some(Input::Space(Some(pt(300.0, 300.0))))
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
