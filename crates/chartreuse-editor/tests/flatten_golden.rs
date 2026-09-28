//! Golden-image tests for `flatten`: each renders a small document and
//! compares it with a reference PNG in `tests/golden/`, allowing
//! [`TOLERANCE`] per channel for floating-point differences between
//! platforms.
//!
//! After an intended rendering change, regenerate the references with
//! `UPDATE_GOLDEN=1 cargo test -p chartreuse-editor --test flatten_golden`
//! and review the new images. A failing comparison writes what it rendered
//! into `golden/` under Cargo's `CARGO_TARGET_TMPDIR` (the failure message
//! names the file) for inspection.
//!
//! Text uses only glyphs the bundled Inter Bold has, so no system fallback
//! font is involved and the images are the same on every platform.

use std::fs;
use std::path::PathBuf;

use chartreuse_core::color::Rgba8;
use chartreuse_core::geometry::PhysicalSize;
use chartreuse_core::image::Image;
use chartreuse_editor::flatten::flatten;
use chartreuse_editor::model::{
    Arrow, BlurMode, BlurRegion, Command, Document, Ellipse, Line, Point, Polyline, Rect,
    Rectangle, Shape, StepMarker, Style, Text,
};
use chartreuse_imaging::{decode, encode, Format};

/// The largest per-channel difference from the reference that still passes.
const TOLERANCE: u8 = 3;

const RED: Rgba8 = Style::DEFAULT_COLOR;
const BLUE: Rgba8 = Rgba8::rgb(0x0a, 0x84, 0xff);
const YELLOW: Rgba8 = Rgba8::rgb(0xff, 0xd6, 0x0a);

/// An opaque base with a soft gradient and a grid, so blending and
/// anti-aliasing against varied colors show in the images.
fn base(width: u32, height: u32) -> Image {
    Image::from_fn(PhysicalSize::new(width, height), |x, y| {
        if x % 16 == 0 || y % 16 == 0 {
            Rgba8::rgb(200, 200, 200)
        } else {
            Rgba8::rgb(
                (40 + x) as u8,
                (60 + y * 2) as u8,
                (120 + (x + y) / 2) as u8,
            )
        }
    })
}

fn style(color: Rgba8, stroke_width: f32) -> Style {
    Style {
        color,
        stroke_width,
        ..Style::default()
    }
}

/// Text in `color`, with no background, so the references show the glyphs
/// alone (the background is tested in `flatten`'s own tests).
fn text_style(color: Rgba8, font_size: f32) -> Style {
    Style {
        color,
        font_size,
        text_background: Rgba8::TRANSPARENT,
        ..Style::default()
    }
}

fn line(ax: f32, ay: f32, bx: f32, by: f32) -> Shape {
    Shape::Line(Line {
        start: Point::new(ax, ay),
        end: Point::new(bx, by),
    })
}

fn arrow(ax: f32, ay: f32, bx: f32, by: f32) -> Shape {
    Shape::Arrow(Arrow {
        start: Point::new(ax, ay),
        end: Point::new(bx, by),
    })
}

fn rectangle(ax: f32, ay: f32, bx: f32, by: f32) -> Shape {
    Shape::Rectangle(Rectangle {
        rect: Rect::from_corners(Point::new(ax, ay), Point::new(bx, by)),
    })
}

fn ellipse(ax: f32, ay: f32, bx: f32, by: f32) -> Shape {
    Shape::Ellipse(Ellipse {
        rect: Rect::from_corners(Point::new(ax, ay), Point::new(bx, by)),
    })
}

fn pen(points: &[(f32, f32)]) -> Shape {
    Shape::Pen(Polyline {
        points: points.iter().map(|&(x, y)| Point::new(x, y)).collect(),
    })
}

fn text(x: f32, y: f32, content: &str) -> Shape {
    Shape::Text(Text::new(Point::new(x, y), content))
}

fn blur(ax: f32, ay: f32, bx: f32, by: f32, mode: BlurMode) -> (Shape, Style) {
    let region = BlurRegion {
        rect: Rect::from_corners(Point::new(ax, ay), Point::new(bx, by)),
    };
    let style = Style {
        blur: mode,
        ..Style::default()
    };
    (Shape::Blur(region), style)
}

fn flattened(base: Image, shapes: impl IntoIterator<Item = (Shape, Style)>) -> Image {
    let mut document = Document::new(base);
    for (shape, style) in shapes {
        document.add(shape, style);
    }
    flatten(&document).expect("flattens")
}

