//! The editor's toolbar, in two lines. The first has the tools, in their
//! [groups](ToolKind::GROUPS), with undo and redo, which act on the document
//! rather than the tool, in a group of their own after the select tool. The
//! second has the active tool's options: crop and resize controls, and the
//! style controls (color, stroke width, font size, text background, and how
//! blur regions obscure), with Clear crop at its right end while the
//! document is cropped. Also the zoom controls, which the editor's owner
//! places ([`Editor::zoom_controls`]).
//!
//! # Style controls
//!
//! The style controls restyle the selected annotations (each field only where
//! the kind draws with it; see [`Shape::style_fields`]) and set the style for
//! new ones. A control shows only if it applies to something in play: the
//! selected annotations, or the ones the active tool makes (none, with the
//! select, crop, or resize tool). It shows the value the selected
//! annotations it applies to share (nothing, if they differ), or else the
//! style for new annotations.
//!
//! The stroke width, corner radius, and font size each offer a list of
//! common sizes and a spinner, whose arrows step the size by one pixel
//! within [`STROKE_RANGE`], [`RADIUS_RANGE`], or [`FONT_RANGE`]. The arrows
//! are disabled while the selected annotations' sizes differ.
//!
//! Text's background has its own swatches, for its color, ending with
//! Transparent (black at 0% opacity, whatever the opacity chosen), and a
//! list of [`OPACITIES`]; each changes only its part of the background, so
//! the opacity kept while transparent applies again once a color is chosen.
//! A background that shows nothing (transparent, or any color at 0%) shows
//! as Transparent.
//!
//! [`Shape::style_fields`]: crate::model::Shape::style_fields
//!
//! Its accent (the active tool, the chosen color swatch) is the theme's
//! primary color; the app's theme sets that to the build flavor's accent.

use std::fmt;
use std::ops::RangeInclusive;

use chartreuse_core::color::Rgba8;
use iced::widget::{
    button, checkbox, column, container, pick_list, row, space, svg, text, text_input, tooltip, Row,
};
use iced::{Alignment, Background, Border, Element, Length, Padding, Theme};

use crate::canvas;
use crate::editor::{Message, ZoomChange};
use crate::model::{Annotation, BlurMode, Document, Style, StyleFields, TextBackground};
use crate::tools::{Preview, ResizeInput, ResizeTool, ResizeUnit, ToolKind};
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

/// The rectangle corner radii on offer, in image pixels.
pub const CORNER_RADII: [f32; 9] = [0.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0];

/// The side of the toolbar's icons, in logical pixels.
const ICON_SIZE: f32 = 18.0;

/// The undo and redo buttons' icons (Lucide's undo-2 and redo-2).
const UNDO_ICON: &[u8] = include_bytes!("../assets/icons/undo.svg");
const REDO_ICON: &[u8] = include_bytes!("../assets/icons/redo.svg");

/// The red slash across the Transparent background swatch, kept inside the
/// chosen swatch's ring.
const TRANSPARENT_SLASH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 18 18"><line x1="3.5" y1="14.5" x2="14.5" y2="3.5" stroke="#ff3b30" stroke-width="2" stroke-linecap="round"/></svg>"##;

/// The font sizes on offer, in image pixels.
pub const FONT_SIZES: [f32; 9] = [12.0, 16.0, 20.0, 24.0, 32.0, 40.0, 48.0, 64.0, 96.0];

/// The stroke widths the spinner steps through, in image pixels.
pub const STROKE_RANGE: RangeInclusive<f32> = 1.0..=100.0;

/// The corner radii the spinner steps through, in image pixels.
pub const RADIUS_RANGE: RangeInclusive<f32> = 0.0..=100.0;

/// The font sizes the spinner steps through, in image pixels.
pub const FONT_RANGE: RangeInclusive<f32> = 1.0..=400.0;

const SWATCH: f32 = 18.0;
const GROUP_SPACING: f32 = 16.0;
const ITEM_SPACING: f32 = 4.0;
/// The space between the toolbar's lines, and between the rows a line
/// wraps into.
const LINE_SPACING: f32 = 8.0;

/// The width of the resize tool's fields.
const RESIZE_FIELD: f32 = 72.0;

