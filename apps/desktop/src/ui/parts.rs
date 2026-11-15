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
    AnyElement, BoxShadow, Div, ElementId, FocusHandle, FontWeight, Role, SharedString, Stateful,
    StyleRefinement, Text, Transformation, Window, div, img, point, px, radians, svg,
};

use super::assets;
use super::look::{Look, color, color_at, sp};
use crate::backend::Account;
use crate::theme::fonts::{MONO, SANS};
use crate::theme::motion::RING_TURN;
use crate::theme::tokens::{Palette, shape};

/// Text a screen reader reads as it is drawn, known by `id`.
pub fn said(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Text {
    Text::new(id.into(), text.into())
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
        .font_family(MONO)
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
    let palette = look.palette;
    let button = Button::new(id)
        .accessibility_label(label.clone())
        .disabled(!enabled)
        .h(sp(shape::BUTTON_HEIGHT + 4.))
        .min_w(px(shape::MIN_TARGET))
        .px(sp(20.))
        .gap(sp(8.))
        .rounded(px(shape::CONTROL_RADIUS))
        .font_family(MONO)
        .font_weight(FontWeight::MEDIUM)
        .text_size(sp(14.))
        .border_1()
        .focus_visible(focus_ring(&palette));
    let button = if !enabled {
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
    };
    button.child(label)
}

/// A drawing of the app's, colored by the caller, `size` logical pixels at
/// the default text size.
pub fn icon(path: &'static str, size: f32) -> gpui_kit::Svg {
    svg().path(path).size(sp(size)).flex_none()
}

/// The loading ring: a thin arc in the accent that turns for as long as it
/// is drawn. **It turns under "reduce motion" too**: it says something is
/// going on, which is not decoration.
pub fn ring(size: f32, look: &Look, window: &mut Window) -> AnyElement {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    let turned =
        STARTED.get_or_init(Instant::now).elapsed().as_secs_f32() / RING_TURN.as_secs_f32();
    window.request_animation_frame();
    icon(assets::RING, size)
        .text_color(color(look.palette.accent))
        .with_transformation(Transformation::rotate(radians(turned.fract() * TAU)))
        .into_any_element()
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

/// An account's picture in a circle of `size` logical pixels: the picture
/// kept on disk, or its initial on the accent while there is none or it does
/// not load. **Not named here**: what holds it says whose picture it is.
pub fn avatar(account: &Account, size: f32, look: &Look) -> Div {
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
            .font_family(MONO)
            .font_weight(FontWeight::BOLD)
            .text_size(px(size * 0.45))
            .text_color(color(palette.on_accent))
            .child(letter.clone())
    };
    let circle = div().size(px(size)).flex_none().rounded_full();
    match &account.picture {
        Some(picture) => circle.child(
            img(picture.clone())
                .size(px(size))
                .rounded_full()
                .with_fallback(move || fallback().into_any_element()),
        ),
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
            active: true,
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