/// Compares `image` with `tests/golden/<name>.png`, or rewrites that file
/// when `UPDATE_GOLDEN=1`.
fn check(name: &str, image: &Image) {
    let file = format!("{name}.png");
    let reference = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(&file);
    if std::env::var_os("UPDATE_GOLDEN").is_some_and(|value| value == "1") {
        fs::write(&reference, encode(image, Format::Png).unwrap()).unwrap();
        return;
    }
    let bytes = fs::read(&reference).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; run with UPDATE_GOLDEN=1 to create it",
            reference.display()
        )
    });
    let expected = decode(&bytes).unwrap();
    let (mut worst, mut mismatched) = (0, 0);
    if expected.size() == image.size() {
        for (a, b) in expected
            .pixels()
            .chunks_exact(4)
            .zip(image.pixels().chunks_exact(4))
        {
            let diff = a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            worst = worst.max(diff);
            mismatched += usize::from(diff > TOLERANCE);
        }
    }
    if expected.size() != image.size() || mismatched > 0 {
        let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("golden");
        fs::create_dir_all(&out).unwrap();
        let actual = out.join(&file);
        fs::write(&actual, encode(image, Format::Png).unwrap()).unwrap();
        panic!(
            "{name}: {mismatched} pixels differ from {} by more than {TOLERANCE} \
             (worst {worst}; sizes {:?} and {:?}); rendered image in {}",
            reference.display(),
            expected.size(),
            image.size(),
            actual.display()
        );
    }
}

#[test]
fn lines() {
    let image = flattened(
        base(96, 64),
        [
            (line(8.0, 56.0, 60.0, 8.0), style(RED, 6.0)),
            // Thin, at fractional coordinates.
            (line(10.25, 20.5, 88.75, 34.0), style(YELLOW, 1.5)),
            // Zero length: a disc.
            (line(78.0, 50.0, 78.0, 50.0), style(BLUE, 12.0)),
        ],
    );
    check("line", &image);
}

#[test]
fn arrows() {
    let image = flattened(
        base(112, 72),
        [
            (arrow(8.0, 64.0, 70.0, 10.0), style(RED, 4.0)),
            // Thick: the head is 3× the stroke width, and the shaft stops at
            // its base.
            (arrow(20.0, 40.0, 104.0, 56.0), style(BLUE, 8.0)),
            // Shorter than its head: all head.
            (arrow(90.0, 12.0, 98.0, 22.0), style(YELLOW, 4.0)),
        ],
    );
    check("arrow", &image);
}

#[test]
fn rectangles() {
    let image = flattened(
        base(96, 72),
        [
            // Dragged bottom-right to top-left.
            (rectangle(80.0, 60.0, 10.0, 8.0), style(RED, 8.0)),
            (rectangle(30.5, 28.5, 60.5, 44.5), style(YELLOW, 1.0)),
            // A zero-width rectangle is a line with round ends.
            (rectangle(88.0, 10.0, 88.0, 40.0), style(BLUE, 4.0)),
        ],
    );
    check("rectangle", &image);
}

#[test]
fn ellipses() {
    let image = flattened(
        base(112, 72),
        [
            (ellipse(8.0, 8.0, 70.0, 64.0), style(RED, 6.0)),
            // A circle at fractional coordinates, thin.
            (ellipse(40.5, 20.25, 72.5, 52.25), style(YELLOW, 1.5)),
            // Flat: a line with round ends. And a dot.
            (ellipse(78.0, 12.0, 106.0, 12.0), style(BLUE, 4.0)),
            (ellipse(92.0, 50.0, 92.0, 50.0), style(BLUE, 12.0)),
        ],
    );
    check("ellipse", &image);
}

#[test]
fn pen_strokes() {
    let spiral: Vec<_> = (0..80)
        .map(|i| {
            let t = i as f32 / 8.0;
            (40.0 + t.cos() * t * 3.0, 36.0 + t.sin() * t * 3.0)
        })
        .collect();
    let image = flattened(
        base(112, 72),
        [
            (pen(&spiral), style(RED, 3.0)),
            // Sharp corners get round joins.
            (
                pen(&[(76.0, 64.0), (86.0, 10.0), (96.0, 64.0), (106.0, 10.0)]),
                style(BLUE, 6.0),
            ),
            // A single point: a dot.
            (pen(&[(20.0, 62.0)]), style(YELLOW, 10.0)),
        ],
    );
    check("pen", &image);
}

#[test]
fn highlighters() {
    let highlight = |points: &[(f32, f32)]| {
        Shape::Highlighter(Polyline {
            points: points.iter().map(|&(x, y)| Point::new(x, y)).collect(),
        })
    };
    let image = flattened(
        base(112, 72),
        [
            (text(6.0, 8.0, "Mark"), text_style(RED, 24.0)),
            // Over the text, folding back over itself: one even tint.
            (
                highlight(&[(6.0, 22.0), (66.0, 22.0), (60.0, 30.0), (10.0, 30.0)]),
                style(YELLOW, 3.0),
            ),
            // A second stroke across it is a second coat.
            (highlight(&[(90.0, 6.0), (40.0, 66.0)]), style(BLUE, 2.0)),
            // A translucent color is fainter still, and a dot is round.
            (
                highlight(&[(96.0, 56.0)]),
                style(Rgba8::new(52, 199, 89, 128), 5.0),
            ),
        ],
    );
    check("highlighter", &image);
}

