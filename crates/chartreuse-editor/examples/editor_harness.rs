//! A development harness for the editor widget: one editor window on a
//! generated test pattern, with no tray, hotkeys, capture, or export around
//! it.
//!
//! ```text
//! cargo run -p chartreuse-editor --example editor_harness [-- --demo]
//! ```
//!
//! `--demo` starts with one annotation of each kind, drawn through the same
//! messages the canvas sends. Cmd+S and Cmd+C print the event the app would
//! export on.

use chartreuse_core::flavor;
use chartreuse_core::geometry::{PhysicalSize, ScaleFactor};
use chartreuse_editor::canvas::{self, Input, InputKind, MARGIN};
use chartreuse_editor::model::BlurMode;
use chartreuse_editor::tools::ToolKind;
use chartreuse_editor::{Editor, Message};
use chartreuse_platform::fake;
use iced::keyboard::{self, key::Named};
use iced::theme::Palette;
use iced::{Element, Point, Size, Theme};

/// The test pattern's size: a 2× capture of an 800 × 500 point area.
const IMAGE: PhysicalSize = PhysicalSize::new(1600, 1000);

fn main() -> iced::Result {
    iced::application(boot, update, view)
        .title("Chartreuse editor harness")
        .theme(theme)
        .window_size(Size::new(1200.0, 800.0))
        .run()
}

struct Harness {
    editor: Editor,
}

fn boot() -> Harness {
    let scale = ScaleFactor::new(2.0).expect("2 is a valid scale factor");
    let mut editor = Editor::new(fake::test_pattern(IMAGE, scale, 0));
    if std::env::args().any(|arg| arg == "--demo") {
        for message in demo() {
            editor.update(message);
        }
    }
    Harness { editor }
}

fn update(harness: &mut Harness, message: Message) {
    if let Some(event) = harness.editor.update(message) {
        println!("editor event: {event:?}");
    }
}

fn view(harness: &Harness) -> Element<'_, Message> {
    iced::widget::column![
        harness.editor.view(),
        iced::widget::container(harness.editor.zoom_controls()).padding(8),
    ]
    .into()
}

/// The app's theme: dark, with the flavor's accent as the primary color.
fn theme(_harness: &Harness) -> Theme {
    Theme::custom(
        flavor::DISPLAY_NAME,
        Palette {
            primary: canvas::color(flavor::ACCENT),
            ..Palette::DARK
        },
    )
}

/// Messages that draw one annotation of each kind (two lines of text),
/// as a canvas exactly [`MARGIN`] larger than the image on every side would
/// send them (a fitted view at 1:1, so canvas = document + margin).
fn demo() -> Vec<Message> {
    let size = Size::new(
        IMAGE.width as f32 + 2.0 * MARGIN,
        IMAGE.height as f32 + 2.0 * MARGIN,
    );
    let input = |kind| Message::Canvas(Input { size, kind });
    let at = |x: f32, y: f32| Point::new(x + MARGIN, y + MARGIN);
    let drag = |from: Point, to: Point| {
        [
            InputKind::Press {
                position: from,
                clicks: 1,
            },
            InputKind::Move { position: to },
            InputKind::Release { position: to },
        ]
        .map(input)
    };
    // A press, a move through each of `points`, and a release at the last.
    let stroke = |points: &[Point]| {
        let mut messages = vec![input(InputKind::Press {
            position: points[0],
            clicks: 1,
        })];
        messages.extend(
            points[1..]
                .iter()
                .map(|&position| input(InputKind::Move { position })),
        );
        messages.push(input(InputKind::Release {
            position: points[points.len() - 1],
        }));
        messages
    };
    let wave = |x0: f32, x1: f32, y: f32, amplitude: f32| -> Vec<Point> {
        (0..=80)
            .map(|i| {
                let t = i as f32 / 80.0;
                at(x0 + (x1 - x0) * t, y + amplitude * (t * 14.0).sin())
            })
            .collect()
    };
    let key = |key: keyboard::Key, text: Option<&str>| {
        input(InputKind::Key {
            key,
            modifiers: keyboard::Modifiers::default(),
            text: text.map(Into::into),
        })
    };

    let mut script = vec![Message::Tool(ToolKind::Rectangle)];
    script.extend(drag(at(200.0, 150.0), at(700.0, 450.0)));
    script.push(Message::Tool(ToolKind::Arrow));
    script.extend(drag(at(1150.0, 750.0), at(720.0, 470.0)));
    script.push(Message::Tool(ToolKind::Line));
    script.extend(drag(at(200.0, 820.0), at(900.0, 880.0)));
    script.push(Message::Tool(ToolKind::Ellipse));
    script.extend(drag(at(1000.0, 120.0), at(1450.0, 400.0)));
    script.push(Message::Tool(ToolKind::Pen));
    script.extend(stroke(&wave(1000.0, 1500.0, 560.0, 40.0)));
    script.push(Message::Tool(ToolKind::Highlighter));
    script.extend(stroke(&wave(200.0, 900.0, 930.0, 12.0)));
    // Pixelate part of the rectangle and the line, and blur part of the pen
    // stroke.
    script.push(Message::Tool(ToolKind::Blur));
    let escape = key(keyboard::Key::Named(Named::Escape), None);
    script.extend(drag(at(560.0, 380.0), at(820.0, 900.0)));
    script.extend([escape.clone(), Message::BlurMode(BlurMode::Gaussian)]);
    script.extend(drag(at(1250.0, 480.0), at(1550.0, 640.0)));
    script.extend([escape, Message::BlurMode(BlurMode::Pixelate)]);
    script.push(Message::Tool(ToolKind::Step));
    for (x, y) in [(260.0, 110.0), (730.0, 110.0), (1170.0, 700.0)] {
        script.extend(drag(at(x, y), at(x, y)));
    }
    script.push(Message::Tool(ToolKind::Text));
    script.extend([
        input(InputKind::Press {
            position: at(760.0, 250.0),
            clicks: 1,
        }),
        input(InputKind::Release {
            position: at(760.0, 250.0),
        }),
    ]);
    for line in ["Click here!", "Then there."].iter().enumerate() {
        if line.0 > 0 {
            script.push(key(keyboard::Key::Named(Named::Enter), None));
        }
        for c in line.1.chars() {
            let c = c.to_string();
            script.push(key(keyboard::Key::Character(c.as_str().into()), Some(&c)));
        }
    }
    script.push(key(keyboard::Key::Named(Named::Escape), None));
    script.push(Message::Tool(ToolKind::Select));
    script
}
