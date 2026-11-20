//! What the screens are built from: the title a screen is known by, text
//! that is read, the buttons and their tips, boxes to tick, chips and
//! badges, menus, a select and a list of suggestions, the loading ring, an
//! account's picture. **Each has its name, its role and its state for a
//! screen reader built in**, so a screen cannot draw a control that is only
//! a shape.

use std::cell::Cell;
use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;

use gpui_kit::base::{
    Align, Button, ElementExt as _, ObservedElement, POPUP_PRIORITY, Placement, Positioner,
    TestSupportExt as _,
};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AnyView, App, Bounds, BoxShadow, Context, Div, ElementId, Entity, FocusHandle,
    FontWeight, MouseDownEvent, PathBuilder, Pixels, Point, Role, SharedString, Stateful,
    StyleRefinement, Text, Toggled, Window, canvas, deferred, div, img, point, px, svg,
};

use super::assets;
use super::look::{Look, color, color_at, sp};
use crate::backend::Account;
use crate::theme::fonts::SANS;
use crate::theme::motion::RING_TURN;
use crate::theme::tokens::{Palette, shape};

/// Text a screen reader reads as it is drawn, known by `id`.
pub fn said(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Text {
    Text::new(id.into(), text.into())
}

/// A username as every screen writes it: in the emphasis color, underlined
/// in pink. The size and weight are the caller's.
pub fn username(look: &Look) -> Div {
    let palette = look.palette;
    div()
        .text_color(color(palette.emphasis))
        .underline()
        .text_decoration_2()
        .text_decoration_color(color(palette.underline))
}

/// The focus ring: the accent's contour, and a contour inside it, so focus
/// is a change of outline and not of color alone.
pub fn focus_ring(palette: &Palette) -> impl Fn(StyleRefinement) -> StyleRefinement + 'static {
    let (outer, inner) = (color(palette.accent), color(palette.focus_inner));
    move |style| {
        let contour = |color, spread: f32| BoxShadow {
            color,
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(spread),
            inset: false,
        };
        style.shadow(vec![
            contour(outer, shape::FOCUS_INNER + shape::FOCUS_RING),
            contour(inner, shape::FOCUS_INNER),
        ])
    }
}

/// What a control that shows no words says when the pointer rests on it:
/// its own name. **One per control**: GPUI refuses a second, so a control
/// built here with its tip is never given another.
pub fn tip(label: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let label = label.into();
    move |window, cx| Tooltip::new(label.clone()).build(window, cx)
}

/// The title a screen is known by: its one heading of the first level, and
/// where focus goes when the screen comes in.
pub fn title(
    focus: &FocusHandle,
    text: impl Into<SharedString>,
    look: &Look,
) -> ObservedElement<Stateful<Div>> {
    let text = text.into();
    div()
        .id("title")
        .test_support()
        .role(Role::Heading)
        .aria_level(1)
        .aria_label(text.clone())
        .track_focus(focus)
        .font_weight(FontWeight::BOLD)
        .text_size(sp(30.))
        .line_height(sp(38.))
        .text_color(color(look.palette.text))
        .text_center()
        .child(text)
}

/// A paragraph of text that is read.
pub fn paragraph(id: impl Into<ElementId>, text: impl Into<SharedString>, look: &Look) -> Div {
    div()
        .font_family(SANS)
        .text_size(sp(15.))
        .line_height(sp(22.))
        .text_color(color(look.palette.secondary))
        .text_center()
        .child(said(id, text))
}

/// How tall the toolkit's large text field is, which a button beside one
/// matches.
pub const FIELD_HEIGHT: f32 = 44.0;

/// Which of the two buttons.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The one thing a screen is for: filled with the accent.
    Primary,
    /// Anything else: an outline.
    Secondary,
}

/// A button with its label. A disabled one is gray and dashed, never pink:
/// it is told apart by its outline as well as its color.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: Kind,
    enabled: bool,
    look: &Look,
) -> Button {
    let label = label.into();
    plain_button(id, label.clone(), kind, enabled, look).child(label)
}