/// The size of a spinner arrow button.
const ARROW_WIDTH: f32 = 20.0;
const ARROW_HEIGHT: f32 = 14.0;

/// The width of the current zoom between the zoom buttons: room for the
/// widest value ([`canvas::MAX_SCALE`], "3200%") with some to spare, so the
/// buttons stay put as the zoom changes.
const ZOOM_VALUE_WIDTH: f32 = 60.0;

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
const RADIUS_OPTIONS: [Pixels; CORNER_RADII.len()] = pixels(CORNER_RADII);
const FONT_OPTIONS: [Pixels; FONT_SIZES.len()] = pixels(FONT_SIZES);

/// The text background opacities on offer, in percent.
pub const OPACITIES: [u8; 11] = [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100];

/// An opacity as an alpha, as a pick-list entry shown in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Opacity(u8);

impl Opacity {
    /// `percent` (0 to 100) as the nearest alpha.
    const fn percent(percent: u8) -> Self {
        Self(((percent as u16 * 255 + 50) / 100) as u8)
    }
}

impl fmt::Display for Opacity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}%", (u16::from(self.0) * 100 + 127) / 255)
    }
}

const OPACITY_OPTIONS: [Opacity; OPACITIES.len()] = {
    let mut out = [Opacity(0); OPACITIES.len()];
    let mut i = 0;
    while i < OPACITIES.len() {
        out[i] = Opacity::percent(OPACITIES[i]);
        i += 1;
    }
    out
};

/// What a style control shows (see the [module docs](self#style-controls)).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Control<T> {
    /// It applies to nothing in play.
    Hidden,
    /// Its value, or `None` if the selected annotations' values differ.
    Shown(Option<T>),
}

/// The style controls' state.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Panel {
    color: Control<Rgba8>,
    stroke_width: Control<f32>,
    corner_radius: Control<f32>,
    font_size: Control<f32>,
    blur: Control<BlurMode>,
    /// The text background's color, or transparent if it shows nothing
    /// (transparent, or any color at 0% opacity).
    text_background: Control<TextBackground>,
    /// The text background's opacity (alpha).
    text_background_opacity: Control<u8>,
}

impl Panel {
    /// The style controls for `editor` (see the
    /// [module docs](self#style-controls)).
    fn of(editor: &Editor) -> Self {
        let tool = editor.tool();
        let selected: Vec<_> = match tool {
            ToolKind::Crop | ToolKind::Resize => Vec::new(),
            _ => editor.document().selected().collect(),
        };
        let made = tool.style_fields();
        let style = editor.style();
        Self {
            color: control(&selected, made, |f| f.color, &style, |s| s.color),
            stroke_width: control(
                &selected,
                made,
                |f| f.stroke_width,
                &style,
                |s| s.stroke_width,
            ),
            corner_radius: control(
                &selected,
                made,
                |f| f.corner_radius,
                &style,
                |s| s.corner_radius,
            ),
            font_size: control(&selected, made, |f| f.font_size, &style, |s| s.font_size),
            blur: control(&selected, made, |f| f.blur, &style, |s| s.blur),
            text_background: control(
                &selected,
                made,
                |f| f.text_background,
                &style,
                |s| {
                    if s.text_background_fill().a == 0 {
                        TextBackground::Transparent
                    } else {
                        s.text_background
                    }
                },
            ),
            text_background_opacity: control(
                &selected,
                made,
                |f| f.text_background,
                &style,
                |s| s.text_background_opacity,
            ),
        }
    }
}

/// One control: the value `field` of `selected`'s annotations that `uses` it
/// share, else `style`'s if the tool's new annotations (`made`) use it.
fn control<T: Copy + PartialEq>(
    selected: &[&Annotation],
    made: StyleFields,
    uses: impl Fn(StyleFields) -> bool,
    style: &Style,
    field: impl Fn(&Style) -> T,
) -> Control<T> {
    let mut values = selected
        .iter()
        .filter(|annotation| uses(annotation.shape.style_fields()))
        .map(|annotation| field(&annotation.style));
    match values.next() {
        Some(first) => Control::Shown(values.all(|value| value == first).then_some(first)),
        None if uses(made) => Control::Shown(Some(field(style))),
        None => Control::Hidden,
    }
}

