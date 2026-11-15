//! Signing in with a pasted session ID: the field at the left, hidden like a
//! password, and at the right where to find one, in four steps.
//!
//! **The ID is a secret.** It is hidden until the person asks to see it,
//! never told to a screen reader, never logged, and gone from the field as
//! soon as it is handed to the engine.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputContentType};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Div, FontWeight, Role, Window, div, px};

use crate::state::flow::Event;
use crate::state::paste::Field;
use crate::theme::fonts::MONO;
use crate::theme::tokens::shape;
use crate::ui::assets;
use crate::ui::look::{Look, color, sp};
use crate::ui::parts::{self, Kind, icon};
use crate::ui::shell::{Phase, Shell};

/// The four steps: what is drawn, and the message that says it.
const STEPS: [(&str, &str); 4] = [
    (assets::STEP_SITE, "paste-step-1"),
    (assets::STEP_TOOLS, "paste-step-2"),
    (assets::STEP_COOKIES, "paste-step-3"),
    (assets::STEP_COPY, "paste-step-4"),
];

/// Where to find a session ID: a list of four steps, each a drawing and a
/// sentence.
fn tutorial(look: &Look) -> Div {
    let palette = look.palette;
    div()
        .flex_1()
        .min_w(sp(260.))
        .max_w(sp(380.))
        .flex()
        .flex_col()
        .gap(sp(14.))
        .child(
            div()
                .id("paste-tutorial-title")
                .test_support()
                .role(Role::Heading)
                .aria_level(2)
                .aria_label(look.tr("paste-tutorial-title"))
                .font_family(MONO)
                .font_weight(FontWeight::MEDIUM)
                .text_size(sp(14.))
                .text_color(color(palette.text))
                .child(look.tr("paste-tutorial-title")),
        )
        .child(
            div()
                .id("paste-steps")
                .test_support()
                .role(Role::List)
                .aria_label(look.tr("paste-tutorial-title"))
                .flex()
                .flex_col()
                .gap(sp(12.))
                .children(STEPS.iter().enumerate().map(|(at, (drawing, message))| {
                    let said = look.tr(message);
                    div()
                        .id(("paste-step", at))
                        .test_support()
                        .role(Role::ListItem)
                        .aria_label(said.clone())
                        .aria_position_in_set(at + 1)
                        .aria_size_of_set(STEPS.len())
                        .flex()
                        .items_center()
                        .gap(sp(12.))
                        .child(
                            div()
                                .flex_none()
                                .w(sp(72.))
                                .h(sp(48.))
                                .rounded(px(shape::CONTROL_RADIUS))
                                .bg(color(palette.frame))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    gpui_kit::svg()
                                        .path(*drawing)
                                        .w(sp(60.))
                                        .h(sp(40.))
                                        .text_color(color(palette.secondary)),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(sp(13.5))
                                .line_height(sp(19.))
                                .text_color(color(palette.text))
                                .child(format!("{}. {said}", at + 1)),
                        )
                })),
        )
}

pub fn render(
    shell: &mut Shell,
    phase: Phase,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    let palette = look.palette;
    let title = parts::title(&shell.title, look.tr("paste-title"), &look);
    let Some(form) = shell.paste.as_ref() else {
        return div().into_any_element();
    };
    let (revealed, judged, checking) = (form.revealed, form.judged, form.checking);
    let valid = judged == Some(Field::Valid);
    let invalid = judged == Some(Field::Invalid);
    let field = form.input.clone();

    let (reveal, eye) = if revealed {
        ("paste-hide", assets::HIDE)
    } else {
        ("paste-show", assets::SHOW)
    };
    let pasted = cx.entity().downgrade();

    let entry = div()
        .flex_1()
        .min_w(sp(300.))
        .max_w(sp(420.))
        .flex()
        .flex_col()
        .gap(sp(10.))
        .child(
            div()
                .font_family(MONO)
                .font_weight(FontWeight::MEDIUM)
                .text_size(sp(13.))
                .text_color(color(palette.text))
                .child(parts::said("paste-field-label", look.tr("paste-field"))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(8.))
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&field)
                            .id("session-id")
                            .aria_label(look.tr("paste-field"))
                            // What a password is to a screen reader: said to
                            // be there, never read out.
                            .content_type(InputContentType::Password)
                            .disabled(checking)
                            .large()
                            .on_paste(move |_, _, cx| {
                                pasted
                                    .update(cx, |shell, _| shell.pasted_into_the_field())
                                    .ok();
                                false
                            }),
                    ),
                )
                .child(
                    parts::icon_button("paste-reveal", look.tr(reveal), eye, !checking, &look)
                        .on_click(cx.listener(|shell, _, window, cx| shell.reveal(window, cx))),
                )
                .child(
                    parts::button(
                        "paste-paste",
                        look.tr("paste-paste"),
                        Kind::Secondary,
                        !checking,
                        &look,
                    )
                    .h(sp(parts::FIELD_HEIGHT))
                    .px(sp(14.))
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.paste_from_the_clipboard(window, cx);
                    })),
                ),
        )
        // What is wrong with what is in the field, said under it and to a
        // screen reader when it is judged.
        .child(
            div()
                .min_h(sp(20.))
                .flex()
                .items_center()
                .gap(sp(6.))
                .text_size(sp(13.))
                .text_color(color(palette.error))
                .when(invalid, |hint| {
                    hint.child(icon(assets::ERROR, 14.).text_color(color(palette.error)))
                        .child(parts::said("paste-hint", look.tr("paste-hint-invalid")))
                }),
        )
        .child(
            parts::button(
                "paste-submit",
                look.tr(if checking {
                    "paste-checking"
                } else {
                    "paste-submit"
                }),
                Kind::Primary,
                valid && !checking,
                &look,
            )
            .when(checking, |button| {
                button.child(parts::ring(16., &look, window))
            })
            .on_click(cx.listener(|shell, _, window, cx| shell.submit_the_paste(window, cx))),
        )
        .child(div().flex().child(
            parts::button("back", look.tr("back"), Kind::Secondary, true, &look).on_click(
                cx.listener(|shell, _, window, cx| {
                    shell.leave_the_sign_in();
                    shell.happened(Event::Back, window, cx);
                }),
            ),
        ));

    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(sp(28.))
        .px(sp(32.))
        .py(sp(40.))
        .relative()
        .top(px(phase.rise))
        .opacity(phase.presence)
        .child(title)
        .child(
            div()
                .w_full()
                .flex()
                .flex_wrap()
                .justify_center()
                .items_start()
                .gap(sp(40.))
                .child(entry)
                .child(tutorial(&look)),
        )
        .into_any_element()
}
