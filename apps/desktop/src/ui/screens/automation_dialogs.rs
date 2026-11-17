//! What opens over the editor: the small menu of a step, the configuration
//! of a step in a dialog, and the question asked when what is left cannot
//! run. **Each keeps focus inside it, closes on Escape, and hands focus back
//! to the canvas** (`Shell::dismiss`).
//!
//! **A dialog holds its fields until "Save"**: nothing of what is typed is
//! kept before, and "Save" is there only while the engine's own rules find
//! nothing wrong with what was typed.

use gpui_kit::base::{Button, FocusTrapElement as _, TestSupportExt as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, AnyElement, Context, Entity, FocusHandle, FontWeight, MouseButton, Role, Toggled,
    anchored, deferred, div, px,
};
use snob_core::automation::{Only, Step, VideoQuality, WebhookFormat, can_join};

use super::words::{problem_words, step_detail, step_name};
use crate::state::steps::problems_of;
use crate::theme::tokens::{frame, shape};
use crate::ui::assets;
use crate::ui::automations::Item;
use crate::ui::automations_editor::{Dialog, Fields};
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts::{self, Kind, icon};
use crate::ui::shell::Shell;

/// Everything of the editor that is open over the window.
pub fn render(shell: &mut Shell, look: &Look, cx: &mut Context<Shell>) -> Vec<AnyElement> {
    let mut over = Vec::new();
    over.extend(menu(shell, look, cx));
    over.extend(configuration(shell, look, cx));
    over.extend(question(shell, look, cx));
    over
}

/// A menu's item: a button across the menu.
fn item(
    id: impl Into<gpui_kit::ElementId>,
    label: String,
    drawing: &'static str,
    look: &Look,
) -> Button {
    let palette = look.palette;
    Button::new(id)
        .role(Role::MenuItem)
        .accessibility_label(label.clone())
        .w_full()
        .h(sp(36.))
        .px(sp(10.))
        .gap(sp(10.))
        .justify_start()
        .rounded(px(shape::CONTROL_RADIUS))
        .text_size(sp(13.5))
        .text_color(color(palette.text))
        .hover(move |style| style.bg(color(palette.frame)))
        .focus_visible(parts::focus_ring(&palette))
        .cursor_pointer()
        .child(icon(drawing, 16.).text_color(color(palette.text)))
        .child(label)
}