/// The toolbar for `editor` (see the [module docs](self)).
pub(crate) fn toolbar(editor: &Editor) -> Element<'_, Message> {
    let document = editor.document();
    let panel = Panel::of(editor);

    let history = group([
        icon_button(
            UNDO_ICON,
            button::secondary,
            document.can_undo().then_some(Message::Undo),
            "Undo".into(),
        ),
        icon_button(
            REDO_ICON,
            button::secondary,
            document.can_redo().then_some(Message::Redo),
            "Redo".into(),
        ),
    ]);

    let tool_group = |kinds: &[ToolKind]| -> Element<'_, Message> {
        group(
            kinds
                .iter()
                .map(|&kind| tool_button(kind, kind == editor.tool())),
        )
        .into()
    };
    // Undo and redo sit in their own group after the select tool's.
    let [select, rest @ ..] = ToolKind::GROUPS;
    let tools = Row::with_children(
        [tool_group(select), history.into()]
            .into_iter()
            .chain(rest.map(tool_group)),
    )
    .spacing(GROUP_SPACING)
    .align_y(Alignment::Center);

    // While cropping: apply (by switching back to the select tool, which
    // applies the crop being edited) and clear. Otherwise clear, if cropped,
    // at the second line's right end.
    let cropping = editor.tool() == ToolKind::Crop;
    let editing_crop = matches!(editor.active_tool().preview(), Preview::Crop(Some(_)));
    let clear_crop = (cropping || document.crop().is_some()).then(|| {
        button(text("Clear crop"))
            .on_press_maybe(
                (editing_crop || document.crop().is_some()).then_some(Message::ClearCrop),
            )
            .style(button::secondary)
            .into()
    });
    let (crop, clear_crop) = if cropping {
        let apply = button(text("Apply crop"))
            .on_press(Message::Tool(ToolKind::Select))
            .style(button::primary)
            .into();
        (Some(group([apply].into_iter().chain(clear_crop))), None)
    } else {
        (None, clear_crop)
    };
    let resize = editor.active_tool().as_resize().map(resize_controls);

    let colors = shown(panel.color).map(|chosen| {
        group(
            COLORS
                .into_iter()
                .map(|color| swatch(color, Some(color) == chosen, Message::Color(color))),
        )
    });

    let mut sizes = Vec::new();
    if let Some(width) = shown(panel.stroke_width) {
        sizes.push(text("Stroke").into());
        sizes.push(
            pick_list(&STROKE_OPTIONS[..], width.map(Pixels), |Pixels(width)| {
                Message::StrokeWidth(width)
            })
            .placeholder("Mixed")
            .into(),
        );
        sizes.push(spinner(width, STROKE_RANGE, Message::StrokeWidth));
    }
    if let Some(radius) = shown(panel.corner_radius) {
        sizes.push(text("Radius").into());
        sizes.push(
            pick_list(&RADIUS_OPTIONS[..], radius.map(Pixels), |Pixels(radius)| {
                Message::CornerRadius(radius)
            })
            .placeholder("Mixed")
            .into(),
        );
        sizes.push(spinner(radius, RADIUS_RANGE, Message::CornerRadius));
    }
    if let Some(size) = shown(panel.font_size) {
        sizes.push(text("Font").into());
        sizes.push(
            pick_list(&FONT_OPTIONS[..], size.map(Pixels), |Pixels(size)| {
                Message::FontSize(size)
            })
            .placeholder("Mixed")
            .into(),
        );
        sizes.push(spinner(size, FONT_RANGE, Message::FontSize));
    }
    let sizes = (!sizes.is_empty()).then(|| group(sizes));

    let background = shown(panel.text_background)
        .zip(shown(panel.text_background_opacity))
        .map(|(chosen, opacity)| {
            let swatches = COLORS
                .into_iter()
                .map(TextBackground::Color)
                .chain([TextBackground::Transparent])
                .map(|background| background_swatch(background, Some(background) == chosen));
            let opacity = pick_list(&OPACITY_OPTIONS[..], opacity.map(Opacity), |Opacity(a)| {
                Message::TextBackgroundOpacity(a)
            })
            .placeholder("Mixed");
            group(
                std::iter::once(text("Background").into())
                    .chain(swatches)
                    .chain([opacity.into()]),
            )
        });

    let blur = shown(panel.blur).map(|chosen| {
        group(BlurMode::ALL.into_iter().map(|mode| {
            button(text(mode.label()))
                .on_press(Message::BlurMode(mode))
                .style(if Some(mode) == chosen {
                    button::primary
                } else {
                    button::secondary
                })
                .into()
        }))
    });

    let options = Row::new()
        .push(crop)
        .push(resize)
        .push(colors)
        .push(sizes)
        .push(background)
        .push(blur)
        .spacing(GROUP_SPACING)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .wrap()
        .vertical_spacing(LINE_SPACING);

    column![
        tools.wrap().vertical_spacing(LINE_SPACING),
        row![
            // Keeps the line, when it has nothing to show, as tall as a
            // line of text buttons, so the canvas below stays put.
            container(text("")).padding(Padding {
                left: 0.0,
                right: 0.0,
                ..button::DEFAULT_PADDING
            }),
            Row::new()
                .push(options)
                .push(clear_crop)
                .spacing(GROUP_SPACING)
                .align_y(Alignment::Start),
        ],
    ]
    .spacing(LINE_SPACING)
    .padding(8)
    .into()
}