/// A button that shows a drawing and no words: named all the same, for
/// whoever does not see the drawing, and its name is its [`tip`].
pub fn icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    drawing: &'static str,
    enabled: bool,
    look: &Look,
) -> Button {
    let ink = if enabled {
        look.palette.text
    } else {
        look.palette.disabled_text
    };
    let label = label.into();
    plain_button(id, label.clone(), Kind::Secondary, enabled, look)
        .w(sp(FIELD_HEIGHT))
        .h(sp(FIELD_HEIGHT))
        .px(px(0.))
        .tooltip(tip(label))
        .child(icon(drawing, 18.).text_color(color(ink)))
}

/// A button for a row of the settings or a card: lower and narrower than
/// [`button`], an outline all the same, and still at least 24 pixels square.
pub fn small_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    look: &Look,
) -> Button {
    let label = label.into();
    plain_button(id, label.clone(), Kind::Secondary, enabled, look)
        .h(sp(30.))
        .px(sp(12.))
        .gap(sp(6.))
        .text_size(sp(13.))
        .child(label)
}

/// A [`small_button`] with a drawing before its words.
pub fn small_icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    drawing: &'static str,
    look: &Look,
) -> Button {
    let label = label.into();
    plain_button(id, label.clone(), Kind::Secondary, true, look)
        .h(sp(30.))
        .px(sp(12.))
        .gap(sp(6.))
        .text_size(sp(13.))
        .child(icon(drawing, 14.).text_color(color(look.palette.text)))
        .child(label)
}

/// A choice of a few options side by side, one always on: a group a screen
/// reader names, holding [`segment`]s.
pub fn segmented(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    look: &Look,
) -> ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .role(Role::RadioGroup)
        .aria_label(label.into())
        .flex()
        .flex_none()
        .gap(px(2.))
        .p(px(3.))
        .rounded(px(shape::CONTROL_RADIUS + 3.))
        .border_1()
        .border_color(color(look.palette.control_line))
        .bg(color(look.palette.frame))
}

/// One option of a [`segmented`] choice. **The one on is told apart by
/// more than its color**: it is raised in the panel's color with an
/// outline, and its words are heavier.
pub fn segment(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    look: &Look,
) -> Button {
    let palette = look.palette;
    let label = label.into();
    Button::new(id)
        .role(Role::RadioButton)
        .accessibility_label(label.clone())
        .aria_toggled(if on {
            gpui_kit::Toggled::True
        } else {
            gpui_kit::Toggled::False
        })
        .h(sp(28.))
        .min_w(px(shape::MIN_TARGET))
        .px(sp(12.))
        .rounded(px(shape::CONTROL_RADIUS))
        .border_1()
        .text_size(sp(13.))
        .focus_visible(focus_ring(&palette))
        .cursor_pointer()
        .map(|segment| {
            if on {
                segment
                    .bg(color(palette.panel))
                    .border_color(color(palette.control_line))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color(palette.text))
            } else {
                segment
                    .border_color(gpui_kit::transparent_black())
                    .text_color(color(palette.secondary))
                    .hover(move |style| style.bg(color_at(palette.text, 0.06)))
            }
        })
        .child(label)
}

/// One option of a [`segmented`] choice drawn as `drawing` alone: named
/// all the same, for whoever does not see the drawing, and its name is its
/// [`tip`].
pub fn icon_segment(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    drawing: &'static str,
    on: bool,
    look: &Look,
) -> Button {
    let ink = if on {
        look.palette.text
    } else {
        look.palette.secondary
    };
    let label = label.into();
    Button::new(id)
        .role(Role::RadioButton)
        .accessibility_label(label.clone())
        .tooltip(tip(label))
        .aria_toggled(if on {
            gpui_kit::Toggled::True
        } else {
            gpui_kit::Toggled::False
        })
        .h(sp(28.))
        .min_w(sp(32.))
        .px(sp(8.))
        .rounded(px(shape::CONTROL_RADIUS))
        .border_1()
        .focus_visible(focus_ring(&look.palette))
        .cursor_pointer()
        .map(|segment| {
            let palette = look.palette;
            if on {
                segment
                    .bg(color(palette.panel))
                    .border_color(color(palette.control_line))
            } else {
                segment
                    .border_color(gpui_kit::transparent_black())
                    .hover(move |style| style.bg(color_at(palette.text, 0.06)))
            }
        })
        .child(icon(drawing, 16.).text_color(color(ink)))
}

