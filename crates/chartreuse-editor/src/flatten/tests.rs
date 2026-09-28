use chartreuse_core::color::Rgba8;
use chartreuse_core::geometry::{PhysicalPoint, PhysicalRect, PhysicalSize};
use chartreuse_core::image::Image;

use super::*;
use crate::font;
use crate::model::{
    distance_to_ellipse, distance_to_polyline, distance_to_segment, distance_to_triangle, Arrow,
    BlurMode, BlurRegion, Command, Ellipse, Line, Polyline, Rect, Rectangle, StepMarker, Style,
    Text,
};

/// How far outside a shape's edge a pixel's center can be and still be
/// partly covered: half a pixel's diagonal, plus slack for anti-aliasing.
const EDGE: f32 = 0.75;

const BLUE: Rgba8 = Rgba8::rgb(0x20, 0x40, 0xf0);

/// A base image with every pixel different, including translucent ones, so
/// any pixel flatten touches by mistake shows.
fn base(width: u32, height: u32) -> Image {
    Image::from_fn(PhysicalSize::new(width, height), |x, y| {
        Rgba8::new(
            (x * 7) as u8,
            (y * 11) as u8,
            ((x + y) * 3) as u8,
            if (x + y) % 3 == 0 { 90 } else { 255 },
        )
    })
}

fn style(color: Rgba8, stroke_width: f32) -> Style {
    Style {
        color,
        stroke_width,
        ..Style::default()
    }
}

fn flattened(base: Image, shapes: impl IntoIterator<Item = (Shape, Style)>) -> Image {
    let mut document = Document::new(base);
    for (shape, style) in shapes {
        document.add(shape, style);
    }
    flatten(&document).expect("flattens")
}

fn line(ax: f32, ay: f32, bx: f32, by: f32) -> Shape {
    Shape::Line(Line {
        start: Point::new(ax, ay),
        end: Point::new(bx, by),
    })
}

/// Checks every pixel of `result` against `base` given the signed distance
/// from a pixel's center to the drawn area (negative inside): pixels well
/// inside are exactly `color` (opaque), pixels well outside keep their bytes.
fn assert_covers(base: &Image, result: &Image, color: Rgba8, distance: impl Fn(Point) -> f32) {
    let (mut inside, mut outside) = (0, 0);
    for y in 0..base.height() {
        for x in 0..base.width() {
            let d = distance(Point::new(x as f32 + 0.5, y as f32 + 0.5));
            let pixel = result.pixel(x, y).unwrap();
            if d < -EDGE {
                assert_eq!(pixel, color, "inside at ({x}, {y})");
                inside += 1;
            } else if d > EDGE {
                assert_eq!(pixel, base.pixel(x, y).unwrap(), "outside at ({x}, {y})");
                outside += 1;
            }
        }
    }
    assert!(
        inside > 0 && outside > 0,
        "{inside} inside, {outside} outside"
    );
}

#[test]
fn a_stroke_covers_exactly_the_points_within_half_its_width() {
    let image = base(60, 40);
    // Runs off the left edge: flattening clips to the image.
    let (a, b) = (Point::new(-10.0, 12.5), Point::new(45.3, 30.0));
    let result = flattened(
        image.clone(),
        [(line(a.x, a.y, b.x, b.y), style(BLUE, 7.0))],
    );
    assert_covers(&image, &result, BLUE, |p| {
        distance_to_segment(p, a, b) - 3.5
    });
}

#[test]
fn a_rectangle_is_its_outline_with_round_outer_corners() {
    let image = base(60, 50);
    let rect = Rect::from_corners(Point::new(10.0, 8.0), Point::new(50.0, 40.0));
    let result = flattened(
        image.clone(),
        [(Shape::Rectangle(Rectangle { rect }), style(BLUE, 6.0))],
    );
    assert_covers(&image, &result, BLUE, |p| rect.distance_to_outline(p) - 3.0);
}

#[test]
fn an_ellipse_is_its_outline_stroked() {
    let image = base(70, 50);
    let rect = Rect::from_corners(Point::new(8.5, 6.0), Point::new(61.0, 44.25));
    let result = flattened(
        image.clone(),
        [(Shape::Ellipse(Ellipse { rect }), style(BLUE, 6.0))],
    );
    assert_covers(&image, &result, BLUE, |p| {
        distance_to_ellipse(p, rect) - 3.0
    });
}