/// The button for the `kind` tool, highlighted if it is `active`.
fn tool_button(kind: ToolKind, active: bool) -> Element<'static, Message> {
    let look = if active {
        button::primary
    } else {
        button::secondary
    };
    let hint = format!("{} ({})", kind.label(), kind.hotkey().to_ascii_uppercase());
    icon_button(kind.icon(), look, Some(Message::Tool(kind)), hint)
}

/// A button showing the SVG `icon` in the `look` button style, sending
/// `on_press` (disabled if `None`), with `hint` as its tooltip.
fn icon_button(
    icon: &'static [u8],
    look: fn(&Theme, button::Status) -> button::Style,
    on_press: Option<Message>,
    hint: String,
) -> Element<'static, Message> {
    // The icon takes the button's text color, as a label would.
    let status = if on_press.is_some() {
        button::Status::Active
    } else {
        button::Status::Disabled
    };
    let icon = svg(svg::Handle::from_memory(icon))
        .width(ICON_SIZE)
        .height(ICON_SIZE)
        .style(move |theme: &Theme, _| svg::Style {
            color: Some(look(theme, status).text_color),
        });
    tooltip(
        button(icon).on_press_maybe(on_press).style(look),
        text(hint),
        tooltip::Position::Bottom,
    )
    .into()
}

/// The zoom controls for `editor`: zoom out, the current zoom, zoom in, and
/// fit.
pub(crate) fn zoom_controls(editor: &Editor) -> Element<'_, Message> {
    let scale = editor.viewport(editor.canvas_size()).scale();
    group([
        button(text("−"))
            .on_press(Message::Zoom(ZoomChange::Out))
            .style(button::secondary)
            .into(),
        text(format!("{:.0}%", scale * 100.0))
            .width(ZOOM_VALUE_WIDTH)
            .align_x(Alignment::Center)
            .into(),
        button(text("+"))
            .on_press(Message::Zoom(ZoomChange::In))
            .style(button::secondary)
            .into(),
        button(text("Fit"))
            .on_press(Message::Zoom(ZoomChange::Fit))
            .style(button::secondary)
            .into(),
    ])
    .into()
}