/// The menu of a step, where the press was.
fn menu(shell: &Shell, look: &Look, cx: &mut Context<Shell>) -> Vec<AnyElement> {
    let palette = look.palette;
    let Some(edit) = shell
        .automations
        .as_ref()
        .and_then(|page| page.edit.as_ref())
    else {
        return Vec::new();
    };
    let Some(menu) = &edit.menu else {
        return Vec::new();
    };
    let automation = edit.state.automation();
    let Some(node) = automation.node(menu.id) else {
        return Vec::new();
    };
    let named = |other: &snob_core::automation::Node| match step_detail(&other.step, look) {
        Some(detail) => format!("{} ({detail})", step_name(&other.step, look)),
        None => step_name(&other.step, look),
    };
    let mut items: Vec<Button> = Vec::new();
    let enabled = node.enabled;
    items.push(
        item(
            "node-menu-enabled",
            look.tr(if enabled {
                "node-menu-disable"
            } else {
                "node-menu-enable"
            }),
            if enabled { assets::HIDE } else { assets::SHOW },
            look,
        )
        .on_click(cx.listener(move |shell, _, window, cx| {
            shell.enable_the_step(!enabled, window, cx);
        })),
    );
    let configure = if matches!(node.step, Step::Schedule { .. }) {
        "node-menu-configure-trigger"
    } else {
        "node-menu-configure"
    };
    // A manual trigger has nothing to configure.
    if !matches!(node.step, Step::Manual {}) {
        items.push(
            item(
                "node-menu-configure",
                look.tr(configure),
                assets::SETTINGS,
                look,
            )
            .on_click(cx.listener(|shell, _, window, cx| {
                shell.open_the_configuration(window, cx);
            })),
        );
    }
    for other in &automation.nodes {
        let joined = automation
            .edges
            .iter()
            .any(|edge| edge.from == node.id && edge.to == other.id);
        let to = other.id;
        if joined {
            items.push(
                item(
                    ("node-menu-disconnect", to as usize),
                    look.tr_with("node-menu-disconnect", &[("step", &named(other))]),
                    assets::CLOSE,
                    look,
                )
                .on_click(cx.listener(move |shell, _, window, cx| {
                    shell.join_the_steps(to, false, window, cx);
                })),
            );
        } else if other.id != node.id && can_join(node.step.stage(), other.step.stage()) {
            items.push(
                item(
                    ("node-menu-connect", to as usize),
                    look.tr_with("node-menu-connect", &[("step", &named(other))]),
                    assets::BRANCH,
                    look,
                )
                .on_click(cx.listener(move |shell, _, window, cx| {
                    shell.join_the_steps(to, true, window, cx);
                })),
            );
        }
    }
    items.push(
        item(
            "node-menu-remove",
            look.tr("node-menu-remove"),
            assets::CLOSE,
            look,
        )
        .text_color(color(palette.error))
        .on_click(cx.listener(|shell, _, window, cx| shell.remove_the_step(window, cx))),
    );

    let backdrop = div()
        .id("node-menu-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|shell, _, window, cx| shell.close_the_node_menu(window, cx)),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|shell, _, window, cx| shell.close_the_node_menu(window, cx)),
        )
        .into_any_element();
    let shown = div()
        .id("node-menu")
        .test_support()
        .role(Role::Menu)
        .aria_label(look.tr("node-menu"))
        .occlude()
        .w(sp(300.))
        .max_h(sp(420.))
        .overflow_y_scroll()
        .p(sp(6.))
        .bg(color(palette.panel))
        .border_1()
        .border_color(color(palette.control_line))
        .rounded(px(shape::CARD_RADIUS))
        .shadow_lg()
        .child(
            div()
                .flex()
                .flex_col()
                .children(items)
                .focus_trap("node-menu-items", &menu.focus),
        );
    let placed = deferred(
        anchored()
            .position(menu.at)
            .anchor(Anchor::TopLeft)
            .snap_to_window_with_margin(px(8.))
            .child(shown),
    )
    .with_priority(1)
    .into_any_element();
    vec![backdrop, placed]
}

/// A dialog over the whole window: a backdrop that holds the pointer, and
/// the card, its focus kept inside.
fn modal(
    id: &'static str,
    title: String,
    wide: bool,
    body: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    trap: &FocusHandle,
    look: &Look,
) -> AnyElement {
    let palette = look.palette;
    let inner = div()
        .flex()
        .flex_col()
        .gap(sp(14.))
        .min_h_0()
        .max_h_full()
        .child(
            div()
                .flex_none()
                .id((id, 1usize))
                .test_support()
                .role(Role::Heading)
                .aria_level(2)
                .aria_label(title.clone())
                .font_weight(FontWeight::BOLD)
                .text_size(sp(18.))
                .line_height(sp(26.))
                .text_color(color(palette.text))
                .child(title.clone()),
        )
        .child(
            div()
                .id((id, 2usize))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(sp(12.))
                .children(body),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(sp(10.))
                .pt(sp(6.))
                .children(actions),
        )
        .focus_trap((id, 3usize), trap);
    let card = div()
        .id(id)
        .test_support()
        .role(Role::Dialog)
        .aria_label(title)
        .occlude()
        .w(sp(if wide { 540. } else { 460. }))
        .max_w_full()
        .max_h_full()
        .flex()
        .flex_col()
        .p(sp(24.))
        .bg(color(palette.panel))
        .border_1()
        .border_color(color(palette.control_line))
        .rounded(px(shape::CARD_RADIUS))
        .shadow_lg()
        .child(inner);
    div()
        .id((id, 4usize))
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .p(px(frame::SIDE * 2.))
        .bg(color_at(palette.frame, 0.72))
        .child(card)
        .into_any_element()
}

