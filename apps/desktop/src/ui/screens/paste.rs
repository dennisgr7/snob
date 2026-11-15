//! Signing in with a pasted session ID.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Window};

use super::column;
use crate::state::flow::Event;
use crate::ui::look::Look;
use crate::ui::parts::{self, Kind};
use crate::ui::shell::{Phase, Shell};

pub fn render(
    shell: &mut Shell,
    phase: Phase,
    _window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    column(phase)
        .child(parts::title(&shell.title, look.tr("paste-title"), &look))
        .child(
            parts::button("back", look.tr("back"), Kind::Secondary, true, &look).on_click(
                cx.listener(|shell, _, window, cx| {
                    shell.leave_the_sign_in();
                    shell.happened(Event::Back, window, cx);
                }),
            ),
        )
        .into_any_element()
}