/// A button's shape and states, with nothing in it yet: what a button that
/// shows a drawing beside its words is built from.
pub fn plain_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: Kind,
    enabled: bool,
    look: &Look,
) -> Button {
    let palette = look.palette;
    let button = Button::new(id)
        .accessibility_label(label)
        .disabled(!enabled)
        .h(sp(shape::BUTTON_HEIGHT + 4.))
        .min_w(px(shape::MIN_TARGET))
        .px(sp(20.))
        .gap(sp(8.))
        .rounded(px(shape::CONTROL_RADIUS))
        .font_weight(FontWeight::MEDIUM)
        .text_size(sp(14.))
        .border_1()
        .focus_visible(focus_ring(&palette));
    if !enabled {
        button
            .bg(color(palette.disabled_fill))
            .border_color(color(palette.control_line))
            .border_dashed()
            .text_color(color(palette.disabled_text))
    } else {
        match kind {
            Kind::Primary => button
                .bg(color(palette.accent))
                .border_color(color(palette.accent))
                .text_color(color(palette.on_accent))
                .hover(move |style| style.bg(color_at(palette.accent, 0.88)))
                .cursor_pointer(),
            Kind::Secondary => button
                .bg(color(palette.panel))
                .border_color(color(palette.control_line))
                .text_color(color(palette.text))
                .hover(move |style| style.bg(color(palette.frame)))
                .cursor_pointer(),
        }
    }
}

/// A drawing of the app's, colored by the caller, `size` logical pixels at
/// the default text size.
pub fn icon(path: &'static str, size: f32) -> gpui_kit::Svg {
    svg().path(path).size(sp(size)).flex_none()
}

/// The box of a [`checkbox`] or of a row that is chosen: filled with the
/// accent and ticked when on, an outline when not.
pub fn tick_box(on: bool, look: &Look) -> Div {
    let palette = look.palette;
    div()
        .flex_none()
        .size(sp(18.))
        .rounded(px(4.))
        .border_1()
        .border_color(color(palette.control_line))
        .bg(color(if on { palette.accent } else { palette.panel }))
        .flex()
        .items_center()
        .justify_center()
        .children(on.then(|| icon(assets::CHECK, 14.).text_color(color(palette.on_accent))))
}

fn toggled(on: bool) -> Toggled {
    if on { Toggled::True } else { Toggled::False }
}

/// A box that is ticked or not, and its words: a press anywhere on them
/// changes it.
pub fn checkbox(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    look: &Look,
) -> Button {
    let palette = look.palette;
    let label = label.into();
    Button::new(id)
        .role(Role::CheckBox)
        .accessibility_label(label.clone())
        .aria_toggled(toggled(on))
        .w_full()
        .min_h(sp(28.))
        .flex()
        .items_start()
        .justify_start()
        .gap(sp(10.))
        .rounded(px(shape::CONTROL_RADIUS))
        .focus_visible(focus_ring(&palette))
        .cursor_pointer()
        .text_size(sp(14.))
        .line_height(sp(21.))
        .text_color(color(palette.text))
        .child(tick_box(on, look).mt(px(2.)))
        .child(div().text_left().child(label))
}

/// A group of [`chip`]s, named for a screen reader, that wraps onto as many
/// lines as it needs.
pub fn chips(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    look: &Look,
) -> ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .role(Role::Group)
        .aria_label(label.into())
        .flex()
        .flex_wrap()
        .gap(sp(4.))
        .text_color(color(look.palette.text))
}