#[test]
fn a_pen_stroke_is_its_path_stroked_with_round_joins() {
    let image = base(70, 50);
    let points: Vec<_> = [
        (5.0, 40.0),
        (20.5, 8.0),
        (34.0, 42.25),
        (52.0, 10.0),
        (66.0, 30.0),
    ]
    .into_iter()
    .map(|(x, y)| Point::new(x, y))
    .collect();
    let result = flattened(
        image.clone(),
        [(
            Shape::Pen(Polyline {
                points: points.clone(),
            }),
            style(BLUE, 5.0),
        )],
    );
    assert_covers(&image, &result, BLUE, |p| {
        distance_to_polyline(p, &points) - 2.5
    });
}

#[test]
fn an_arrow_is_a_shaft_to_the_head_base_and_a_filled_head() {
    let image = base(80, 50);
    let arrow = Arrow {
        start: Point::new(8.0, 30.0),
        end: Point::new(70.0, 12.0),
    };
    // Thick enough that a shaft stroked to the tip would poke out past it.
    let width = 8.0;
    let head = arrow.head(width).unwrap();
    let result = flattened(
        image.clone(),
        [(Shape::Arrow(arrow.clone()), style(BLUE, width))],
    );
    assert_covers(&image, &result, BLUE, |p| {
        let shaft = distance_to_segment(p, arrow.start, head.base) - width / 2.0;
        // distance_to_triangle is 0 inside, so only the outside is checked
        // for the head; its interior is as deep as the shaft allows.
        shaft.min(distance_to_triangle(p, head.corners()))
    });
}

#[test]
fn zero_length_strokes_are_discs_and_zero_width_draws_nothing() {
    let image = base(30, 30);
    let center = Point::new(14.5, 15.0);
    let point = Rect::from_corners(center, center);
    for shape in [
        line(center.x, center.y, center.x, center.y),
        Shape::Arrow(Arrow {
            start: center,
            end: center,
        }),
        Shape::Rectangle(Rectangle { rect: point }),
        Shape::Ellipse(Ellipse { rect: point }),
        Shape::Pen(Polyline {
            points: vec![center, center],
        }),
    ] {
        let result = flattened(image.clone(), [(shape.clone(), style(BLUE, 10.0))]);
        assert_covers(&image, &result, BLUE, |p| (p - center).length() - 5.0);

        let invisible = flattened(image.clone(), [(shape, style(BLUE, 0.0))]);
        assert!(invisible == image, "zero width drew something");
    }
}

#[test]
fn translucent_colors_blend_source_over_in_straight_alpha() {
    let clear = Image::filled(PhysicalSize::new(20, 20), Rgba8::new(0, 0, 0, 0));
    let white = Image::filled(PhysicalSize::new(20, 20), Rgba8::rgb(255, 255, 255));
    let red = Rgba8::new(255, 0, 0, 128);
    let stroke = || [(line(0.0, 10.0, 20.0, 10.0), style(red, 8.0))];

    // Over nothing, the color itself: straight alpha, not darkened.
    let over_clear = flattened(clear, stroke()).pixel(10, 10).unwrap();
    assert_eq!(over_clear, red);
    // Over white, half of each.
    let over_white = flattened(white.clone(), stroke()).pixel(10, 10).unwrap();
    assert_eq!(over_white, Rgba8::rgb(255, 127, 127));
    // Two coats over white: the second blends over the first.
    let twice = flattened(white, stroke().into_iter().chain(stroke()))
        .pixel(10, 10)
        .unwrap();
    assert_eq!(twice, Rgba8::rgb(255, 63, 63));
}

