//! The home screen, a title until there is something to put under it, and
//! the settings, which for now are the log's: its detailed level, its
//! folder, and the diagnostic report to paste into an issue.
//!
//! Both show the report of the last crash until the person has dealt with
//! it. **Nothing of it is sent anywhere**: it can be read and copied here.

use gpui_kit::base::{Button, TestSupportExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Div, FontWeight, Role, Toggled, div, px};

use super::column;
use crate::state::flow::Event;
use crate::theme::fonts::MONO;
use crate::theme::tokens::shape;
use crate::ui::assets;
use crate::ui::frame::clock;
use crate::ui::look::{Look, color, sp};
use crate::ui::parts::{self, Kind, icon};
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
fn crash(shell: &mut Shell, look: &Look, cx: &mut Context<Shell>) -> Option<Div> {
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
                            .font_family(MONO)
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
                        parts::button(
                            "crash-view",
                            look.tr("crash-view"),
                            Kind::Secondary,
                            true,
                            look,
                        )
                        .aria_expanded(open)
                        .on_click(cx.listener(|shell, _, _, cx| {
                            shell.crash_open = !shell.crash_open;
                            cx.notify();
                        })),
                    )
                    .child(
                        parts::button(
                            "crash-copy",
                            look.tr("crash-copy"),
                            Kind::Secondary,
                            true,
                            look,
                        )
                        .on_click(cx.listener(|shell, _, _, cx| shell.copy_the_crash(cx))),
                    )
                    .child(
                        parts::button(
                            "crash-dismiss",
                            look.tr("crash-dismiss"),
                            Kind::Secondary,
                            true,
                            look,
                        )
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.dismiss_the_crash(window, cx);
                        })),
                    ),
            ),
    )
}

pub fn home(shell: &mut Shell, phase: Phase, cx: &mut Context<Shell>) -> AnyElement {
    let look = Look::now(cx).clone();
    column(phase)
        .child(parts::title(&shell.title, look.tr("home-title"), &look))
        .children(crash(shell, &look, cx))
        .into_any_element()
}

pub fn settings(shell: &mut Shell, phase: Phase, cx: &mut Context<Shell>) -> AnyElement {
    let look = Look::now(cx).clone();
    let palette = look.palette;
    let detailed_until = shell.backend().detailed_until();
    let detailed = detailed_until.is_some();
    let copied = shell.report_copied;

    // The switch: its track and its thumb, and its state in words beside it.
    let switch = Button::new("settings-detailed")
        .role(Role::Switch)
        .accessibility_label(look.tr("settings-detailed"))
        .aria_toggled(if detailed {
            Toggled::True
        } else {
            Toggled::False
        })
        .flex_none()
        .w(px(46.))
        .h(px(26.))
        .p(px(2.))
        .rounded_full()
        .border_1()
        .border_color(color(palette.control_line))
        .bg(color(if detailed {
            palette.accent
        } else {
            palette.panel
        }))
        .cursor_pointer()
        .focus_visible(parts::focus_ring(&palette))
        .justify_start()
        .on_click(cx.listener(move |shell, _, _, cx| shell.set_detailed(!detailed, cx)))
        .child(
            div()
                .size(px(20.))
                .rounded_full()
                .bg(color(if detailed {
                    palette.on_accent
                } else {
                    palette.control_line
                }))
                .when(detailed, |thumb| thumb.ml(px(20.))),
        );

    let logs = card(&look)
        .child(
            div()
                .id("settings-logs")
                .test_support()
                .role(Role::Heading)
                .aria_level(2)
                .aria_label(look.tr("settings-logs"))
                .font_family(MONO)
                .font_weight(FontWeight::BOLD)
                .text_size(sp(16.))
                .text_color(color(palette.text))
                .child(look.tr("settings-logs")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(14.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(sp(2.))
                        .child(
                            div()
                                .text_size(sp(14.5))
                                .text_color(color(palette.text))
                                .child(parts::said(
                                    "settings-detailed-label",
                                    look.tr("settings-detailed"),
                                )),
                        )
                        .child(
                            div()
                                .text_size(sp(13.))
                                .line_height(sp(19.))
                                .text_color(color(palette.secondary))
                                .child(parts::said(
                                    "settings-detailed-note",
                                    match detailed_until {
                                        Some(until) => look.tr_with(
                                            "settings-detailed-until",
                                            &[("time", &clock(until))],
                                        ),
                                        None => look.tr("settings-detailed-note"),
                                    },
                                )),
                        ),
                )
                .child(switch),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(10.))
                .child(
                    parts::button(
                        "settings-open-logs",
                        look.tr("settings-open-logs"),
                        Kind::Secondary,
                        true,
                        &look,
                    )
                    .on_click(cx.listener(|shell, _, _, cx| shell.open_the_logs(cx))),
                )
                .child(
                    parts::button(
                        "settings-copy-report",
                        look.tr("settings-copy-report"),
                        Kind::Secondary,
                        true,
                        &look,
                    )
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.copy_the_report(window, cx);
                    })),
                ),
        )
        // Said for as long as the screen is shown, not for two seconds.
        .when(copied, |card| {
            card.child(
                div()
                    .text_size(sp(13.))
                    .text_color(color(palette.text))
                    .child(parts::said(
                        "settings-report-copied",
                        look.tr("settings-report-copied"),
                    )),
            )
        });

    column(phase)
        .child(parts::title(&shell.title, look.tr("settings-title"), &look))
        .child(logs)
        .children(crash(shell, &look, cx))
        .child(
            parts::button("back", look.tr("back"), Kind::Secondary, true, &look).on_click(
                cx.listener(|shell, _, window, cx| {
                    shell.happened(Event::Back, window, cx);
                }),
            ),
        )
        .into_any_element()
}