/// One of several things that are each on or off, as a day of the week of
/// a schedule: `label` is its whole name, `shown` the few letters drawn.
/// **The one on is told apart by more than its color**: a wash, an outline
/// in the text's color and heavier words.
pub fn chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    shown: impl Into<SharedString>,
    on: bool,
    look: &Look,
) -> Button {
    let palette = look.palette;
    Button::new(id)
        .role(Role::CheckBox)
        .accessibility_label(label)
        .aria_toggled(toggled(on))
        .h(sp(28.))
        .min_w(sp(36.))
        .px(sp(10.))
        .rounded_full()
        .border_1()
        .text_size(sp(13.))
        .focus_visible(focus_ring(&palette))
        .cursor_pointer()
        .map(|chip| {
            if on {
                chip.bg(color_at(palette.accent, 0.16))
                    .border_color(color(palette.text))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color(palette.text))
            } else {
                chip.bg(color(palette.panel))
                    .border_color(color(palette.control_line))
                    .text_color(color(palette.secondary))
                    .hover(move |style| style.bg(color_at(palette.text, 0.06)))
            }
        })
        .child(shown.into())
}

/// What a [`badge`] marks, which its outline repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Verified,
    Friend,
}

/// A word that marks an account: verified, private, a friend. **Its word
/// says it**; the outline's color only repeats it. Read as a label, `id`
/// telling it apart from a badge of the same word beside it, as on each row
/// of a list.
pub fn badge(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: Tone,
    look: &Look,
) -> Div {
    let palette = look.palette;
    let label = label.into();
    let outline = match tone {
        Tone::Plain => palette.line,
        Tone::Verified => palette.verified,
        Tone::Friend => palette.underline,
    };
    div()
        .flex_none()
        .px(sp(8.))
        .py(sp(2.))
        .rounded_full()
        .border_1()
        .border_color(color(outline))
        .text_size(sp(12.))
        .text_color(color(palette.secondary))
        .child(
            div()
                .id(id)
                .test_support()
                .role(Role::Label)
                .aria_label(label.clone())
                .child(label),
        )
}

/// What opens a menu, kept where it was last drawn so the menu opens under
/// it; while that menu is open, the menu follows it on the next frame.
pub fn drawn_at(drawn: &Rc<Cell<Bounds<Pixels>>>, open: bool, opener: impl IntoElement) -> Div {
    let drawn = drawn.clone();
    div()
        .flex_none()
        .on_prepaint(move |bounds, window, _| {
            if drawn.replace(bounds) != bounds && open {
                crate::ui::frames::ask(window);
            }
        })
        .child(opener)
}

/// An item of a small menu: a button across it, a drawing before its words.
pub fn menu_item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    drawing: &'static str,
    look: &Look,
) -> Button {
    let palette = look.palette;
    let label = label.into();
    Button::new(id)
        .role(Role::MenuItem)
        .accessibility_label(label.clone())
        .w_full()
        .h(sp(40.))
        .px(sp(10.))
        .gap(sp(10.))
        .justify_start()
        .rounded(px(shape::CONTROL_RADIUS))
        .text_size(sp(14.))
        .text_color(color(palette.text))
        .hover(move |style| style.bg(color(palette.frame)))
        .focus_visible(focus_ring(&palette))
        .cursor_pointer()
        .child(icon(drawing, 16.).text_color(color(palette.text)))
        .child(label)
}

/// The line between two groups of a menu's items.
pub fn menu_separator(look: &Look) -> Div {
    div().h(px(1.)).my(sp(6.)).bg(color(look.palette.line))
}

/// What a [`select`] keeps between frames: whether its options are open,
/// where its field was last drawn, and the focus of the field and of each
/// option.
pub struct Select {
    pub open: bool,
    pub bounds: Rc<Cell<Bounds<Pixels>>>,
    pub field: FocusHandle,
    pub options: Vec<FocusHandle>,
}

impl Select {
    /// A closed select of `options` options.
    pub fn new(options: usize, cx: &mut App) -> Self {
        Self {
            open: false,
            bounds: Rc::default(),
            field: cx.focus_handle(),
            options: (0..options).map(|_| cx.focus_handle()).collect(),
        }
    }
}