#[test]
fn a_step_marker_is_a_disc_with_its_derived_number_on_it() {
    let image = base(60, 60);
    let center = Point::new(30.5, 29.0);
    let marker = |x| {
        Shape::Step(StepMarker {
            center: Point::new(x, center.y),
        })
    };
    let style = Style {
        color: BLUE,
        font_size: 20.0,
        ..Style::default()
    };
    // The second of two markers, the first moved off the image: it shows 2.
    let mut document = Document::new(image.clone());
    document.add(marker(-100.0), style);
    document.add(marker(center.x), style);
    let result = flatten(&document).unwrap();

    let radius = StepMarker::radius(style.font_size);
    let white = StepMarker::number_color(BLUE);
    let mut number = 0;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let p = Point::new(x as f32 + 0.5, y as f32 + 0.5);
            let d = p.distance(center) - radius;
            let pixel = result.pixel(x, y).unwrap();
            if d > EDGE {
                assert_eq!(pixel, image.pixel(x, y).unwrap(), "outside at ({x}, {y})");
            } else if d < -EDGE && d > -radius * 0.3 {
                // The rim, clear of the number.
                assert_eq!(pixel, BLUE, "rim at ({x}, {y})");
            }
            number += usize::from(pixel == white);
        }
    }
    assert!(number > 20, "{number} pixels of the number");

    // A lone marker shows 1: fewer pixels than "2", as a glyph check.
    let mut single = Document::new(image);
    single.add(marker(center.x), style);
    let one = flatten(&single).unwrap();
    let count = |image: &Image| {
        image
            .pixels()
            .chunks_exact(4)
            .filter(|p| *p == white.to_array())
            .count()
    };
    assert!(count(&one) < count(&result), "a 1 has less ink than a 2");
}

#[test]
fn a_highlighter_is_one_even_tint_where_it_overlaps_itself() {
    let white = Image::filled(PhysicalSize::new(40, 40), Rgba8::rgb(255, 255, 255));
    let highlight = |points: &[(f32, f32)]| {
        (
            Shape::Highlighter(Polyline {
                points: points.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            }),
            // 12 wide.
            style(BLUE, 3.0),
        )
    };
    // A "Z" folded back over itself: the stroke crosses itself at (20, 20)
    // and its joins overlap.
    let crossing = flattened(
        white.clone(),
        [highlight(&[
            (2.0, 2.0),
            (38.0, 38.0),
            (38.0, 2.0),
            (2.0, 38.0),
        ])],
    );
    let once = crossing.pixel(10, 10).unwrap();
    // White under blue at 40%, to within rounding.
    let tint = |c: u8| 255.0 * (1.0 - 0.4) + f32::from(c) * 0.4;
    for (got, want) in
        once.to_array()
            .into_iter()
            .zip([tint(BLUE.r), tint(BLUE.g), tint(BLUE.b), 255.0])
    {
        assert!((f32::from(got) - want).abs() <= 1.5, "{once:?}");
    }
    assert_eq!(
        crossing.pixel(20, 20).unwrap(),
        once,
        "not darker where it crosses"
    );
    assert_eq!(crossing.pixel(37, 37).unwrap(), once, "nor at the joins");

    // Two strokes are two coats.
    let two = flattened(
        white,
        [
            highlight(&[(2.0, 2.0), (38.0, 38.0)]),
            highlight(&[(38.0, 2.0), (2.0, 38.0)]),
        ],
    );
    assert_eq!(two.pixel(10, 10).unwrap(), once);
    assert_ne!(two.pixel(20, 20).unwrap(), once);
}

#[test]
fn later_annotations_are_drawn_over_earlier_ones() {
    let image = base(20, 20);
    let green = Rgba8::rgb(0, 200, 0);
    let result = flattened(
        image,
        [
            (line(0.0, 10.0, 20.0, 10.0), style(BLUE, 6.0)),
            (line(10.0, 0.0, 10.0, 20.0), style(green, 6.0)),
        ],
    );
    assert_eq!(result.pixel(10, 10), Some(green));
    assert_eq!(result.pixel(2, 10), Some(BLUE));
}

#[test]
fn text_is_drawn_in_its_color_only_around_its_layout_box() {
    let image = base(160, 60);
    let style = Style {
        color: BLUE,
        font_size: 36.0,
        ..Style::default()
    };
    let position = Point::new(10.25, 6.5);
    let content = "HIT\nll";
    let result = flattened(
        image.clone(),
        [(Shape::Text(Text::new(position, content)), style)],
    );
    let size = font::measure(content, style.font_size);
    // Glyph ink can overhang the advance box a little; nothing reaches
    // further than this.
    let reach = Rect::new(position, size).expand(style.font_size * 0.1);
    let mut solid = 0;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let pixel = result.pixel(x, y).unwrap();
            if !reach.contains(Point::new(x as f32 + 0.5, y as f32 + 0.5)) {
                assert_eq!(pixel, image.pixel(x, y).unwrap(), "at ({x}, {y})");
            }
            solid += usize::from(pixel == BLUE);
        }
    }
    // The bold stems of "HIT" and "ll" fully cover many pixels.
    assert!(solid > 150, "{solid} pixels in the text color");
}