/// The question asked on leaving what cannot run, with what is wrong.
fn question(shell: &Shell, look: &Look, cx: &mut Context<Shell>) -> Vec<AnyElement> {
    let palette = look.palette;
    let Some(leave) = shell
        .automations
        .as_ref()
        .and_then(|page| page.edit.as_ref())
        .and_then(|edit| edit.leaving.as_ref())
    else {
        return Vec::new();
    };
    let wrong: Vec<AnyElement> = leave
        .problems
        .iter()
        .enumerate()
        .map(|(at, problem)| {
            let words = problem_words(problem, look);
            div()
                .id(("leave-problem", at))
                .test_support()
                .role(Role::ListItem)
                .aria_label(words.clone())
                .flex()
                .gap(sp(8.))
                .text_size(sp(14.))
                .line_height(sp(21.))
                .text_color(color(palette.text))
                .child(
                    icon(assets::ERROR, 16.)
                        .mt(px(2.))
                        .text_color(color(palette.error)),
                )
                .child(words)
                .into_any_element()
        })
        .collect();
    let body = vec![
        div()
            .text_size(sp(14.))
            .line_height(sp(21.))
            .text_color(color(palette.secondary))
            .child(parts::said("leave-body", look.tr("leave-body")))
            .into_any_element(),
        div()
            .id("leave-problems")
            .test_support()
            .role(Role::List)
            .aria_label(look.tr("leave-problems"))
            .flex()
            .flex_col()
            .gap(sp(6.))
            .children(wrong)
            .into_any_element(),
    ];
    let actions = vec![
        parts::button(
            "leave-discard",
            look.tr("leave-discard"),
            Kind::Secondary,
            true,
            look,
        )
        .on_click(cx.listener(|shell, _, window, cx| shell.discard_the_changes(window, cx)))
        .into_any_element(),
        parts::button(
            "leave-keep",
            look.tr("leave-keep"),
            Kind::Primary,
            true,
            look,
        )
        .track_focus(&leave.cancel)
        .on_click(cx.listener(|shell, _, window, cx| shell.keep_editing(window, cx)))
        .into_any_element(),
    ];
    vec![modal(
        "leave-question",
        look.tr("leave-title"),
        false,
        body,
        actions,
        &leave.focus,
        look,
    )]
}

/// A field with its label above it.
fn labeled(id: &'static str, label: String, input: &Entity<InputState>, look: &Look) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .text_size(sp(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(look.palette.text))
                .child(label.clone()),
        )
        .child(Input::new(input).id(id).aria_label(label))
        .into_any_element()
}

/// Small print under a field.
fn small(id: &'static str, text: String, look: &Look) -> AnyElement {
    div()
        .text_size(sp(12.5))
        .line_height(sp(18.))
        .text_color(color(look.palette.secondary))
        .child(parts::said(id, text))
        .into_any_element()
}

/// A box that is ticked or not, and its words: a press anywhere on them
/// changes it.
fn checkbox(id: &'static str, label: String, on: bool, look: &Look) -> Button {
    let palette = look.palette;
    Button::new(id)
        .role(Role::CheckBox)
        .accessibility_label(label.clone())
        .aria_toggled(if on { Toggled::True } else { Toggled::False })
        .w_full()
        .min_h(sp(28.))
        .flex()
        .items_start()
        .justify_start()
        .gap(sp(10.))
        .rounded(px(shape::CONTROL_RADIUS))
        .focus_visible(parts::focus_ring(&palette))
        .cursor_pointer()
        .text_size(sp(14.))
        .line_height(sp(21.))
        .text_color(color(palette.text))
        .child(
            div()
                .flex_none()
                .mt(px(2.))
                .size(sp(18.))
                .rounded(px(4.))
                .border_1()
                .border_color(color(palette.control_line))
                .bg(color(if on { palette.accent } else { palette.panel }))
                .flex()
                .items_center()
                .justify_center()
                .children(
                    on.then(|| icon(assets::CHECK, 14.).text_color(color(palette.on_accent))),
                ),
        )
        .child(div().text_left().child(label))
}

