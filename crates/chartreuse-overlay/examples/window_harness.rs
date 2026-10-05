//! Development harness for the window-selection overlay, on the `fake` platform
//! backend (no real capture, no Screen Recording permission).
//!
//! ```text
//! cargo run -p chartreuse-overlay --example window_harness [-- --scale 0.5] [--demo]
//! ```
//!
//! Opens one ordinary window per fake display, laid out like the fake desktop and
//! scaled down to fit (or by `--scale`), each showing that display's generated
//! test-pattern capture with the fake window list painted on it, back to front
//! (each window a flat color with a darker title bar). Move the pointer to
//! highlight the window under it; click to commit, which prints the window's id,
//! application and title; Escape cancels. Either way a new selection starts.
//! Close any window to quit.
//!
//! `--demo` starts with the pointer over the window that spans the external and
//! primary displays and saves each window's rendering (iced's own window
//! screenshot, which needs no Screen Recording permission) to a temporary
//! directory.
//!
//! Windows are decorated and normal-level (overlay window setup belongs to
//! `chartreuse_overlay::setup`). Every window has the same title bar, so their
//! contents keep the desktop's relative layout.

use std::path::PathBuf;
use std::time::Duration;

use chartreuse_core::color::Rgba8;
use chartreuse_core::display::{DisplayInfo, DisplayLayout};
use chartreuse_core::flavor;
use chartreuse_core::geometry::{LogicalPoint, LogicalRect, PhysicalRect, PhysicalSize};
use chartreuse_core::image::Image;
use chartreuse_core::window::{WindowId, WindowInfo};
use chartreuse_imaging::{encode, Format};
use chartreuse_overlay::shared::frozen_image;
use chartreuse_overlay::window::{Input, Outcome, WindowOverlay, WindowSelection};
use chartreuse_platform::fake::Fake;
use iced::widget::image::Handle;
use iced::{window, Color, Element, Point, Size, Subscription, Task};

/// The screen area (logical points) the scaled-down desktop is fitted into.
const FIT: Size = Size::new(1400.0, 800.0);
/// Where the desktop's top-left corner goes on the screen.
const MARGIN: Point = Point::new(40.0, 60.0);
/// The height of a painted window's title bar, in logical points.
const TITLE_BAR: f64 = 28.0;
/// Window body colors, by position in the window list.
const BODIES: [Rgba8; 4] = [
    Rgba8::rgb(0xe8, 0xd5, 0xb0),
    Rgba8::rgb(0xb0, 0xd0, 0xe8),
    Rgba8::rgb(0xc8, 0xe8, 0xb0),
    Rgba8::rgb(0xe0, 0xb8, 0xe0),
];

fn main() -> iced::Result {
    iced::daemon(Harness::boot, Harness::update, Harness::view)
        .title(Harness::title)
        .subscription(Harness::subscription)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    Selection(Input),
    Screenshot(window::Id, window::Screenshot),
    WindowClosed,
}

/// Command-line options.
#[derive(Debug, Default)]
struct Args {
    scale: Option<f64>,
    demo: bool,
}

#[derive(Debug)]
struct Overlay {
    window: window::Id,
    display: DisplayInfo,
    image: Handle,
}

#[derive(Debug)]
struct Harness {
    selection: WindowSelection,
    overlays: Vec<Overlay>,
    accent: Color,
    output_dir: PathBuf,
}