/// Asserts that no channel of `a` differs from `b`'s by more than one.
fn assert_within_rounding(a: &Image, b: &Image) {
    assert_eq!(a.size(), b.size());
    let worst = a
        .pixels()
        .iter()
        .zip(b.pixels())
        .map(|(a, b)| a.abs_diff(*b))
        .max();
    assert!(worst <= Some(1), "differ by up to {worst:?}");
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

#[test]
fn a_blur_region_obscures_only_what_is_beneath_it_within_its_pixels() {
    let image = base(80, 60);
    let green = Rgba8::rgb(0, 200, 0);
    let below = (line(0.0, 30.0, 80.0, 30.0), style(BLUE, 6.0));
    let above = (line(40.0, 0.0, 40.0, 60.0), style(green, 6.0));
    // Edges at 10.4 and 50.6 round to pixels 10 to 51.
    let region = blur(10.4, 10.0, 50.6, 50.0, BlurMode::Pixelate);
    let pixels = PhysicalRect::new(10, 10, 41, 40);

    let result = flattened(
        image.clone(),
        [below.clone(), region.clone(), above.clone()],
    );
    let unblurred = flattened(image.clone(), [below.clone(), above.clone()]);
    let mut pixelated = flattened(image, [below]);
    chartreuse_imaging::pixelate(&mut pixelated, pixels, BlurRegion::PIXELATE_BLOCK);

    for y in 0..result.height() {
        for x in 0..result.width() {
            let pixel = result.pixel(x, y).unwrap();
            let inside = pixels.contains(PhysicalPoint::new(x as i32, y as i32));
            let near_above = (x as f32 + 0.5 - 40.0).abs() < 3.0 + EDGE;
            if !inside {
                assert_eq!(
                    pixel,
                    unblurred.pixel(x, y).unwrap(),
                    "outside at ({x}, {y})"
                );
            } else if near_above {
                // Drawn over the region, unobscured.
                if (x as f32 + 0.5 - 40.0).abs() < 3.0 - EDGE {
                    assert_eq!(pixel, green, "above at ({x}, {y})");
                }
            } else {
                assert_eq!(
                    pixel,
                    pixelated.pixel(x, y).unwrap(),
                    "inside at ({x}, {y})"
                );
            }
        }
    }
    // The line beneath is gone: its blocks mix it with the base.
    assert!((10..51).all(|x| result.pixel(x, 30).unwrap() != BLUE));
}

#[test]
fn a_blur_region_obscures_other_blur_regions_beneath_it() {
    let image = base(60, 40);
    let region = blur(5.0, 5.0, 45.0, 35.0, BlurMode::Gaussian);
    let result = flattened(
        image.clone(),
        [
            (line(0.0, 20.0, 60.0, 20.0), style(BLUE, 4.0)),
            blur(20.0, 0.0, 60.0, 40.0, BlurMode::Pixelate),
            region,
        ],
    );
    let mut expected = flattened(
        image,
        [
            (line(0.0, 20.0, 60.0, 20.0), style(BLUE, 4.0)),
            blur(20.0, 0.0, 60.0, 40.0, BlurMode::Pixelate),
        ],
    );
    chartreuse_imaging::blur(
        &mut expected,
        PhysicalRect::new(5, 5, 40, 30),
        BlurRegion::BLUR_RADIUS,
    );
    assert!(result == expected);
}

#[test]
fn obscured_gives_a_region_exactly_the_pixels_flatten_does() {
    let image = base(90, 60);
    let text = Shape::Text(Text::new(Point::new(30.0, 20.0), "Hi"));
    let text_style = Style {
        color: BLUE,
        font_size: 24.0,
        ..Style::default()
    };
    // The region blurs across a pixelated region below it, which reads
    // pixels well outside the top region (the text and the stroke among
    // them), so obscuring it takes flattening more than its own pixels.
    let stray = line(2.0, 57.0, 10.0, 57.0);
    let shapes = [
        (stray.clone(), style(BLUE, 2.0)),
        (text, text_style),
        (
            line(0.0, 50.0, 90.0, 10.0),
            style(Rgba8::rgb(0, 200, 0), 5.0),
        ),
        blur(20.0, 5.0, 70.0, 45.0, BlurMode::Pixelate),
        (line(0.0, 5.0, 90.0, 55.0), style(BLUE, 3.0)),
        (
            Shape::Step(StepMarker {
                center: Point::new(80.0, 50.0),
            }),
            text_style,
        ),
    ];
    let (top, mode) = (
        BlurRegion {
            rect: Rect::from_corners(Point::new(55.0, 30.0), Point::new(88.0, 58.0)),
        },
        BlurMode::Gaussian,
    );
    let mut document = Document::new(image.clone());
    for (shape, style) in shapes.iter().cloned() {
        document.add(shape, style);
    }
    let below: Vec<_> = document
        .annotations()
        .iter()
        .map(|annotation| Drawn::of(&document, annotation))
        .collect();
    let mut full = document.clone();
    full.add(
        Shape::Blur(top.clone()),
        Style {
            blur: mode,
            ..Style::default()
        },
    );
    let flat = flatten(&full).unwrap();

    let (pixels, shown) = obscured(&image, &below, &top, mode).unwrap();
    assert_eq!(pixels, PhysicalRect::new(55, 30, 33, 28));
    let expected = chartreuse_imaging::crop(&flat, pixels).unwrap();
    assert_within_rounding(&shown, &expected);

    // What can affect it: everything but the stray line, far from both
    // regions.
    let beneath = beneath(&image, &below, &top);
    assert_eq!(beneath.len(), below.len() - 1);
    assert!(beneath.iter().all(|drawn| *drawn.shape != stray));
    let (_, from_beneath) = obscured(&image, &beneath, &top, mode).unwrap();
    assert!(from_beneath == shown);

    // Off the image: nothing.
    let off = BlurRegion {
        rect: Rect::from_corners(Point::new(-20.0, 0.0), Point::new(-1.0, 10.0)),
    };
    assert!(obscured(&image, &below, &off, mode).is_none());
}

#[test]
fn a_crop_cuts_the_flattened_image_down_last() {
    let image = base(60, 40);
    let mut document = Document::new(image.clone());
    let (shape, pixelate) = blur(0.0, 0.0, 60.0, 40.0, BlurMode::Pixelate);
    document.add(line(0.0, 20.0, 60.0, 20.0), style(BLUE, 4.0));
    // A pixelated region over the whole image: its grid starts at the
    // image's corner, not the crop's, so the crop is of the result.
    document.add(shape, pixelate);
    let whole = flatten(&document).unwrap();

    let crop = |document: &mut Document, ax, ay, bx, by| {
        let rect = Rect::from_corners(Point::new(ax, ay), Point::new(bx, by));
        document.apply(Command::SetCrop(Some(rect)));
        flatten(document).unwrap()
    };
    let cropped = crop(&mut document, 10.0, 5.0, 45.0, 30.0);
    assert!(cropped == chartreuse_imaging::crop(&whole, PhysicalRect::new(10, 5, 35, 25)).unwrap());
    // Rounded to whole pixels and clipped to the image.
    let clipped = crop(&mut document, 49.6, -8.0, 90.0, 12.4);
    assert!(clipped == chartreuse_imaging::crop(&whole, PhysicalRect::new(50, 0, 10, 12)).unwrap());
    // Entirely outside, or uncropped: the whole image.
    assert!(crop(&mut document, 100.0, 0.0, 120.0, 10.0) == whole);
    document.apply(Command::SetCrop(None));
    assert!(flatten(&document).unwrap() == whole);

    // With no annotations too.
    let mut plain = Document::new(image.clone());
    let cropped = crop(&mut plain, 0.0, 0.0, 20.0, 10.0);
    assert!(cropped == chartreuse_imaging::crop(&image, PhysicalRect::new(0, 0, 20, 10)).unwrap());
}

#[test]
fn an_empty_image_flattens_to_itself() {
    let empty = Image::filled(PhysicalSize::new(0, 0), BLUE);
    let result = flattened(
        empty.clone(),
        [(line(0.0, 0.0, 5.0, 5.0), style(BLUE, 4.0))],
    );
    assert!(result == empty);
}

mod canvas {
    //! Flatten against the canvas itself, drawn headlessly by iced's
    //! tiny-skia renderer at 1:1.

    use iced::advanced::renderer::{self, Headless};
    use iced::futures::executor::block_on;
    use iced::{mouse, Color, Renderer, Theme};
    use iced_runtime::user_interface::{self, UserInterface};

    use super::*;
    use crate::canvas::InputKind;
    use crate::canvas::MARGIN;
    use crate::editor::testing::{self, at, click, drag, input, named, press, type_text};
    use crate::tools::ToolKind;
    use crate::{Editor, Message};

    /// The canvas at [`testing::CANVAS`], cut down to the image (which the
    /// fitted view shows 1:1, [`MARGIN`] from the canvas's corner).
    fn canvas_image(editor: &Editor) -> Image {
        let mut renderer = block_on(<Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .expect("a tiny-skia renderer");
        let mut ui = UserInterface::build(
            crate::canvas::view(editor),
            testing::CANVAS,
            user_interface::Cache::default(),
            &mut renderer,
        );
        ui.draw(
            &mut renderer,
            &Theme::Dark,
            &renderer::Style {
                text_color: Color::WHITE,
            },
            mouse::Cursor::Unavailable,
        );
        let canvas_width = testing::CANVAS.width as u32;
        let size = iced::Size::new(canvas_width, testing::CANVAS.height as u32);
        let screenshot = renderer.screenshot(size, 1.0, Color::BLACK);
        let base = editor.document().base();
        let margin = MARGIN as u32;
        Image::from_fn(base.size(), |x, y| {
            let i = (((y + margin) * canvas_width + x + margin) * 4) as usize;
            let [r, g, b, a] = screenshot[i..i + 4] else {
                unreachable!()
            };
            Rgba8::new(r, g, b, a)
        })
    }

    /// Ends a gesture's aftermath: the new annotation is selected, and a
    /// style change would restyle it.
    fn deselect(editor: &mut Editor) {
        named(editor, iced::keyboard::key::Named::Escape);
        assert_eq!(editor.document().selected().count(), 0);
    }

    fn draw(
        editor: &mut Editor,
        tool: ToolKind,
        color: Rgba8,
        width: f32,
        from: iced::Point,
        to: iced::Point,
    ) {
        editor.update(Message::Color(color));
        editor.update(Message::StrokeWidth(width));
        editor.update(Message::Tool(tool));
        drag(editor, from, to);
        deselect(editor);
    }

    /// A freehand stroke with `tool` through canvas `points`.
    fn stroke(
        editor: &mut Editor,
        tool: ToolKind,
        color: Rgba8,
        width: f32,
        points: &[iced::Point],
    ) {
        editor.update(Message::Color(color));
        editor.update(Message::StrokeWidth(width));
        editor.update(Message::Tool(tool));
        let [first, rest @ ..] = points else {
            unreachable!()
        };
        press(editor, *first, 1);
        for &position in rest {
            input(editor, InputKind::Move { position });
        }
        input(
            editor,
            InputKind::Release {
                position: points[points.len() - 1],
            },
        );
        deselect(editor);
    }

    /// Canvas points along a wave from document `(x0, y)` to `(x1, y)`.
    fn wave(x0: f32, x1: f32, y: f32, amplitude: f32) -> Vec<iced::Point> {
        (0..=60)
            .map(|i| {
                let t = i as f32 / 60.0;
                at(x0 + (x1 - x0) * t, y + amplitude * (t * 12.0).sin())
            })
            .collect()
    }

    #[test]
    fn flatten_matches_the_canvas_at_actual_size() {
        // iced's tiny-skia renderer draws the canvas's base image one pixel
        // left of where it belongs: it rotates the image's corner around its
        // center by -2π in f32 (`Rectangle::with_vertices`), getting x =
        // 15.99997 instead of 16, and truncates that to an integer. So the
        // base varies only down the image, where the shift doesn't show, and
        // the last column (backdrop on the canvas) is not compared.
        let mut editor = Editor::new(Image::from_fn(PhysicalSize::new(400, 300), |_, y| {
            Rgba8::rgb((y * 7 % 256) as u8, (255 - y * 3 / 4) as u8, 200)
        }));
        let yellow = Rgba8::rgb(255, 214, 10);
        let translucent = Rgba8::new(40, 220, 90, 150);
        draw(
            &mut editor,
            ToolKind::Line,
            yellow,
            7.0,
            at(20.0, 30.5),
            at(180.25, 90.0),
        );
        draw(
            &mut editor,
            ToolKind::Rectangle,
            Rgba8::rgb(255, 59, 48),
            5.0,
            at(60.0, 40.0),
            at(240.5, 160.0),
        );
        draw(
            &mut editor,
            ToolKind::Arrow,
            translucent,
            12.0,
            at(30.0, 250.0),
            at(300.0, 120.0),
        );
        draw(
            &mut editor,
            ToolKind::Ellipse,
            Rgba8::rgb(10, 132, 255),
            6.0,
            at(250.0, 20.0),
            at(390.25, 110.5),
        );
        stroke(
            &mut editor,
            ToolKind::Pen,
            Rgba8::rgb(175, 82, 222),
            4.0,
            &wave(20.0, 200.0, 280.0, 12.0),
        );
        // A highlighter looping back over itself and across the first line.
        let mut loop_back = wave(30.0, 170.0, 60.0, 25.0);
        loop_back.extend(wave(170.0, 40.0, 70.0, -20.0));
        stroke(&mut editor, ToolKind::Highlighter, yellow, 5.0, &loop_back);
        // And one clipped by the image's bottom-right corner.
        stroke(
            &mut editor,
            ToolKind::Highlighter,
            Rgba8::rgb(10, 132, 255),
            4.0,
            &wave(320.0, 396.0, 292.0, 5.0),
        );
        // Three step markers, the middle one deleted: the last shows 2.
        editor.update(Message::Color(Rgba8::rgb(255, 204, 0)));
        editor.update(Message::FontSize(20.0));
        editor.update(Message::Tool(ToolKind::Step));
        for x in [260.0, 300.0, 340.25] {
            click(&mut editor, at(x, 150.5));
        }
        editor.update(Message::Tool(ToolKind::Select));
        click(&mut editor, at(300.0, 150.5));
        editor.update(Message::Delete);

        editor.update(Message::Color(Rgba8::rgb(250, 250, 250)));
        editor.update(Message::FontSize(30.0));
        editor.update(Message::Tool(ToolKind::Text));
        click(&mut editor, at(150.3, 180.6));
        type_text(&mut editor, "Flatten Åg\nmatches!");
        named(&mut editor, iced::keyboard::key::Named::Escape);
        deselect(&mut editor);
        // A translucent stroke over the text, so z-order across the canvas's
        // layers is compared too.
        draw(
            &mut editor,
            ToolKind::Line,
            translucent,
            10.0,
            at(140.0, 200.0),
            at(380.0, 230.0),
        );
        assert_eq!(editor.document().annotations().len(), 11);

        let flat = flatten(editor.document()).unwrap();
        let canvas = canvas_image(&editor);
        let (mut worst, mut differ) = (0, 0);
        for y in 0..flat.height() {
            for x in 0..flat.width() - 1 {
                let (a, b) = (flat.pixel(x, y).unwrap(), canvas.pixel(x, y).unwrap());
                let diff = a
                    .to_array()
                    .iter()
                    .zip(b.to_array())
                    .map(|(a, b)| a.abs_diff(b))
                    .max()
                    .unwrap();
                worst = worst.max(diff);
                differ += usize::from(diff > 2);
            }
        }
        // Rounding in blending leaves every pixel within 2; the stroker's
        // float math, run at different offsets, can shade a stray edge pixel
        // (measured: one, by 11) differently.
        assert!(
            worst <= 16 && differ <= 8,
            "worst channel difference {worst}, {differ} pixels differ by more than 2"
        );
    }
}