/// What is wrong, in words, under the fields; nothing when nothing is.
fn wrong(id: &'static str, sentences: Vec<String>, look: &Look) -> Option<AnyElement> {
    if sentences.is_empty() {
        return None;
    }
    let palette = look.palette;
    Some(
        div()
            .id(id)
            .test_support()
            .role(Role::Status)
            .aria_label(sentences.join(". "))
            .flex()
            .flex_col()
            .gap(px(4.))
            .children(sentences.into_iter().map(|sentence| {
                div()
                    .flex()
                    .gap(sp(8.))
                    .text_size(sp(13.))
                    .line_height(sp(19.))
                    .text_color(color(palette.error))
                    .child(
                        icon(assets::ERROR, 16.)
                            .mt(px(1.))
                            .text_color(color(palette.error)),
                    )
                    .child(sentence)
            }))
            .into_any_element(),
    )
}

/// The configuration of a step, over the window.
fn configuration(shell: &Shell, look: &Look, cx: &mut Context<Shell>) -> Vec<AnyElement> {
    let Some(edit) = shell
        .automations
        .as_ref()
        .and_then(|page| page.edit.as_ref())
    else {
        return Vec::new();
    };
    let Some(dialog) = &edit.dialog else {
        return Vec::new();
    };
    let automation = edit.state.automation();
    let Some(node) = automation.node(dialog.node) else {
        return Vec::new();
    };
    let in_use = shell.active().map(|account| account.named.pk);
    let now = snob_core::clock::now();
    let typed = dialog.step(&node.step, in_use, now, cx);
    let mut problems: Vec<String> = match (&dialog.fields, &typed) {
        (Fields::Schedule { .. }, _) => dialog
            .schedule_form(cx)
            .and_then(|form| form.problem())
            .into_iter()
            .collect(),
        (_, Some(step)) => problems_of(automation, node.id, step.clone())
            .iter()
            .map(|problem| problem_words(problem, look))
            .collect(),
        (_, None) => Vec::new(),
    };
    problems.dedup();
    let valid = problems.is_empty() && typed.is_some();
    let (title, body) = match &dialog.fields {
        Fields::Schedule { .. } => (
            look.tr("dialog-schedule-title"),
            schedule_body(dialog, look),
        ),
        Fields::Action { .. } => (
            look.tr_with(
                "dialog-configure-title",
                &[("name", &step_name(&node.step, look))],
            ),
            action_body(dialog, look, cx),
        ),
        Fields::Filter { .. } => (
            look.tr_with(
                "dialog-configure-title",
                &[("name", &step_name(&node.step, look))],
            ),
            filter_body(dialog, look, cx),
        ),
        Fields::Webhook { .. } => (
            look.tr_with(
                "dialog-configure-title",
                &[("name", &step_name(&node.step, look))],
            ),
            webhook_body(dialog, look, cx),
        ),
    };
    let mut body = body;
    body.extend(wrong("dialog-problems", problems, look));
    let actions = vec![
        parts::button(
            "dialog-cancel",
            look.tr("cancel"),
            Kind::Secondary,
            true,
            look,
        )
        .track_focus(&dialog.cancel)
        .on_click(cx.listener(|shell, _, window, cx| {
            shell.close_the_configuration(window, cx);
        }))
        .into_any_element(),
        parts::button(
            "dialog-save",
            look.tr("dialog-save"),
            Kind::Primary,
            valid,
            look,
        )
        .on_click(cx.listener(|shell, _, window, cx| {
            shell.apply_the_configuration(window, cx);
        }))
        .into_any_element(),
    ];
    vec![modal(
        "step-dialog",
        title,
        true,
        body,
        actions,
        &dialog.focus,
        look,
    )]
}

