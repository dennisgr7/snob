//! The splash: the brand alone in the middle of the panel while the app
//! reads what is on this computer. It leaves with a quick fade, a little
//! smaller.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, Role, div, px};

use super::column;
use crate::theme::fonts::MONO;
use crate::ui::look::{Look, color};
use crate::ui::shell::{Phase, Shell};

/// The brand's size here, in logical pixels.
const BRAND: f32 = 56.0;

pub fn render(phase: Phase, cx: &mut Context<Shell>) -> AnyElement {
    let look = Look::now(cx);
    column(phase)
        .child(
            div()
                .id("splash-brand")
                .test_support()
                .role(Role::Image)
                .aria_label(look.tr("brand"))
                .font_family(MONO)
                .font_weight(FontWeight::BOLD)
                .italic()
                .text_size(px(BRAND * phase.scale))
                .text_color(color(look.palette.accent))
                .child(look.tr("brand")),
        )
        .into_any_element()
}