/// A list over everything else, under what opened it (`opener`, where that
/// was last drawn), as wide as it: what a [`select`] and [`suggestions`]
/// open. A press outside both closes it.
fn popup_list<V: 'static>(
    id: SharedString,
    label: SharedString,
    opener: &Rc<Cell<Bounds<Pixels>>>,
    rows: Vec<AnyElement>,
    look: &Look,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> AnyElement {
    let palette = look.palette;
    let under = opener.get();
    let opener = opener.clone();
    let outside = cx.listener(move |view, event: &MouseDownEvent, window, cx| {
        // A press on what opened it acts by itself.
        if !opener.get().contains(&event.position) {
            close(view, window, cx);
        }
    });
    deferred(
        Positioner::side(under)
            .placement(Placement::Bottom)
            .align(Align::Start)
            .offset(px(6.))
            .margin(px(8.))
            .occlude()
            .child(
                div()
                    .id(id)
                    .test_support()
                    .role(Role::ListBox)
                    .aria_label(label)
                    .occlude()
                    .w(under.size.width)
                    .p(sp(4.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .rounded(px(shape::CONTROL_RADIUS + 2.))
                    .border_1()
                    .border_color(color(palette.control_line))
                    .bg(color(palette.panel))
                    .shadow_lg()
                    .on_mouse_down_out(outside)
                    .children(rows),
            ),
    )
    .with_priority(POPUP_PRIORITY)
    .into_any_element()
}

/// One choice of a [`select`] or of [`suggestions`], named `label`; what it
/// shows is the caller's.
pub fn suggestion(id: impl Into<ElementId>, label: impl Into<SharedString>, look: &Look) -> Button {
    let palette = look.palette;
    option_row(id, label, look).hover(move |style| style.bg(color_at(palette.text, 0.08)))
}

/// A [`suggestion`] with no wash under the pointer, which the chosen option
/// of a [`select`] does without.
fn option_row(id: impl Into<ElementId>, label: impl Into<SharedString>, look: &Look) -> Button {
    let palette = look.palette;
    Button::new(id)
        .role(Role::ListBoxOption)
        .accessibility_label(label)
        .w_full()
        .min_h(sp(36.))
        .px(sp(10.))
        .gap(sp(8.))
        .justify_start()
        .rounded(px(shape::CONTROL_RADIUS))
        .text_size(sp(13.5))
        .text_color(color(palette.text))
        .focus_visible(focus_ring(&palette))
        .cursor_pointer()
}

/// The choices that match what is being typed in a field, drawn under it
/// (`field`, where it was last drawn, as [`drawn_at`] keeps it): rows of
/// [`suggestion`]s.
pub fn suggestions<V: 'static>(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    field: &Rc<Cell<Bounds<Pixels>>>,
    rows: Vec<AnyElement>,
    look: &Look,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> AnyElement {
    popup_list(id.into(), label.into(), field, rows, look, close, cx)
}

/// A field that opens a list of `options`, one of them `chosen`, for a
/// choice of more than a [`segmented`] row holds. `toggle` opens and closes
/// it, `choose` is told which option was pressed, `close` that a press fell
/// outside. The one chosen stands out by a wash, a bar and heavier words.
#[expect(
    clippy::too_many_arguments,
    reason = "a field, its options, and what each press does"
)]
pub fn select<V: 'static>(
    id: &'static str,
    label: impl Into<SharedString>,
    options: &[String],
    chosen: usize,
    state: &Select,
    look: &Look,
    toggle: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    choose: impl Fn(&mut V, usize, &mut Window, &mut Context<V>) + Clone + 'static,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Div {
    let palette = look.palette;
    let label = label.into();
    let open = state.open;
    let bounds = state.bounds.clone();
    let name = options.get(chosen).cloned().unwrap_or_default();
    let list = open.then(|| {
        let rows = options
            .iter()
            .enumerate()
            .map(|(at, option)| {
                let on = at == chosen;
                let choose = choose.clone();
                option_row(
                    (SharedString::from(format!("{id}-option")), at),
                    option.clone(),
                    look,
                )
                .when_some(state.options.get(at), |row, focus| row.track_focus(focus))
                .aria_selected(on)
                .when(!on, |row| {
                    row.hover(move |style| style.bg(color_at(palette.text, 0.08)))
                })
                .when(on, |row| {
                    row.relative()
                        .bg(color_at(palette.text, 0.06))
                        .font_weight(FontWeight::MEDIUM)
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(sp(8.))
                                .bottom(sp(8.))
                                .w(px(3.))
                                .rounded_r(px(2.))
                                .bg(color(palette.text)),
                        )
                })
                .child(option.clone())
                .on_click(cx.listener(move |view, _, window, cx| {
                    choose(view, at, window, cx);
                }))
                .into_any_element()
            })
            .collect();
        popup_list(
            SharedString::from(format!("{id}-options")),
            label.clone(),
            &bounds,
            rows,
            look,
            close,
            cx,
        )
    });
    div()
        .flex_none()
        .on_prepaint(move |drawn, window, _| {
            if bounds.replace(drawn) != drawn && open {
                // The list follows the field on the next frame.
                crate::ui::frames::ask(window);
            }
        })
        .child(
            suggestion(id, format!("{label}: {name}"), look)
                .track_focus(&state.field)
                .role(Role::ComboBox)
                .aria_expanded(open)
                .min_h(sp(32.))
                .border_1()
                .border_color(color(palette.control_line))
                .bg(color(palette.panel))
                .child(name)
                .child(div().flex_1())
                .child(icon(assets::CHEVRON, 14.).text_color(color(palette.secondary)))
                .on_click(cx.listener(move |view, _, window, cx| toggle(view, window, cx))),
        )
        .children(list)
}