fn schedule_body(dialog: &Dialog, look: &Look) -> Vec<AnyElement> {
    let Fields::Schedule {
        every,
        on,
        at,
        cron,
        jitter,
    } = &dialog.fields
    else {
        return Vec::new();
    };
    vec![
        small(
            "dialog-schedule-note",
            look.tr("dialog-schedule-note"),
            look,
        ),
        labeled("schedule-every", look.tr("schedule-every"), every, look),
        labeled("schedule-on", look.tr("schedule-on"), on, look),
        labeled("schedule-at", look.tr("schedule-at"), at, look),
        labeled("schedule-cron", look.tr("schedule-cron"), cron, look),
        labeled("schedule-jitter", look.tr("schedule-jitter"), jitter, look),
    ]
}

fn filter_body(dialog: &Dialog, look: &Look, cx: &mut Context<Shell>) -> Vec<AnyElement> {
    let Fields::Filter { only } = &dialog.fields else {
        return Vec::new();
    };
    let chosen = *only;
    vec![
        parts::segmented("filter-choice", look.tr("filter-choice"), look)
            .flex_wrap()
            .children(Only::ALL.into_iter().map(|one| {
                parts::segment(
                    Item::Filter(one).id(),
                    look.tr(Item::Filter(one).id()),
                    one == chosen,
                    look,
                )
                .on_click(cx.listener(move |shell, _, _, cx| shell.choose_the_filter(one, cx)))
            }))
            .into_any_element(),
    ]
}

fn action_body(dialog: &Dialog, look: &Look, cx: &mut Context<Shell>) -> Vec<AnyElement> {
    let Fields::Action {
        username, save_to, ..
    } = &dialog.fields
    else {
        return Vec::new();
    };
    let Some(form) = dialog.action_form(cx) else {
        return Vec::new();
    };
    let mut body: Vec<AnyElement> = Vec::new();
    if !form.what().is_about_another() {
        body.push(
            parts::segmented("action-whose", look.tr("action-whose"), look)
                .child(
                    parts::segment("action-mine", look.tr("action-mine"), form.mine, look)
                        .on_click(cx.listener(|shell, _, _, cx| shell.read_as_mine(true, cx))),
                )
                .child(
                    parts::segment("action-other", look.tr("action-other"), !form.mine, look)
                        .on_click(cx.listener(|shell, _, _, cx| shell.read_as_mine(false, cx))),
                )
                .into_any_element(),
        );
    }
    if !form.mine {
        body.push(labeled(
            "action-username",
            look.tr("action-username"),
            username,
            look,
        ));
    }
    if form.needs_consent() {
        let name = form.username.trim().trim_start_matches('@').to_string();
        body.push(
            checkbox(
                "action-consent",
                look.tr_with("action-consent", &[("name", &name)]),
                form.consent,
                look,
            )
            .on_click(cx.listener(move |shell, _, _, cx| {
                shell.consent_to_the_lists(!form.consent, cx);
            }))
            .into_any_element(),
        );
        body.push(small(
            "action-consent-note",
            look.tr("action-consent-note"),
            look,
        ));
    }
    if form.saves_files() {
        body.push(
            div()
                .flex()
                .items_end()
                .gap(sp(8.))
                .child(div().flex_1().child(labeled(
                    "action-folder",
                    look.tr("action-folder"),
                    save_to,
                    look,
                )))
                .child(
                    parts::small_button("action-choose", look.tr("action-choose"), true, look)
                        .mb(px(7.))
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.choose_the_folder(window, cx);
                        })),
                )
                .into_any_element(),
        );
        body.push(small(
            "action-folder-note",
            look.tr("action-folder-note"),
            look,
        ));
    }
    if form.saves_videos() {
        let compat = form.quality == Some(VideoQuality::Compat);
        body.push(
            parts::segmented("action-quality", look.tr("action-quality"), look)
                .child(
                    parts::segment("action-best", look.tr("action-best"), !compat, look).on_click(
                        cx.listener(|shell, _, _, cx| {
                            shell.choose_the_quality(Some(VideoQuality::Best), cx);
                        }),
                    ),
                )
                .child(
                    parts::segment("action-compat", look.tr("action-compat"), compat, look)
                        .on_click(cx.listener(|shell, _, _, cx| {
                            shell.choose_the_quality(Some(VideoQuality::Compat), cx);
                        })),
                )
                .into_any_element(),
        );
    }
    body
}

