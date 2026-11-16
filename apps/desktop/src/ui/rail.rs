//! The rail of sections at the left of an open frame: one button a section,
//! the one the app is in chosen.
//!
//! **The section chosen is told apart by more than a color**: a wash behind
//! its button, a bar of the accent at its left and its drawing in the text's
//! color, and with more contrast an outline. Its measures are the frame's,
//! in logical pixels: twice the text does not widen it.

use gpui_kit::base::{Button, TestSupportExt as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Role, div, px};

use super::assets;
use super::look::{Look, color, color_at};
use super::parts;
use super::shell::{FrameNow, Shell};
use crate::state::flow::Place;
use crate::theme::tokens::{frame, shape};

/// Above the first button, and between two.
const GAP: f32 = 8.0;

/// The drawing of a section.
fn drawing(place: Place) -> &'static str {
    match place {
        Place::Home => assets::HOME,
        Place::Search => assets::SEARCH,
        Place::Automations => assets::AUTOMATIONS,
    }
}

/// A section's button, as a test and the log name it.
pub fn id(place: Place) -> &'static str {
    match place {
        Place::Home => "rail-home",
        Place::Search => "rail-search",
        Place::Automations => "rail-automations",
    }
}

/// The rail, when the frame is open or on its way: nothing while it is
/// closed.
pub(crate) fn render(now: &FrameNow, look: &Look, cx: &mut Context<Shell>) -> Option<AnyElement> {
    if now.openness <= 0.0 && now.icons.iter().all(|icon| icon.presence <= 0.0) {
        return None;
    }
    let palette = look.palette;
    let contrast = look.settings.more_contrast;
    let inset = (frame::RAIL - frame::RAIL_BUTTON) / 2.0;
    let row = |at: f32| GAP + at * (frame::RAIL_BUTTON + GAP);

    // Behind the buttons, so a press reaches the button over it.
    let pill = now.pill.map(|pill| {
        let presence = pill.presence * now.icons[0].presence;
        div()
            .absolute()
            .top(px(row(pill.row)))
            .left(px(inset))
            .size(px(frame::RAIL_BUTTON))
            .rounded(px(shape::CONTROL_RADIUS))
            .bg(color_at(palette.text, 0.08))
            .when(contrast, |pill| {
                pill.border_1().border_color(color(palette.line))
            })
            .opacity(presence)
            .child(
                div()
                    .absolute()
                    .left(px(-inset))
                    .top(px(10.))
                    .bottom(px(10.))
                    .w(px(3.))
                    .rounded_r(px(2.))
                    .bg(color(palette.accent)),
            )
    });

    let buttons = Place::ALL.into_iter().enumerate().map(|(at, place)| {
        let icon = now.icons[at];
        let chosen = now.selected == Some(at);
        let name = super::screens::section_title(place, look);
        let tip = name.clone();
        let ink = if chosen {
            palette.text
        } else {
            palette.secondary
        };
        Button::new(id(place))
            .accessibility_label(name)
            .aria_selected(chosen)
            .absolute()
            .top(px(row(at as f32)))
            .left(px(inset))
            .size(px(frame::RAIL_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(shape::CONTROL_RADIUS))
            .opacity(icon.presence)
            .when(!chosen, |button| {
                button.hover(move |style| style.bg(color_at(palette.text, 0.05)))
            })
            .focus_visible(parts::focus_ring(&palette))
            .cursor_pointer()
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |shell, _, window, cx| shell.go(place, window, cx)))
            .child(
                gpui_kit::svg()
                    .path(drawing(place))
                    .size(px(20. * icon.scale))
                    .relative()
                    .left(px(icon.shift))
                    .text_color(color(ink)),
            )
    });

    Some(
        div()
            .id("rail")
            .test_support()
            .role(Role::Navigation)
            .aria_label(look.tr("rail"))
            .absolute()
            .top(px(frame::BAND))
            .left_0()
            .bottom_0()
            .w(px(frame::RAIL))
            .children(pill)
            .children(buttons)
            .into_any_element(),
    )
}
