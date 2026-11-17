//! What the screens are built from: the title a screen is known by, text
//! that is read, the two buttons, the loading ring, an account's picture.
//! **Each has its name, its role and its state for a screen reader built
//! in**, so a screen cannot draw a control that is only a shape.

use std::f32::consts::TAU;
use std::sync::OnceLock;
use std::time::Instant;

use gpui_kit::base::{Button, ObservedElement, TestSupportExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, BoxShadow, Div, ElementId, FocusHandle, FontWeight, PathBuilder, Pixels,
    Point, Role, SharedString, Stateful, StyleRefinement, Text, Window, canvas, div, img, point,
    px, svg,
};

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
/// whoever does not see the drawing.
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
    plain_button(id, label, Kind::Secondary, enabled, look)
        .w(sp(FIELD_HEIGHT))
        .h(sp(FIELD_HEIGHT))
        .px(px(0.))
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
/// all the same, for whoever does not see the drawing.
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
        .accessibility_label(label)
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
    window.request_animation_frame();
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

    #[test]
    fn an_account_without_a_picture_is_its_first_letter() {
        assert_eq!(initial(&account(Some("me"), None)), "M");
        assert_eq!(initial(&account(Some("_.someone"), None)), "S");
        assert_eq!(initial(&account(None, Some("Ada"))), "A");
        assert_eq!(initial(&account(None, None)), "?");
    }
}
