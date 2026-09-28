//! Keeping keys a focused field took from reaching the canvas.
//!
//! iced hands every event to every widget: a row or column updates all its
//! children whether or not an earlier one captured the event. So keys typed
//! into a toolbar field (the resize tool's) would also reach the canvas,
//! where Backspace deletes the selection and letters switch tools. The canvas
//! is laid out after the toolbar, so by the time it is updated a focused
//! field has captured the keys it took; [`skip_captured_keys`] drops those.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{overlay, renderer, Clipboard, Shell, Widget};
use iced::{keyboard, mouse, Element, Event, Length, Rectangle, Size, Vector};

/// `content`, except that it never hears a key press another widget has
/// already captured.
pub(crate) fn skip_captured_keys<'a, Message: 'a, Theme: 'a, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> Element<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(SkipCapturedKeys {
        content: content.into(),
    })
}

/// A transparent wrapper: `content`'s own tree, layout, and drawing.
struct SkipCapturedKeys<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for SkipCapturedKeys<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if shell.is_event_captured()
            && matches!(event, Event::Keyboard(keyboard::Event::KeyPressed { .. }))
        {
            return;
        }
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}

#[cfg(test)]
mod tests {
    use iced::advanced::clipboard;
    use iced::advanced::widget::operation::focusable;
    use iced::futures::executor::block_on;
    use iced::keyboard::key::{Code, Physical};
    use iced::widget::canvas::{Action, Geometry, Program};
    use iced::widget::{column, text_input, Canvas};
    use iced::{Point, Renderer, Theme};
    use iced_runtime::user_interface::{Cache, UserInterface};

    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum Heard {
        Field(String),
        Canvas,
    }

    /// A canvas that reports every key press it hears.
    struct Keys;

    impl Program<Heard> for Keys {
        type State = ();

        fn update(
            &self,
            _state: &mut (),
            event: &Event,
            _bounds: Rectangle,
            _cursor: mouse::Cursor,
        ) -> Option<Action<Heard>> {
            matches!(event, Event::Keyboard(keyboard::Event::KeyPressed { .. }))
                .then(|| Action::publish(Heard::Canvas))
        }

        fn draw(
            &self,
            _state: &(),
            _renderer: &Renderer,
            _theme: &Theme,
            _bounds: Rectangle,
            _cursor: mouse::Cursor,
        ) -> Vec<Geometry> {
            Vec::new()
        }
    }

    fn key(c: &str) -> Event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character(c.into()),
            modified_key: keyboard::Key::Character(c.into()),
            physical_key: Physical::Code(Code::KeyX),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::default(),
            text: Some(c.into()),
            repeat: false,
        })
    }

    #[test]
    fn keys_a_focused_field_takes_never_reach_the_content() {
        let mut renderer = block_on(<Renderer as renderer::Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .expect("a tiny-skia renderer");
        let view = || -> Element<'_, Heard, Theme, Renderer> {
            column![
                text_input("", "").id("field").on_input(Heard::Field),
                skip_captured_keys(Canvas::new(Keys)),
            ]
            .into()
        };
        let mut cache = Cache::default();
        let mut heard = |focus: bool, cache: Cache| {
            let mut ui =
                UserInterface::build(view(), Size::new(200.0, 200.0), cache, &mut renderer);
            if focus {
                ui.operate(&renderer, &mut focusable::focus("field".into()));
            }
            let mut messages = Vec::new();
            let _ = ui.update(
                &[key("x")],
                mouse::Cursor::Available(Point::ORIGIN),
                &mut renderer,
                &mut clipboard::Null,
                &mut messages,
            );
            (messages, ui.into_cache())
        };

        let (messages, next) = heard(false, cache);
        assert_eq!(messages, [Heard::Canvas], "no field has focus");
        cache = next;
        let (messages, _) = heard(true, cache);
        assert_eq!(messages, [Heard::Field("x".into())], "the field took it");
    }
}