fn webhook_body(dialog: &Dialog, look: &Look, cx: &mut Context<Shell>) -> Vec<AnyElement> {
    let Fields::Webhook {
        url,
        format,
        heartbeat,
        headers,
    } = &dialog.fields
    else {
        return Vec::new();
    };
    let (chosen, beating) = (*format, *heartbeat);
    let mut body = vec![
        labeled("webhook-url", look.tr("webhook-url"), url, look),
        parts::segmented("webhook-format", look.tr("webhook-format"), look)
            .flex_wrap()
            .children(WebhookFormat::ALL.into_iter().map(|one| {
                parts::segment(
                    ("webhook-format-one", one as usize),
                    look.tr(&format!("webhook-format-{}", one.as_str())),
                    one == chosen,
                    look,
                )
                .on_click(cx.listener(move |shell, _, _, cx| shell.choose_the_format(one, cx)))
            }))
            .into_any_element(),
        checkbox(
            "webhook-heartbeat",
            look.tr("webhook-heartbeat"),
            beating,
            look,
        )
        .on_click(cx.listener(|shell, _, _, cx| shell.toggle_the_heartbeat(cx)))
        .into_any_element(),
        small(
            "webhook-heartbeat-note",
            look.tr("webhook-heartbeat-note"),
            look,
        ),
        div()
            .id("webhook-headers-title")
            .test_support()
            .role(Role::Heading)
            .aria_level(3)
            .aria_label(look.tr("webhook-headers"))
            .text_size(sp(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(color(look.palette.text))
            .child(look.tr("webhook-headers"))
            .into_any_element(),
        small(
            "webhook-headers-note",
            look.tr("webhook-headers-note"),
            look,
        ),
    ];
    for (at, row) in headers.iter().enumerate() {
        body.push(
            div()
                .id(("webhook-header", at))
                .flex()
                .items_center()
                .gap(sp(8.))
                .child(
                    div().flex_1().child(
                        Input::new(&row.name)
                            .id(("webhook-header-name", at))
                            .aria_label(look.tr("webhook-header-name")),
                    ),
                )
                .child(
                    div().flex_1().child(
                        Input::new(&row.value)
                            .id(("webhook-header-value", at))
                            .aria_label(look.tr("webhook-header-value")),
                    ),
                )
                .child(
                    parts::icon_button(
                        ("webhook-header-remove", at),
                        look.tr("webhook-header-remove"),
                        assets::CLOSE,
                        true,
                        look,
                    )
                    .on_click(cx.listener(move |shell, _, _, cx| shell.remove_a_header(at, cx))),
                )
                .into_any_element(),
        );
    }
    body.push(
        div()
            .child(
                parts::small_icon_button(
                    "webhook-header-add",
                    look.tr("webhook-header-add"),
                    assets::ADD,
                    look,
                )
                .on_click(cx.listener(|shell, _, window, cx| shell.add_a_header(window, cx))),
            )
            .into_any_element(),
    );
    body
}
