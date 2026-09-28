//! The editor's toolbar: tools, crop controls, style controls (including how
//! blur regions obscure), undo and redo, and zoom.
//!
//! Its accent (the active tool, the chosen color swatch) is the theme's
//! primary color; the app's theme sets that to the build flavor's accent.

use std::fmt;

use chartreuse_core::color::Rgba8;
use iced::widget::{button, pick_list, row, space, text, tooltip, Row};
use iced::{Alignment, Background, Border, Element, Theme};

use crate::canvas;
use crate::editor::{Message, ZoomChange};
use crate::model::BlurMode;
use crate::tools::{Preview, ToolKind};
use crate::Editor;

/// The color swatches, in order.
pub const COLORS: [Rgba8; 8] = [
    Rgba8::from_rgb_hex(0xff_3b_30),
    Rgba8::from_rgb_hex(0xff_95_00),
    Rgba8::from_rgb_hex(0xff_cc_00),
    Rgba8::from_rgb_hex(0x34_c7_59),
    Rgba8::from_rgb_hex(0x00_7a_ff),
    Rgba8::from_rgb_hex(0xaf_52_de),
    Rgba8::from_rgb_hex(0x00_00_00),
    Rgba8::from_rgb_hex(0xff_ff_ff),
];

/// The stroke widths on offer, in image pixels.
pub const STROKE_WIDTHS: [f32; 9] = [1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0];

/// The font sizes on offer, in image pixels.
pub const FONT_SIZES: [f32; 9] = [12.0, 16.0, 20.0, 24.0, 32.0, 40.0, 48.0, 64.0, 96.0];

const SWATCH: f32 = 18.0;
const GROUP_SPACING: f32 = 16.0;
const ITEM_SPACING: f32 = 4.0;

/// A size in image pixels, as a pick-list entry.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pixels(f32);

impl fmt::Display for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} px", self.0)
    }
}

const fn pixels<const N: usize>(values: [f32; N]) -> [Pixels; N] {
    let mut out = [Pixels(0.0); N];
    let mut i = 0;
    while i < N {
        out[i] = Pixels(values[i]);
        i += 1;
    }
    out
}

const STROKE_OPTIONS: [Pixels; STROKE_WIDTHS.len()] = pixels(STROKE_WIDTHS);
const FONT_OPTIONS: [Pixels; FONT_SIZES.len()] = pixels(FONT_SIZES);

/// The toolbar for `editor`.
pub(crate) fn toolbar(editor: &Editor) -> Element<'_, Message> {
    let style = editor.style();
    let document = editor.document();

    let tools = group(ToolKind::ALL.into_iter().map(|kind| {
        let active = kind == editor.tool();
        let hint = format!("{} ({})", kind.label(), kind.hotkey().to_ascii_uppercase());
        tooltip(
            button(text(kind.label()))
                .on_press(Message::Tool(kind))
                .style(if active {
                    button::primary
                } else {
                    button::secondary
                }),
            text(hint),
            tooltip::Position::Bottom,
        )
        .into()
    }));

    // While cropping: apply (by switching back to the select tool, which
    // applies the crop being edited) and clear. Otherwise clear, if cropped.
    let cropping = editor.tool() == ToolKind::Crop;
    let editing_crop = matches!(editor.active_tool().preview(), Preview::Crop(Some(_)));
    let mut crop = Vec::new();
    if cropping {
        crop.push(
            button(text("Apply crop"))
                .on_press(Message::Tool(ToolKind::Select))
                .style(button::primary)
                .into(),
        );
    }
    if cropping || document.crop().is_some() {
        crop.push(
            button(text("Clear crop"))
                .on_press_maybe(
                    (editing_crop || document.crop().is_some()).then_some(Message::ClearCrop),
                )
                .style(button::secondary)
                .into(),
        );
    }
    let crop = (!crop.is_empty()).then(|| group(crop));

    let colors = group(
        COLORS
            .into_iter()
            .map(|color| swatch(color, color == style.color)),
    );

    let sizes = group([
        text("Stroke").into(),
        pick_list(
            &STROKE_OPTIONS[..],
            Some(Pixels(style.stroke_width)),
            |Pixels(width)| Message::StrokeWidth(width),
        )
        .into(),
        text("Font").into(),
        pick_list(
            &FONT_OPTIONS[..],
            Some(Pixels(style.font_size)),
            |Pixels(size)| Message::FontSize(size),
        )
        .into(),
    ]);

    let blur = group(BlurMode::ALL.into_iter().map(|mode| {
        button(text(mode.label()))
            .on_press(Message::BlurMode(mode))
            .style(if mode == style.blur {
                button::primary
            } else {
                button::secondary
            })
            .into()
    }));

    let history = group([
        button(text("Undo"))
            .on_press_maybe(document.can_undo().then_some(Message::Undo))
            .style(button::secondary)
            .into(),
        button(text("Redo"))
            .on_press_maybe(document.can_redo().then_some(Message::Redo))
            .style(button::secondary)
            .into(),
    ]);

    let scale = editor.viewport(editor.canvas_size()).scale();
    let zoom = group([
        button(text("−"))
            .on_press(Message::Zoom(ZoomChange::Out))
            .style(button::secondary)
            .into(),
        text(format!("{:.0}%", scale * 100.0)).into(),
        button(text("+"))
            .on_press(Message::Zoom(ZoomChange::In))
            .style(button::secondary)
            .into(),
        button(text("Fit"))
            .on_press(Message::Zoom(ZoomChange::Fit))
            .style(button::secondary)
            .into(),
    ]);

    row![tools]
        .push(crop)
        .extend([colors, sizes, blur, history, zoom].map(Element::from))
        .spacing(GROUP_SPACING)
        .padding(8)
        .align_y(Alignment::Center)
        .wrap()
        .vertical_spacing(8)
        .into()
}

/// Controls laid out as one toolbar group.
fn group<'a>(items: impl IntoIterator<Item = Element<'a, Message>>) -> Row<'a, Message> {
    Row::with_children(items)
        .spacing(ITEM_SPACING)
        .align_y(Alignment::Center)
}

/// A color swatch button, ringed in the accent when `chosen`.
fn swatch<'a>(color: Rgba8, chosen: bool) -> Element<'a, Message> {
    let fill = canvas::color(color);
    button(space().width(SWATCH).height(SWATCH))
        .padding(0)
        .on_press(Message::Color(color))
        .style(move |theme: &Theme, status| {
            let palette = theme.extended_palette();
            let ring = if chosen {
                Border {
                    color: palette.primary.base.color,
                    width: 3.0,
                    radius: 4.0.into(),
                }
            } else {
                Border {
                    color: match status {
                        button::Status::Hovered => palette.background.base.text,
                        _ => palette.background.strong.color,
                    },
                    width: 1.0,
                    radius: 4.0.into(),
                }
            };
            button::Style {
                background: Some(Background::Color(fill)),
                border: ring,
                ..button::Style::default()
            }
        })
        .into()
}