/// The resize tool's controls: the width and height fields, their unit, Keep
/// proportions, Apply, and the size the fields give.
fn resize_controls(tool: &ResizeTool) -> Row<'_, Message> {
    let field = |value: &str, input: fn(String) -> ResizeInput| {
        text_input("", value)
            .on_input(move |text| Message::Resize(input(text)))
            .on_submit(Message::Resize(ResizeInput::Apply))
            .width(RESIZE_FIELD)
            .into()
    };
    let units = ResizeUnit::ALL.into_iter().map(|unit| {
        button(text(unit.label()))
            .on_press(Message::Resize(ResizeInput::Unit(unit)))
            .style(if unit == tool.unit() {
                button::primary
            } else {
                button::secondary
            })
            .into()
    });
    let target = tool.target();
    let outcome = match target {
        Some(size) => format!("→ {} × {} px", size.width, size.height),
        None => format!("Sizes are 1 to {} px", Document::MAX_SIDE),
    };
    group(
        [
            text("Width").into(),
            field(tool.width(), ResizeInput::Width),
            text("Height").into(),
            field(tool.height(), ResizeInput::Height),
        ]
        .into_iter()
        .chain(units)
        .chain([
            checkbox(tool.proportional())
                .label("Keep proportions")
                .on_toggle(|on| Message::Resize(ResizeInput::Proportional(on)))
                .into(),
            button(text("Apply"))
                .on_press_maybe(target.map(|_| Message::Resize(ResizeInput::Apply)))
                .style(button::primary)
                .into(),
            text(outcome).into(),
        ]),
    )
}

/// A shown control's value; `None` if it is hidden.
const fn shown<T: Copy>(control: Control<T>) -> Option<Option<T>> {
    match control {
        Control::Hidden => None,
        Control::Shown(value) => Some(value),
    }
}

/// Controls laid out as one toolbar group.
fn group<'a>(items: impl IntoIterator<Item = Element<'a, Message>>) -> Row<'a, Message> {
    Row::with_children(items)
        .spacing(ITEM_SPACING)
        .align_y(Alignment::Center)
}

/// A color swatch button sending `message`, ringed in the accent when
/// `chosen`.
fn swatch<'a>(color: Rgba8, chosen: bool, message: Message) -> Element<'a, Message> {
    swatch_showing(color, space().width(SWATCH).height(SWATCH), chosen, message)
}

/// A text background swatch, ringed in the accent when `chosen`: a color, or
/// Transparent as black with a red slash.
fn background_swatch<'a>(background: TextBackground, chosen: bool) -> Element<'a, Message> {
    let message = Message::TextBackground(background);
    match background {
        TextBackground::Color(color) => swatch(color, chosen, message),
        TextBackground::Transparent => swatch_showing(
            Rgba8::BLACK,
            svg(svg::Handle::from_memory(TRANSPARENT_SLASH))
                .width(SWATCH)
                .height(SWATCH),
            chosen,
            message,
        ),
    }
}

/// A swatch button filled with `fill` under `content` (a swatch in size),
/// sending `message`, ringed in the accent when `chosen`.
fn swatch_showing<'a>(
    fill: Rgba8,
    content: impl Into<Element<'a, Message>>,
    chosen: bool,
    message: Message,
) -> Element<'a, Message> {
    let fill = canvas::color(fill);
    button(content)
        .padding(0)
        .on_press(message)
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

/// Up and down arrows that step `value` by one pixel within `range`, sending
/// `message` with the new size; disabled where there is no step to take
/// (see [`stepped`]).
fn spinner<'a>(
    value: Option<f32>,
    range: RangeInclusive<f32>,
    message: fn(f32) -> Message,
) -> Element<'a, Message> {
    let arrow = |label: &'a str, up: bool| {
        button(text(label).size(9).center())
            .padding(0)
            .width(ARROW_WIDTH)
            .height(ARROW_HEIGHT)
            .on_press_maybe(stepped(value, up, &range).map(message))
            .style(button::secondary)
    };
    column![arrow("▲", true), arrow("▼", false)]
        .spacing(2)
        .into()
}

/// `value` stepped up or down to the next whole pixel, if that stays within
/// `range`; `None` if there is no value (the selection's differ) or no step
/// to take.
fn stepped(value: Option<f32>, up: bool, range: &RangeInclusive<f32>) -> Option<f32> {
    let value = value?;
    if up {
        let next = (value.floor() + 1.0).min(*range.end());
        (next > value).then_some(next)
    } else {
        let next = (value.ceil() - 1.0).max(*range.start());
        (next < value).then_some(next)
    }
}

#[cfg(test)]
mod tests {
    use iced::keyboard::{self, key::Named};

    use super::*;
    use crate::editor::testing::{at, click, drag, editor, modifiers, named, type_text};

