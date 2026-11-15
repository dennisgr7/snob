//! A sign-in that failed: what happened and what to do in plain words, the
//! same method again or the way back, and the code the failure was logged
//! under for whoever looks for it, folded away.

use gpui_kit::base::{Button, TestSupportExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, Role, div, px};
use snob_engine::login::Failure;

use super::{column, failure};
use crate::state::flow::{Event, Method};
use crate::theme::fonts::MONO;
use crate::theme::tokens::shape;
use crate::ui::assets;
use crate::ui::look::{Look, color, sp};
use crate::ui::parts::{self, Kind, icon};
use crate::ui::shell::{Phase, Shell};

pub fn render(
    shell: &mut Shell,
    why: Failure,
    id: String,
    retry: Method,
    phase: Phase,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    let palette = look.palette;
    let said = failure(why, retry, shell.failed_until, &look);
    let open = shell.details_open;

    let details = (!id.is_empty()).then(|| {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(sp(6.))
            .child(
                Button::new("error-details")
                    .accessibility_label(look.tr("error-details"))
                    .aria_expanded(open)
                    .min_h(px(shape::MIN_TARGET))
                    .px(sp(8.))
                    .rounded(px(shape::CONTROL_RADIUS))
                    .text_size(sp(13.))
                    .text_color(color(palette.secondary))
                    .underline()
                    .cursor_pointer()
                    .focus_visible(parts::focus_ring(&palette))
                    .on_click(cx.listener(|shell, _, _, cx| {
                        shell.details_open = !shell.details_open;
                        cx.notify();
                    }))
                    .child(look.tr("error-details")),
            )
            .when(open, |details| {
                details.child(
                    div()
                        .font_family(MONO)
                        .text_size(sp(13.))
                        .text_color(color(palette.text))
                        .child(parts::said(
                            "error-code",
                            look.tr_with("error-code", &[("id", &id)]),
                        )),
                )
            })
    });

    column(phase)
        .child(icon(assets::ERROR, 36.).text_color(color(palette.error)))
        .child(parts::title(&shell.title, look.tr("error-title"), &look))
        .child(
            div()
                .id("error-why")
                .test_support()
                .role(Role::Alert)
                .aria_label(said.clone())
                .max_w(sp(460.))
                .text_size(sp(15.))
                .line_height(sp(22.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(palette.text))
                .text_center()
                .child(said),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .justify_center()
                .gap(sp(12.))
                .pt(sp(8.))
                .child(
                    parts::button(
                        "error-retry",
                        look.tr("error-retry"),
                        Kind::Primary,
                        true,
                        &look,
                    )
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.happened(Event::Retry, window, cx);
                    })),
                )
                .child(
                    parts::button("back", look.tr("back"), Kind::Secondary, true, &look).on_click(
                        cx.listener(|shell, _, window, cx| {
                            shell.happened(Event::Back, window, cx);
                        }),
                    ),
                ),
        )
        .children(details)
        .into_any_element()
}
