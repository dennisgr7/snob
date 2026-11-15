//! "Let's get started.": the two ways to sign in. With the account whose
//! session expired, when that is why the person is here; with a way back,
//! when they came to add an account to one already signed in.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, Role, div};

use super::{column, handle};
use crate::backend::Account;
use crate::state::flow::{Event, Method, Named};
use crate::theme::fonts::MONO;
use crate::ui::look::{Look, color, sp};
use crate::ui::parts::{self, Kind};
use crate::ui::shell::{Phase, Shell};

pub fn render(
    shell: &mut Shell,
    expired: Option<Named>,
    back: bool,
    phase: Phase,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    let palette = look.palette;

    // The account asked back: its picture and its name, so it is plainly not
    // a new person being asked to start.
    let asked_back = expired.map(|named| {
        let account = shell
            .accounts()
            .iter()
            .find(|account| account.named.pk == named.pk)
            .cloned()
            .unwrap_or(Account {
                named: named.clone(),
                full_name: None,
                biography: None,
                picture: None,
                active: false,
            });
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(sp(10.))
            .child(
                div()
                    .id("expired-photo")
                    .test_support()
                    .role(Role::Image)
                    .aria_label(parts::photo_label(&account, &look))
                    .child(parts::avatar(&account, 72., &look)),
            )
            .child(
                div()
                    .font_family(MONO)
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(sp(16.))
                    .text_color(color(palette.text))
                    .child(parts::said("expired-account", handle(&named))),
            )
            .child(parts::paragraph(
                "expired-why",
                look.tr("start-expired"),
                &look,
            ))
    });

    let choice = |id: &'static str, kind: Kind| {
        parts::button(id, look.tr(id), kind, true, &look).min_w(sp(320.))
    };

    column(phase)
        .children(asked_back)
        .child(parts::title(&shell.title, look.tr("start-title"), &look))
        .children(
            shell
                .notice
                .clone()
                .map(|notice| parts::paragraph("notice", notice, &look)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(12.))
                .pt(sp(8.))
                .child(choice("start-browser", Kind::Primary).on_click(cx.listener(
                    |shell, _, window, cx| {
                        shell.chose(Method::Browser, window, cx);
                    },
                )))
                .child(choice("start-paste", Kind::Secondary).on_click(cx.listener(
                    |shell, _, window, cx| {
                        shell.chose(Method::Paste, window, cx);
                    },
                )))
                .when(back, |buttons| {
                    buttons.child(choice("back", Kind::Secondary).on_click(cx.listener(
                        |shell, _, window, cx| {
                            shell.happened(Event::Back, window, cx);
                        },
                    )))
                }),
        )
        .into_any_element()
}