    const BLUE: Rgba8 = COLORS[4];
    const WHITE: Rgba8 = Rgba8::WHITE;

    /// A line in the default style and a blue text of font size 40, with
    /// nothing selected, back in the select tool.
    fn line_and_text() -> Editor {
        let mut editor = editor();
        editor.update(Message::Tool(ToolKind::Line));
        drag(&mut editor, at(10.0, 10.0), at(200.0, 10.0));
        named(&mut editor, Named::Escape);
        editor.update(Message::Color(BLUE));
        editor.update(Message::FontSize(40.0));
        editor.update(Message::Tool(ToolKind::Text));
        click(&mut editor, at(20.0, 100.0));
        type_text(&mut editor, "Hi");
        editor.update(Message::Tool(ToolKind::Select));
        named(&mut editor, Named::Escape);
        editor
    }

    /// Every style control hidden.
    const NO_CONTROLS: Panel = Panel {
        color: Control::Hidden,
        stroke_width: Control::Hidden,
        corner_radius: Control::Hidden,
        font_size: Control::Hidden,
        blur: Control::Hidden,
        text_background: Control::Hidden,
        text_background_opacity: Control::Hidden,
    };

    #[test]
    fn with_nothing_selected_the_select_tool_shows_no_style_controls() {
        assert_eq!(Panel::of(&line_and_text()), NO_CONTROLS);
    }

    #[test]
    fn a_selection_shows_what_its_kinds_use_and_the_values_they_share() {
        let mut editor = line_and_text();
        click(&mut editor, at(100.0, 10.0));
        let line = Panel::of(&editor);
        assert_eq!(line.color, Control::Shown(Some(Style::DEFAULT_COLOR)));
        let stroke = Style::default().stroke_width;
        assert_eq!(line.stroke_width, Control::Shown(Some(stroke)));
        assert_eq!(line.font_size, Control::Hidden, "not for a line");
        assert_eq!(line.corner_radius, Control::Hidden, "not for a line");
        assert_eq!(line.blur, Control::Hidden);
        assert_eq!(line.text_background, Control::Hidden);

        // Shift-click the text too: the colors differ; each size comes from
        // the one kind that has it.
        modifiers(&mut editor, keyboard::Modifiers::SHIFT);
        click(&mut editor, at(25.0, 110.0));
        assert_eq!(editor.document().selection().len(), 2);
        assert_eq!(
            Panel::of(&editor),
            Panel {
                color: Control::Shown(None),
                stroke_width: Control::Shown(Some(stroke)),
                corner_radius: Control::Hidden,
                font_size: Control::Shown(Some(40.0)),
                blur: Control::Hidden,
                text_background: Control::Shown(Some(TextBackground::Color(WHITE))),
                text_background_opacity: Control::Shown(Some(
                    Style::DEFAULT_TEXT_BACKGROUND_OPACITY
                )),
            }
        );

        // Changing the font size restyles only the text.
        editor.update(Message::FontSize(64.0));
        let [line, text] = editor.document().annotations() else {
            panic!("expected two annotations");
        };
        let unchanged = Style::default().font_size;
        assert_eq!(
            (line.style.font_size, text.style.font_size),
            (unchanged, 64.0)
        );

        // So does its background's opacity, keeping its color.
        editor.update(Message::TextBackgroundOpacity(0));
        let text = &editor.document().annotations()[1];
        assert_eq!(
            (
                text.style.text_background,
                text.style.text_background_opacity
            ),
            (TextBackground::Color(WHITE), 0)
        );
    }

