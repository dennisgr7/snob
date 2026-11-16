//! The home screen, a title until there is something to put under it, the
//! search still to come, each with its title at the top left as the
//! settings have theirs, and the report of the last crash, which home and
//! the settings show until the person has dealt with it. **Nothing of it is
//! sent anywhere**: it can be read and copied here.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Div, FontWeight, Role, div, px};

use crate::state::flow::Place;
use crate::theme::fonts::MONO;
use crate::theme::tokens::shape;
use crate::ui::assets;
use crate::ui::look::{Look, color, sp};
use crate::ui::parts::{self, icon};
use crate::ui::shell::{Phase, Shell};

/// A card: something set apart inside the panel.
fn card(look: &Look) -> Div {
    div()
        .w_full()
        .max_w(sp(560.))
        .p(sp(20.))
        .flex()
        .flex_col()
        .gap(sp(12.))
        .rounded(px(shape::CARD_RADIUS))
        .border_1()
        .border_color(color(look.palette.line))
}

/// The report of the last crash, offered until it is dismissed.
pub(super) fn crash(shell: &mut Shell, look: &Look, cx: &mut Context<Shell>) -> Option<Div> {
    let palette = look.palette;
    let report = shell.crash.clone()?;
    let open = shell.crash_open;
    Some(
        card(look)
            .border_color(color(palette.control_line))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(8.))
                    .child(icon(assets::ERROR, 18.).text_color(color(palette.error)))
                    .child(
                        div()
                            .id("crash-title")
                            .test_support()
                            .role(Role::Heading)
                            .aria_level(2)
                            .aria_label(look.tr("crash-title"))
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(sp(14.))
                            .text_color(color(palette.text))
                            .child(look.tr("crash-title")),
                    ),
            )
            .child(
                div()
                    .text_size(sp(13.5))
                    .line_height(sp(20.))
                    .text_color(color(palette.secondary))
                    .child(parts::said("crash-body", look.tr("crash-body"))),
            )
            .when(open, |card| {
                card.child(
                    div()
                        .id("crash-report")
                        .test_support()
                        .role(Role::Document)
                        .aria_label(look.tr("crash-view"))
                        .max_h(sp(180.))
                        .overflow_y_scroll()
                        .p(sp(10.))
                        .rounded(px(shape::CONTROL_RADIUS))
                        .bg(color(palette.frame))
                        .font_family(MONO)
                        .text_size(sp(12.))
                        .line_height(sp(17.))
                        .text_color(color(palette.text))
                        .children(report.lines().enumerate().map(|(at, line)| {
                            div().child(parts::said(("crash-line", at), line.to_string()))
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(sp(10.))
                    .child(
                        parts::small_button("crash-view", look.tr("crash-view"), true, look)
                            .aria_expanded(open)
                            .on_click(cx.listener(|shell, _, _, cx| {
                                shell.crash_open = !shell.crash_open;
                                cx.notify();
                            })),
                    )
                    .child(
                        parts::small_button("crash-copy", look.tr("crash-copy"), true, look)
                            .on_click(cx.listener(|shell, _, _, cx| shell.copy_the_crash(cx))),
                    )
                    .child(
                        parts::small_button("crash-dismiss", look.tr("crash-dismiss"), true, look)
                            .on_click(cx.listener(|shell, _, window, cx| {
                                shell.dismiss_the_crash(window, cx);
                            })),
                    ),
            ),
    )
}

/// A section's page: its title at the top left, where the settings have
/// theirs, and what it holds under it, at the place its coming or going has
/// it.
pub(super) fn page(shell: &mut Shell, title: String, phase: Phase, look: &Look) -> Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_start()
        .gap(sp(16.))
        .px(sp(32.))
        .pt(sp(26.))
        .pb(sp(40.))
        .relative()
        .top(px(phase.rise))
        .opacity(phase.presence)
        .child(parts::title(&shell.title, title, look).text_left())
}

pub fn home(shell: &mut Shell, phase: Phase, cx: &mut Context<Shell>) -> AnyElement {
    let look = Look::now(cx).clone();
    page(shell, look.tr("home-title"), phase, &look)
        .children(crash(shell, &look, cx))
        .into_any_element()
}

pub fn search(shell: &mut Shell, phase: Phase, cx: &mut Context<Shell>) -> AnyElement {
    let look = Look::now(cx).clone();
    let title = super::section_title(Place::Search, &look);
    page(shell, title, phase, &look)
        .child(
            parts::paragraph("section-note", look.tr("search-note"), &look)
                .max_w(sp(560.))
                .text_left(),
        )
        .into_any_element()
}
