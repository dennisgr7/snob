//! The welcome: the account's picture and its name, for a moment, before
//! the home screen. **It moves on by itself, and a screen reader is told it
//! before it does** (`Shell::enter`).

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, Role, div, px};

use super::{column, handle};
use crate::state::flow::Named;
use crate::theme::fonts::MONO;
use crate::ui::look::{Look, color, sp};
use crate::ui::parts;
use crate::ui::shell::{Phase, Shell};

/// The picture's size, in logical pixels.
const PICTURE: f32 = 96.0;

pub fn render(
    shell: &mut Shell,
    account: Named,
    phase: Phase,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    let palette = look.palette;
    let who = handle(&account);
    let whole = look.tr_with("welcome", &[("account", &who)]);
    let (before, after) = look.texts.around("welcome", "account", &who);
    let signed_in = shell
        .accounts()
        .iter()
        .find(|known| known.named.pk == account.pk)
        .cloned();

    // The picture grows into place while the words rise under it; under
    // "reduce motion" it is there at its size and only fades in.
    let grown = if look.settings.reduce_motion {
        PICTURE
    } else {
        PICTURE * (0.9 + 0.1 * phase.presence)
    };
    let picture = signed_in.map(|account| {
        div()
            .id("welcome-photo")
            .test_support()
            .role(Role::Image)
            .aria_label(parts::photo_label(&account, &look))
            .size(px(PICTURE))
            .flex()
            .items_center()
            .justify_center()
            .child(parts::avatar(&account, grown, &look))
    });

    column(Phase { rise: 0.0, ..phase })
        .children(picture)
        .child(
            div()
                .id("title")
                .test_support()
                .role(Role::Heading)
                .aria_level(1)
                .aria_label(whole)
                .track_focus(&shell.title)
                .relative()
                .top(px(phase.rise))
                .flex()
                .flex_wrap()
                .justify_center()
                .font_family(MONO)
                .font_weight(FontWeight::MEDIUM)
                .text_size(sp(22.))
                .line_height(sp(30.))
                .text_color(color(palette.text))
                .child(before)
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(palette.emphasis))
                        .underline()
                        .text_decoration_2()
                        .text_decoration_color(color(palette.underline))
                        .child(who),
                )
                .child(after),
        )
        .into_any_element()
}
