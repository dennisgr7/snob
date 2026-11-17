//! The welcome: the account's picture and its name, for a moment, before
//! the home screen. **It moves on by itself, and a screen reader is told it
//! before it does** (`Shell::enter`).

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, Role, div, px};

use super::{column, handle};
use crate::state::flow::Named;
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

    // The picture pops into place, a touch past its size and back, while
    // the words rise under it; under "reduce motion" it is there at its size
    // and only fades in. A picture still being read when the welcome comes
    // in arrives on its own once it is (`parts::avatar`).
    let grown = if look.settings.reduce_motion {
        PICTURE
    } else {
        let at = phase.presence.clamp(0.0, 1.0);
        PICTURE * (0.86 + 0.14 * at + 0.06 * (std::f32::consts::PI * at).sin())
    };
    let shown = shell.pictures_shown().clone();
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
            .child(parts::avatar(
                &account,
                grown,
                &look,
                parts::shown_of(&shown, &account),
            ))
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
                .font_weight(FontWeight::MEDIUM)
                .text_size(sp(22.))
                .line_height(sp(30.))
                .text_color(color(palette.text))
                .child(before)
                .child(
                    parts::username(&look)
                        .font_weight(FontWeight::BOLD)
                        .child(who),
                )
                .child(after),
        )
        .into_any_element()
}
