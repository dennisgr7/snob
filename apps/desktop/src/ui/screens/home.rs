//! The home screen and the settings: a title each, until there is something
//! to put under it.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context};

use super::column;
use crate::state::flow::Event;
use crate::ui::look::Look;
use crate::ui::parts::{self, Kind};
use crate::ui::shell::{Phase, Shell};

pub fn home(shell: &mut Shell, phase: Phase, cx: &mut Context<Shell>) -> AnyElement {
    let look = Look::now(cx).clone();
    column(phase)
        .child(parts::title(&shell.title, look.tr("home-title"), &look))
        .into_any_element()
}

pub fn settings(shell: &mut Shell, phase: Phase, cx: &mut Context<Shell>) -> AnyElement {
    let look = Look::now(cx).clone();
    column(phase)
        .child(parts::title(&shell.title, look.tr("settings-title"), &look))
        .child(
            parts::button("back", look.tr("back"), Kind::Secondary, true, &look).on_click(
                cx.listener(|shell, _, window, cx| {
                    shell.happened(Event::Back, window, cx);
                }),
            ),
        )
        .into_any_element()
}