    #[test]
    fn a_transparent_background_keeps_the_opacity_for_the_next_color() {
        let mut editor = line_and_text();
        click(&mut editor, at(25.0, 110.0));
        let opacity = Control::Shown(Some(Style::DEFAULT_TEXT_BACKGROUND_OPACITY));
        let shown = |editor: &Editor| {
            let panel = Panel::of(editor);
            (panel.text_background, panel.text_background_opacity)
        };
        let fill = |editor: &Editor| {
            editor.document().annotations()[1]
                .style
                .text_background_fill()
        };

        editor.update(Message::TextBackground(TextBackground::Transparent));
        assert_eq!(fill(&editor), Rgba8::TRANSPARENT);
        assert_eq!(
            shown(&editor),
            (Control::Shown(Some(TextBackground::Transparent)), opacity),
            "the opacity stays where it was"
        );

        editor.update(Message::TextBackground(TextBackground::Color(BLUE)));
        assert_eq!(
            fill(&editor),
            Rgba8 {
                a: Style::DEFAULT_TEXT_BACKGROUND_OPACITY,
                ..BLUE
            }
        );
        assert_eq!(
            shown(&editor),
            (Control::Shown(Some(TextBackground::Color(BLUE))), opacity)
        );

        // Any color at 0% shows nothing, so it shows as transparent.
        editor.update(Message::TextBackgroundOpacity(0));
        assert_eq!(
            shown(&editor),
            (
                Control::Shown(Some(TextBackground::Transparent)),
                Control::Shown(Some(0))
            )
        );
    }

    #[test]
    fn drawing_tools_show_what_they_make_along_with_the_selection() {
        let mut editor = line_and_text();
        editor.update(Message::Tool(ToolKind::Blur));
        let blur = Panel::of(&editor);
        assert_eq!(blur.blur, Control::Shown(Some(BlurMode::Pixelate)));
        assert_eq!(
            (blur.color, blur.stroke_width, blur.font_size),
            (Control::Hidden, Control::Hidden, Control::Hidden)
        );

        // A region just drawn is selected: blurring it shows as its mode.
        drag(&mut editor, at(100.0, 100.0), at(200.0, 200.0));
        editor.update(Message::BlurMode(BlurMode::Gaussian));
        editor.update(Message::Tool(ToolKind::Step));
        let step = Panel::of(&editor);
        assert_eq!(step.blur, Control::Shown(Some(BlurMode::Gaussian)));
        assert_eq!(step.font_size, Control::Shown(Some(40.0)), "for new steps");
        assert_eq!(step.stroke_width, Control::Hidden);
        assert_eq!(step.text_background, Control::Hidden, "steps have none");
    }

    #[test]
    fn new_rectangles_keep_the_last_corner_radius_chosen() {
        let mut editor = line_and_text();
        editor.update(Message::Tool(ToolKind::Rectangle));
        assert_eq!(
            Panel::of(&editor).corner_radius,
            Control::Shown(Some(Style::DEFAULT_CORNER_RADIUS))
        );
        // Rounding the rectangle just drawn (and selected) rounds the next.
        drag(&mut editor, at(50.0, 150.0), at(150.0, 250.0));
        editor.update(Message::CornerRadius(12.0));
        drag(&mut editor, at(200.0, 150.0), at(300.0, 250.0));
        let radii: Vec<_> = editor
            .document()
            .annotations()
            .iter()
            .map(|annotation| annotation.style.corner_radius)
            .collect();
        let unchanged = Style::DEFAULT_CORNER_RADIUS;
        assert_eq!(
            radii,
            [unchanged, unchanged, 12.0, 12.0],
            "line, text, rectangles"
        );
        assert_eq!(Panel::of(&editor).corner_radius, Control::Shown(Some(12.0)));
    }

    #[test]
    fn the_crop_tool_shows_no_style_controls() {
        let mut editor = line_and_text();
        click(&mut editor, at(100.0, 10.0));
        editor.update(Message::Tool(ToolKind::Crop));
        assert_eq!(Panel::of(&editor), NO_CONTROLS);
    }

    #[test]
    fn spinners_step_by_whole_pixels_within_their_range() {
        let range = 1.0..=100.0;
        assert_eq!(stepped(Some(8.0), true, &range), Some(9.0));
        assert_eq!(stepped(Some(8.0), false, &range), Some(7.0));
        assert_eq!(stepped(Some(2.5), true, &range), Some(3.0));
        assert_eq!(stepped(Some(2.5), false, &range), Some(2.0));
        assert_eq!(stepped(Some(1.0), false, &range), None, "the smallest");
        assert_eq!(stepped(Some(100.0), true, &range), None, "the largest");
        assert_eq!(stepped(None, true, &range), None, "mixed sizes");
    }
}
