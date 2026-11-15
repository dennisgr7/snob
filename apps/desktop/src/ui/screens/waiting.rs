//! The screens that wait: for the update check and its download, and for
//! the person to sign in in snob's browser window. Each says in words what
//! it waits for, beside the ring that turns.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Role, Window, div, px, relative};

use super::column;
use crate::state::flow::{Event, Updating};
use crate::theme::fonts::MONO;
use crate::theme::tokens::shape;
use crate::ui::look::{Look, color, sp};
use crate::ui::parts::{self, Kind};
use crate::ui::shell::{Phase, Shell};

pub fn updating(
    shell: &mut Shell,
    at: Updating,
    phase: Phase,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    let palette = look.palette;
    let title = match at {
        Updating::Checking => "update-checking",
        Updating::Downloading(_) => "update-downloading",
        Updating::Restarting => "update-restarting",
    };
    let screen = column(phase).child(parts::title(&shell.title, look.tr(title), &look));
    match at {
        Updating::Checking | Updating::Restarting => screen.child(parts::ring(28., &look, window)),
        Updating::Downloading(percent) => {
            let said = match percent {
                Some(percent) => {
                    look.tr_with("update-percent", &[("percent", &percent.to_string())])
                }
                None => look.tr("loading"),
            };
            let done = f32::from(percent.unwrap_or(0).min(100)) / 100.0;
            screen
                .child(
                    div()
                        .id("update-progress")
                        .test_support()
                        .role(Role::ProgressIndicator)
                        .aria_label(look.tr("update-downloading"))
                        .aria_value(said.clone())
                        .aria_min_numeric_value(0.0)
                        .aria_max_numeric_value(100.0)
                        .when_some(percent, |bar, percent| {
                            bar.aria_numeric_value(f64::from(percent))
                        })
                        .w(sp(320.))
                        .max_w_full()
                        .h(px(8.))
                        .rounded_full()
                        .bg(color(palette.line))
                        .border_1()
                        .border_color(color(palette.control_line))
                        .overflow_hidden()
                        .child(div().h_full().w(relative(done)).bg(color(palette.accent))),
                )
                .child(
                    div()
                        .font_family(MONO)
                        .text_size(sp(14.))
                        .text_color(color(palette.text))
                        .child(said),
                )
        }
    }
    .into_any_element()
}

pub fn browser(
    shell: &mut Shell,
    phase: Phase,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    column(phase)
        .child(parts::title(&shell.title, look.tr("browser-title"), &look))
        .child(
            div()
                .max_w(sp(460.))
                .flex()
                .flex_col()
                .gap(sp(8.))
                .child(
                    parts::paragraph("browser-body", look.tr("browser-body"), &look)
                        .text_color(color(look.palette.text)),
                )
                .child(parts::paragraph(
                    "browser-note",
                    look.tr("browser-note"),
                    &look,
                )),
        )
        .child(
            div()
                .py(sp(6.))
                .min_h(px(shape::MIN_TARGET))
                .child(parts::ring(28., &look, window)),
        )
        .child(
            parts::button("cancel", look.tr("cancel"), Kind::Secondary, true, &look).on_click(
                cx.listener(|shell, _, window, cx| {
                    shell.happened(Event::Back, window, cx);
                }),
            ),
        )
        .into_any_element()
}