/// A whole number typed in a field, with a button on each side that takes
/// one off or adds one: `step` is told -1 or 1. Each button is named for
/// what it changes, so two steppers on one screen are told apart.
pub fn stepper<V: 'static>(
    id: &'static str,
    label: impl Into<SharedString>,
    input: &Entity<InputState>,
    look: &Look,
    step: impl Fn(&mut V, i32, &mut Window, &mut Context<V>) + Clone + 'static,
    cx: &mut Context<V>,
) -> Div {
    let label = label.into();
    let less = step.clone();
    div()
        .flex()
        .items_center()
        .gap(sp(6.))
        .child(
            icon_button(
                SharedString::from(format!("{id}-less")),
                look.tr_with("stepper-less", &[("what", &label)]),
                assets::MINUS,
                true,
                look,
            )
            .on_click(cx.listener(move |view, _, window, cx| less(view, -1, window, cx))),
        )
        .child(
            div()
                .w(sp(72.))
                .child(Input::new(input).id(id).aria_label(label.clone())),
        )
        .child(
            icon_button(
                SharedString::from(format!("{id}-more")),
                look.tr_with("stepper-more", &[("what", &label)]),
                assets::ADD,
                true,
                look,
            )
            .on_click(cx.listener(move |view, _, window, cx| step(view, 1, window, cx))),
        )
}

/// The loading ring: a thin arc in the accent that turns for as long as it
/// is drawn. **It turns under "reduce motion" too**: it says something is
/// going on, which is not decoration.
///
/// **Drawn as a path at its angle on every frame**, not a drawing turned:
/// a turned drawing is resampled from its upright pixels, softer at some
/// angles than at others and with its edges sharpened as text's are, which
/// reads as a ring that shimmers rather than one that turns.
pub fn ring(size: f32, look: &Look, window: &mut Window) -> AnyElement {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    crate::ui::frames::ask(window);
    let turned = turn_at(STARTED.get_or_init(Instant::now).elapsed());
    let ink = color(look.palette.accent);
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let mut path = PathBuilder::fill();
            path.add_polygon(&arc(bounds, turned), true);
            match path.build() {
                Ok(path) => window.paint_path(path, ink),
                Err(e) => tracing::debug!(error = %e, "the loading ring was not drawn"),
            }
        },
    )
    .size(sp(size))
    .flex_none()
    .into_any_element()
}