impl Harness {
    fn boot() -> (Self, Task<Message>) {
        let platform = Fake::new().platform();
        let displays = platform
            .displays
            .displays()
            .expect("the fake backend lists displays");
        let layout = DisplayLayout::new(displays).expect("the fake displays form a layout");
        let captures = futures::executor::block_on(platform.capture.capture_displays())
            .expect("the fake backend captures");
        let windows = futures::executor::block_on(platform.window_list.windows())
            .expect("the fake backend lists windows");

        let args = parse_args();
        let desktop = layout.bounds();
        let scale = args.scale.unwrap_or_else(|| {
            let fit = (f64::from(FIT.width) / desktop.size.width)
                .min(f64::from(FIT.height) / desktop.size.height);
            fit.min(1.0)
        });
        let output_dir = std::env::temp_dir().join("chartreuse-window-harness");
        println!(
            "window harness: {} displays, {} windows, shown at {scale:.2}×",
            captures.len(),
            windows.len()
        );
        for window in &windows {
            println!("  {}", describe(window));
        }
        println!("point at a window and click it, Escape cancels, close a window to quit");

        let mut tasks = Vec::new();
        let mut overlays = Vec::new();
        for capture in captures {
            let bounds = capture.display.logical_bounds;
            let (window, open) = window::open(window::Settings {
                size: Size::new(
                    (bounds.size.width * scale) as f32,
                    (bounds.size.height * scale) as f32,
                ),
                position: window::Position::Specific(Point::new(
                    MARGIN.x + ((bounds.origin.x - desktop.origin.x) * scale) as f32,
                    MARGIN.y + ((bounds.origin.y - desktop.origin.y) * scale) as f32,
                )),
                resizable: false,
                ..window::Settings::default()
            });
            tasks.push(if args.demo {
                open.then(|window| {
                    // A screenshot reads back the last frame drawn; give the new
                    // window time to draw its first one.
                    Task::future(async { std::thread::sleep(Duration::from_secs(2)) }).then(
                        move |()| {
                            window::screenshot(window)
                                .map(move |shot| Message::Screenshot(window, shot))
                        },
                    )
                })
            } else {
                open.discard()
            });
            let mut image = capture.image;
            paint_windows(&mut image, &capture.display, &windows);
            overlays.push(Overlay {
                window,
                display: capture.display,
                image: frozen_image(image),
            });
        }

        let accent = flavor::ACCENT;
        let mut selection = WindowSelection::new(layout, windows);
        if args.demo {
            // Over the browser, which spans the external and primary displays.
            selection.apply(Input::Move(LogicalPoint::new(-300.0, 300.0)));
            println!(
                "demo: hovering {}; screenshots go to {}",
                selection.hovered().map_or_else(String::new, describe),
                output_dir.display()
            );
        }
        let harness = Self {
            selection,
            overlays,
            accent: Color::from_rgba8(accent.r, accent.g, accent.b, f32::from(accent.a) / 255.0),
            output_dir,
        };
        (harness, Task::batch(tasks))
    }

    fn title(&self, window: window::Id) -> String {
        self.overlay(window).map_or_else(String::new, |overlay| {
            format!("Window harness: {}", overlay.display.name)
        })
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Selection(input) => {
                match self.selection.apply(input) {
                    Some(Outcome::Commit(id)) => self.committed(id),
                    Some(Outcome::Cancel) => println!("cancelled"),
                    None => return Task::none(),
                }
                // Start over, still highlighting the window under the pointer.
                let mut next = WindowSelection::new(
                    self.selection.layout().clone(),
                    self.selection.windows().to_vec(),
                );
                if let Some(pointer) = self.selection.pointer() {
                    next.move_to(pointer);
                }
                self.selection = next;
                Task::none()
            }
            Message::Screenshot(window, shot) => {
                let Some(overlay) = self.overlay(window) else {
                    return Task::none();
                };
                let name = format!("window-{}.png", overlay.display.id.0);
                let size = PhysicalSize::new(shot.size.width, shot.size.height);
                match Image::new(size, shot.rgba.to_vec()) {
                    Ok(image) => self.save(&name, &image),
                    Err(error) => eprintln!("unusable screenshot of {name}: {error}"),
                }
                Task::none()
            }
            Message::WindowClosed => iced::exit(),
        }
    }

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        match self.overlay(window) {
            Some(overlay) => WindowOverlay::new(
                &self.selection,
                &overlay.display,
                &overlay.image,
                self.accent,
                Message::Selection,
            )
            .view(),
            None => iced::widget::space().into(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        window::close_events().map(|_| Message::WindowClosed)
    }

    fn overlay(&self, window: window::Id) -> Option<&Overlay> {
        self.overlays
            .iter()
            .find(|overlay| overlay.window == window)
    }

    fn committed(&self, id: WindowId) {
        let window = self
            .selection
            .hovered()
            .expect("a committed selection keeps its window");
        assert_eq!(window.id, id);
        println!("committed {}", describe(window));
    }

    /// Writes `image` as a PNG named `name` in the output directory.
    fn save(&self, name: &str, image: &Image) {
        let path = self.output_dir.join(name);
        let written = encode(image, Format::Png)
            .map_err(|error| error.to_string())
            .and_then(|png| {
                std::fs::create_dir_all(&self.output_dir).map_err(|error| error.to_string())?;
                std::fs::write(&path, png).map_err(|error| error.to_string())
            });
        match written {
            Ok(()) => println!("  wrote {}", path.display()),
            Err(error) => eprintln!("  could not write {}: {error}", path.display()),
        }
    }
}