#[test]
fn step_markers() {
    let step = |x: f32, y: f32| {
        Shape::Step(StepMarker {
            center: Point::new(x, y),
        })
    };
    let mut document = Document::new(base(128, 64));
    // Four markers, the second deleted: the others show 1, 2, and 3, each
    // number in the color that reads on its disc.
    document.add(step(20.0, 20.0), text_style(RED, 20.0));
    let deleted = document.add(step(50.0, 20.0), text_style(BLUE, 20.0));
    document.add(step(64.0, 40.5), text_style(YELLOW, 24.0));
    document.add(
        step(104.25, 30.0),
        text_style(Rgba8::rgb(255, 255, 255), 14.0),
    );
    document.apply(Command::Delete { ids: vec![deleted] });
    check("step", &flatten(&document).expect("flattens"));
}

#[test]
fn text_annotations() {
    let image = flattened(
        base(176, 80),
        [
            (text(6.0, 4.0, "Chartreuse\nÅgy? 42"), text_style(RED, 22.0)),
            // A fractional position and a small size.
            (text(118.3, 60.6, "small"), text_style(YELLOW, 11.0)),
        ],
    );
    check("text", &image);
}

#[test]
fn blur_regions() {
    let image = flattened(
        base(160, 96),
        [
            (text(6.0, 6.0, "Secret"), text_style(RED, 26.0)),
            (line(4.0, 70.0, 150.0, 50.0), style(YELLOW, 6.0)),
            // Pixelated over the text, at fractional edges that round to
            // whole pixels.
            blur(4.4, 4.0, 90.6, 40.0, BlurMode::Pixelate),
            // Blurred, off the right edge of the image and over the
            // pixelated one: it blurs the mosaic too.
            blur(70.0, 20.0, 180.0, 80.0, BlurMode::Gaussian),
            // Drawn above both, so neither touches it.
            (arrow(20.0, 88.0, 120.0, 30.0), style(BLUE, 4.0)),
        ],
    );
    check("blur", &image);
}

#[test]
fn crop() {
    let mut document = Document::new(base(128, 96));
    document.add(rectangle(8.0, 8.0, 100.0, 70.0), style(BLUE, 6.0));
    document.add(text(40.0, 30.0, "Crop"), text_style(RED, 30.0));
    let (shape, pixelate) = blur(60.0, 50.0, 120.0, 90.0, BlurMode::Pixelate);
    document.add(shape, pixelate);
    // Annotations run past the crop's edges, cut off with the image.
    document.apply(Command::SetCrop(Some(Rect::from_corners(
        Point::new(24.0, 16.0),
        Point::new(112.0, 80.0),
    ))));
    let image = flatten(&document).expect("flattens");
    assert_eq!((image.width(), image.height()), (88, 64));
    check("crop", &image);
}

#[test]
fn z_order() {
    let image = flattened(
        base(128, 72),
        [
            (rectangle(10.0, 10.0, 70.0, 60.0), style(BLUE, 10.0)),
            // Text above the rectangle, a line above the text, and an arrow
            // above everything: each is drawn over the ones before it.
            (text(20.0, 20.0, "Top"), text_style(YELLOW, 28.0)),
            (line(4.0, 40.0, 120.0, 30.0), style(RED, 6.0)),
            (
                arrow(110.0, 64.0, 40.0, 16.0),
                style(Rgba8::rgb(255, 255, 255), 3.0),
            ),
        ],
    );
    check("z_order", &image);
}

#[test]
fn translucent_colors() {
    // The left half of the base is transparent: translucent annotations over
    // it stay translucent in the output, in straight alpha.
    let opaque = base(112, 64);
    let base = Image::from_fn(opaque.size(), |x, y| {
        if x < 40 {
            Rgba8::new(0, 0, 0, 0)
        } else {
            opaque.pixel(x, y).unwrap()
        }
    });
    let image = flattened(
        base,
        [
            (
                rectangle(12.0, 10.0, 80.0, 50.0),
                style(Rgba8::new(10, 132, 255, 140), 12.0),
            ),
            (
                line(4.0, 56.0, 104.0, 6.0),
                style(Rgba8::new(255, 59, 48, 128), 10.0),
            ),
            (
                text(20.0, 18.0, "50%"),
                text_style(Rgba8::new(255, 214, 10, 160), 26.0),
            ),
        ],
    );
    check("translucent", &image);
}