/// The outline of the ring's arc inside `bounds`, `turned` of a turn from
/// upright: three quarters of a circle of radius 9 with a round-ended stroke
/// of 2.5, in a box of 24, as the drawing the app had before.
fn arc(bounds: Bounds<Pixels>, turned: f32) -> Vec<Point<Pixels>> {
    const STEPS: usize = 72;
    const CAP_STEPS: usize = 10;
    let scale = f32::from(bounds.size.width.min(bounds.size.height)) / 24.0;
    let (radius, half) = (9.0 * scale, 1.25 * scale);
    let center = bounds.center();
    let at = |angle: f32, from: Point<Pixels>, distance: f32| {
        point(
            from.x + px(distance * angle.cos()),
            from.y + px(distance * angle.sin()),
        )
    };
    // From the top, clockwise (y grows downward), three quarters round.
    let start = -TAU / 4.0 + turned * TAU;
    let end = start + TAU * 0.75;
    let along = |step: usize| start + (end - start) * step as f32 / STEPS as f32;
    let around = |step: usize| TAU / 2.0 * step as f32 / CAP_STEPS as f32;

    let mut outline = Vec::with_capacity(2 * (STEPS + CAP_STEPS + 2));
    outline.extend((0..=STEPS).map(|step| at(along(step), center, radius + half)));
    let tip = at(end, center, radius);
    outline.extend((1..CAP_STEPS).map(|step| at(end + around(step), tip, half)));
    outline.extend(
        (0..=STEPS)
            .rev()
            .map(|step| at(along(step), center, radius - half)),
    );
    let tail = at(start, center, radius);
    outline.extend((1..CAP_STEPS).map(|step| at(start + TAU / 2.0 + around(step), tail, half)));
    outline
}

/// How far into its turn the ring is `since` it first turned, from 0 to 1.
/// **Counted in whole microseconds**: seconds as an `f32` lose their
/// precision as the app stays open, and after a day a frame's angle moves
/// in steps the eye sees.
fn turn_at(since: std::time::Duration) -> f32 {
    let turn = RING_TURN.as_micros();
    (since.as_micros() % turn) as f32 / turn as f32
}

/// The first letter an account is drawn as while it has no picture.
fn initial(account: &Account) -> String {
    account
        .named
        .username
        .as_deref()
        .or(account.full_name.as_deref())
        .and_then(|name| name.chars().find(|c| c.is_alphanumeric()))
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_string())
}

/// What a picture of `account` is called.
pub fn photo_label(account: &Account, look: &Look) -> String {
    match &account.named.username {
        Some(username) => look.tr_with("account-photo", &[("username", username)]),
        None => look.tr("account-photo-unnamed"),
    }
}

/// How far each account's picture has come in (`Shell::pictures_shown`):
/// from 0, still being read, to 1, there.
pub type Shown = std::collections::HashMap<snob_core::Pk, f32>;

/// How far `account`'s picture has come in: there when nobody said.
pub fn shown_of(shown: &Shown, account: &Account) -> f32 {
    shown.get(&account.named.pk).copied().unwrap_or(1.0)
}