/// Paints `windows` onto `display`'s capture, back to front, so the frozen image
/// shows where they are.
fn paint_windows(image: &mut Image, display: &DisplayInfo, windows: &[WindowInfo]) {
    let grid = display.pixel_grid();
    let mut back_to_front: Vec<(usize, &WindowInfo)> = windows.iter().enumerate().collect();
    back_to_front.sort_by_key(|(_, window)| std::cmp::Reverse(window.z_order));
    for (index, window) in back_to_front {
        let body = BODIES[index % BODIES.len()];
        let bar = Rgba8::rgb(body.r / 2, body.g / 2, body.b / 2);
        let bounds = window.bounds;
        let title_bar = LogicalRect::new(
            bounds.min_x(),
            bounds.min_y(),
            bounds.size.width,
            TITLE_BAR.min(bounds.size.height),
        );
        fill(image, grid.rect_to_physical(&bounds), body);
        fill(image, grid.rect_to_physical(&title_bar), bar);
    }
}

/// Fills the part of `rect` (image pixels) that lies on `image` with `color`.
fn fill(image: &mut Image, rect: PhysicalRect, color: Rgba8) {
    let all = PhysicalRect::new(0, 0, image.width(), image.height());
    let Some(rect) = rect.intersection(&all) else {
        return;
    };
    let stride = image.width() as usize * 4;
    let (x, y) = (rect.origin.x as usize, rect.origin.y as usize);
    let pixel = color.to_array();
    for row in image
        .pixels_mut()
        .chunks_exact_mut(stride)
        .skip(y)
        .take(rect.size.height as usize)
    {
        for out in row[x * 4..(x + rect.size.width as usize) * 4]
            .as_chunks_mut::<4>()
            .0
        {
            *out = pixel;
        }
    }
}

/// Parses `--scale <factor>` and `--demo`.
fn parse_args() -> Args {
    let mut parsed = Args::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => parsed.demo = true,
            "--scale" => {
                parsed.scale = args
                    .next()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|scale| *scale > 0.0 && scale.is_finite());
                if parsed.scale.is_none() {
                    eprintln!("--scale needs a positive number; fitting to the screen");
                }
            }
            other => eprintln!("ignoring unknown argument {other:?}"),
        }
    }
    parsed
}

fn describe(window: &WindowInfo) -> String {
    let bounds = window.bounds;
    format!(
        "window {} ({}): {:?} at ({}, {}) {}×{} pt",
        window.id.0,
        window.owner.name,
        window.title.as_deref().unwrap_or("(untitled)"),
        bounds.min_x(),
        bounds.min_y(),
        bounds.size.width,
        bounds.size.height
    )
}