/// An account's picture in a circle of `size` logical pixels: the picture
/// kept on disk, or its initial on the accent while there is none or it does
/// not load. While it is read the circle is a quiet tint, and it comes in
/// as `shown` goes from 0 to 1: fading in, and growing into the circle
/// unless motion is reduced. **Not named here**: what holds it says whose
/// picture it is.
pub fn avatar(account: &Account, size: f32, look: &Look, shown: f32) -> Div {
    let palette = look.palette;
    let letter = initial(account);
    let fallback = move || {
        div()
            .size(px(size))
            .rounded_full()
            .bg(color(palette.accent))
            .flex()
            .items_center()
            .justify_center()
            .font_weight(FontWeight::BOLD)
            .text_size(px(size * 0.45))
            .text_color(color(palette.on_accent))
            .child(letter.clone())
    };
    let circle = div().size(px(size)).flex_none().rounded_full();
    match &account.picture {
        Some(picture) => {
            let shown = shown.clamp(0.0, 1.0);
            let drawn = if look.settings.reduce_motion {
                size
            } else {
                size * (0.86 + 0.14 * shown)
            };
            circle
                .flex()
                .items_center()
                .justify_center()
                .when(shown < 1.0, |circle| {
                    circle.bg(color_at(palette.text, 0.08 * (1.0 - shown)))
                })
                .child(
                    img(picture.clone())
                        .size(px(drawn))
                        .rounded_full()
                        .opacity(shown)
                        .with_fallback(move || fallback().into_any_element()),
                )
        }
        None => circle.child(fallback()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::flow::Named;

    fn account(username: Option<&str>, full_name: Option<&str>) -> Account {
        Account {
            named: Named {
                pk: 42.into(),
                username: username.map(str::to_string),
            },
            full_name: full_name.map(str::to_string),
            biography: None,
            picture: None,
            picture_kept_at: None,
            active: true,
        }
    }

    #[test]
    fn the_ring_turns_evenly_however_long_the_app_has_been_open() {
        use std::time::Duration;
        let step = Duration::from_micros(6_100); // a frame at 164 Hz
        for open in [0, 3_600, 86_400, 30 * 86_400] {
            let since = Duration::from_secs(open) + Duration::from_millis(137);
            let moved = turn_at(since + step) - turn_at(since);
            let expected = step.as_secs_f32() / RING_TURN.as_secs_f32();
            assert!(
                (moved - expected).abs() < 1e-4,
                "after {open} s open a frame turns it {moved}, not {expected}"
            );
        }
        assert_eq!(turn_at(RING_TURN), 0.0);
    }

    #[test]
    fn the_ring_is_the_same_arc_at_every_angle_and_stays_in_its_box() {
        let bounds = Bounds::new(point(px(10.), px(20.)), gpui_kit::size(px(24.), px(24.)));
        let center = bounds.center();
        let first = arc(bounds, 0.0);
        // Upright, it starts at the top, on the outer edge of the stroke.
        assert!(
            (f32::from(first[0].x) - 22.0).abs() < 1e-3,
            "{:?}",
            first[0]
        );
        assert!(
            (f32::from(first[0].y) - 21.75).abs() < 1e-3,
            "{:?}",
            first[0]
        );
        for turned in [0.0, 0.1, 0.125, 0.37, 0.5, 0.9] {
            let outline = arc(bounds, turned);
            assert_eq!(outline.len(), first.len());
            for at in &outline {
                assert!(bounds.contains(at), "{at:?} at {turned}");
                let from = f32::from(at.x - center.x).hypot(f32::from(at.y - center.y));
                assert!(from <= 10.25 + 1e-3, "{from} from the middle at {turned}");
            }
        }
    }

    /// GPUI refuses a second tip on one control in a debug build: a control
    /// that refuses one already has its own.
    #[cfg(debug_assertions)]
    #[test]
    fn a_control_that_shows_only_a_drawing_has_exactly_one_tip() {
        use crate::system::Settings;
        let look = Look::of(Settings::default(), false);
        let takes_a_tip = |button: Button| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                drop(button.tooltip(tip("another")));
            }))
            .is_ok()
        };
        assert!(!takes_a_tip(icon_button(
            "hide",
            "Hide",
            crate::ui::assets::HIDE,
            true,
            &look
        )));
        assert!(!takes_a_tip(icon_segment(
            "grid",
            "Grid",
            crate::ui::assets::FIT,
            true,
            &look
        )));
        assert!(
            takes_a_tip(button("save", "Save", Kind::Primary, true, &look)),
            "a button that says its name in words has no tip of its own"
        );
    }

    #[test]
    fn an_account_without_a_picture_is_its_first_letter() {
        assert_eq!(initial(&account(Some("me"), None)), "M");
        assert_eq!(initial(&account(Some("_.someone"), None)), "S");
        assert_eq!(initial(&account(None, Some("Ada"))), "A");
        assert_eq!(initial(&account(None, None)), "?");
    }
}
