//! What opens over the editor: the small menu of a step, the configuration
//! of a step in a dialog, and the question asked when what is left cannot
//! run. **Each keeps focus inside it, closes on Escape, and hands focus back
//! to the canvas** (`Shell::dismiss`).
//!
//! **A dialog holds its fields until "Save"**: nothing of what is typed is
//! kept before, and "Save" is there only while the engine's own rules find
//! nothing wrong with what was typed.
//!
//! **Every step's dialog is laid out one way** ([`modal`], [`group`]): the
//! step's drawing and name on top, each thing asked under its label with
//! its help below it, and Cancel left of Save, which says why it is off.

use gpui_kit::base::{Button, FocusTrapElement as _, POPUP_PRIORITY, TestSupportExt as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, AnyElement, Context, Entity, FocusHandle, FontWeight, MouseButton, Role, anchored,
    deferred, div, px,
};
use snob_core::automation::{Only, Step, VideoQuality, WATCH_TOML, WebhookFormat, can_join};
use snob_core::watch::schedule::Nth;

use super::words::{
    MONDAY_FIRST, and_list, month_short_words, month_words, problem_words, step_detail, step_name,
    weekday_short_words, weekday_words, wrong_words,
};
use crate::backend::Account;
use crate::state::schedule::{
    By, DayMode, EVERY_MONTHS, Field, Frequency, Refused, ScheduleForm, Unit, toggle,
};
use crate::state::steps::{Suggestion, problems_of};
use crate::theme::tokens::{frame, shape};
use crate::ui::assets;
use crate::ui::automations::Item;
use crate::ui::automations_editor::{Dialog, Fields, ScheduleFields, Suggesting};
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts::{self, Kind, icon};
use crate::ui::shell::Shell;
use crate::ui::{SuggestDown, SuggestUp};

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
        .child(
            icon(drawing, 16.)
                .flex_none()
                .text_color(color(palette.text)),
        )
        .child(div().flex_1().min_w_0().text_left().truncate().child(label))
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
    // A step taken an arrow off is named alone, as the canvas names it; its
    // detail follows only to tell it from another joined step of that name,
    // and its place among them when the detail is the same too.
    let joined_to = |other: &snob_core::automation::Node| {
        automation
            .edges
            .iter()
            .any(|edge| edge.from == node.id && edge.to == other.id)
    };
    let told_apart = |other: &snob_core::automation::Node| {
        let name = step_name(&other.step, look);
        let alike = automation
            .nodes
            .iter()
            .any(|one| one.id != other.id && joined_to(one) && step_name(&one.step, look) == name);
        match step_detail(&other.step, look).filter(|_| alike) {
            Some(detail) => format!("{name} \u{b7} {detail}"),
            None => name,
        }
    };
    let named_alone = |other: &snob_core::automation::Node| {
        let label = told_apart(other);
        let twins: Vec<_> = automation
            .nodes
            .iter()
            .filter(|one| joined_to(one) && told_apart(one) == label)
            .map(|one| one.id)
            .collect();
        match twins.iter().position(|id| *id == other.id) {
            Some(at) if twins.len() > 1 => format!("{label} \u{b7} {}", at + 1),
            _ => label,
        }
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
        let to = other.id;
        if joined_to(other) {
            items.push(
                item(
                    ("node-menu-disconnect", to as usize),
                    look.tr_with("node-menu-disconnect", &[("step", &named_alone(other))]),
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
    .with_priority(POPUP_PRIORITY)
    .into_any_element();
    vec![backdrop, placed]
}

/// A dialog over the whole window: a backdrop that holds the pointer, and
/// the card, its focus kept inside. **Every dialog of the editor is this
/// one shape**: its title on top, beside the step's `drawing` when it is
/// about a step, the body under it with room for every focus ring, and the
/// buttons at the foot, the one that does it last.
#[expect(
    clippy::too_many_arguments,
    reason = "a dialog's title, drawing, width, body, buttons and focus"
)]
fn modal(
    id: &'static str,
    title: String,
    drawing: Option<&'static str>,
    wide: bool,
    body: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    trap: &FocusHandle,
    look: &Look,
) -> AnyElement {
    let palette = look.palette;
    let heading = div()
        .id((id, 1usize))
        .test_support()
        .role(Role::Heading)
        .aria_level(2)
        .aria_label(title.clone())
        .flex_1()
        .min_w_0()
        .font_weight(FontWeight::BOLD)
        .text_size(sp(18.))
        .line_height(sp(26.))
        .text_color(color(palette.text))
        .child(title.clone());
    let inner = div()
        .flex()
        .flex_col()
        .gap(sp(16.))
        .min_h_0()
        .max_h_full()
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(sp(12.))
                .children(drawing.map(|drawing| {
                    div()
                        .flex_none()
                        .size(sp(26.))
                        .rounded(px(shape::CONTROL_RADIUS))
                        .bg(color_at(palette.accent, 0.12))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(drawing, 16.).text_color(color(palette.accent)))
                }))
                .child(heading),
        )
        .child(parts::room_for_focus(
            div()
                .id((id, 2usize))
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(sp(14.))
                .children(body),
        ))
        .child(
            div()
                .flex_none()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(sp(10.))
                .pt(sp(12.))
                .border_t_1()
                .border_color(color(palette.line))
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
        .min_w(sp(320.))
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
        None,
        false,
        body,
        actions,
        &leave.focus,
        look,
    )]
}

/// One thing a dialog asks: its label above it, when it has one, then
/// what the caller puts in, the control and the help under it.
fn group(label: Option<String>, look: &Look) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(4.))
        .children(label.map(|label| label_text(label, look)))
}

/// A choice with its label above it.
fn titled(label: String, control: AnyElement, look: &Look) -> AnyElement {
    group(Some(label), look).child(control).into_any_element()
}

/// A field with its label above it.
fn labeled(id: &'static str, label: String, input: &Entity<InputState>, look: &Look) -> AnyElement {
    group(Some(label.clone()), look)
        .child(Input::new(input).id(id).aria_label(label))
        .into_any_element()
}

/// Help under what it is about.
fn small(id: &'static str, text: String, look: &Look) -> AnyElement {
    div()
        .text_size(sp(12.5))
        .line_height(sp(18.))
        .text_color(color(look.palette.secondary))
        .child(parts::said(id, text))
        .into_any_element()
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
    let refused = dialog.schedule_form(cx).and_then(|form| form.problem(now));
    let mut problems: Vec<String> = match (&dialog.fields, &typed) {
        (Fields::Schedule(_), _) => Vec::new(),
        (_, Some(step)) => problems_of(automation, node.id, step.clone())
            .iter()
            .map(|problem| problem_words(problem, look))
            .collect(),
        (_, None) => Vec::new(),
    };
    problems.dedup();
    let why_not = refused
        .as_ref()
        .map(|refused| wrong_words(&refused.wrong, look))
        .or_else(|| problems.first().cloned());
    let valid = why_not.is_none() && typed.is_some();
    let (title, body) = match &dialog.fields {
        Fields::Schedule(_) => (
            look.tr("dialog-schedule-title"),
            schedule_body(dialog, refused.as_ref(), look, cx),
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
            webhook_body(dialog, automation.id == WATCH_TOML, look, cx),
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
        .when_some(why_not, |save, why| {
            save.aria_description(why.clone()).tooltip(parts::tip(why))
        })
        .on_click(cx.listener(|shell, _, window, cx| {
            shell.apply_the_configuration(window, cx);
        }))
        .into_any_element(),
    ];
    vec![modal(
        "step-dialog",
        title,
        Some(Item::of(&node.step).drawing()),
        true,
        body,
        actions,
        &dialog.focus,
        look,
    )]
}

/// A label above what it names.
fn label_text(text: String, look: &Look) -> gpui_kit::Div {
    div()
        .text_size(sp(13.))
        .line_height(sp(18.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color(look.palette.text))
        .child(text)
}

/// What is wrong with the schedule, under `field` when it is about it.
fn wrong_under(field: Field, refused: Option<&Refused>, body: &mut Vec<AnyElement>, look: &Look) {
    if let Some(refused) = refused.filter(|refused| refused.field == field) {
        body.extend(wrong(
            "dialog-problems",
            vec![wrong_words(&refused.wrong, look)],
            look,
        ));
    }
}

/// The first letter of `words` as a capital.
fn capitalized(words: &str) -> String {
    let mut letters = words.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

fn frequency_id(frequency: Frequency) -> &'static str {
    match frequency {
        Frequency::Year => "schedule-frequency-year",
        Frequency::Month => "schedule-frequency-month",
        Frequency::Week => "schedule-frequency-week",
        Frequency::Day => "schedule-frequency-day",
        Frequency::Advanced => "schedule-frequency-advanced",
    }
}

/// Which one of a month's weekdays, said alone: "First".
fn nth_id(nth: Nth) -> &'static str {
    match nth {
        Nth::First => "schedule-nth-first",
        Nth::Second => "schedule-nth-second",
        Nth::Third => "schedule-nth-third",
        Nth::Fourth => "schedule-nth-fourth",
        Nth::Last => "schedule-nth-last",
    }
}

fn unit_id(unit: Unit) -> &'static str {
    match unit {
        Unit::Minutes => "schedule-unit-minutes",
        Unit::Hours => "schedule-unit-hours",
        Unit::Days => "schedule-unit-days",
        Unit::Weeks => "schedule-unit-weeks",
    }
}

/// The count of a schedule, its label above it, and a button on each side.
fn counted(
    label: String,
    input: &Entity<InputState>,
    look: &Look,
    cx: &mut Context<Shell>,
) -> AnyElement {
    group(Some(label.clone()), look)
        .child(parts::stepper(
            "schedule-count",
            label,
            input,
            look,
            |shell: &mut Shell, by, window, cx| shell.step_the_count(by, window, cx),
            cx,
        ))
        .into_any_element()
}

/// The times of day a schedule runs at: one field each, a button that
/// takes one away, and one that adds one.
fn times_list(fields: &ScheduleFields, look: &Look, cx: &mut Context<Shell>) -> AnyElement {
    let rows = fields.times.iter().enumerate().map(|(at, row)| {
        let number = (at + 1).to_string();
        div()
            .id(("schedule-time-row", at))
            .flex()
            .items_center()
            .gap(sp(8.))
            .child(
                div().w(sp(112.)).child(
                    Input::new(&row.input)
                        .id(("schedule-time", at))
                        .aria_label(look.tr_with("schedule-time", &[("number", &number)])),
                ),
            )
            .child(
                parts::icon_button(
                    ("schedule-time-remove", at),
                    look.tr_with("schedule-time-remove", &[("number", &number)]),
                    assets::CLOSE,
                    true,
                    look,
                )
                .on_click(cx.listener(move |shell, _, _, cx| shell.remove_a_time(at, cx))),
            )
    });
    group(Some(look.tr("schedule-times")), look)
        .children(rows)
        .child(
            div().child(
                parts::small_icon_button(
                    "schedule-time-add",
                    look.tr("schedule-time-add"),
                    assets::ADD,
                    look,
                )
                .on_click(cx.listener(|shell, _, window, cx| shell.add_a_time(window, cx))),
            ),
        )
        .into_any_element()
}

/// Which days of a month: by date, as chips, or one weekday of it.
fn month_days(
    form: &ScheduleForm,
    refused: Option<&Refused>,
    body: &mut Vec<AnyElement>,
    look: &Look,
    cx: &mut Context<Shell>,
) {
    let choices = &form.choices;
    let by = choices.by;
    let days = group(Some(look.tr("schedule-by")), look).child(
        parts::segmented("schedule-by", look.tr("schedule-by"), look)
            .flex_wrap()
            .child(
                parts::segment(
                    "schedule-by-date",
                    look.tr("schedule-by-date"),
                    by == By::Date,
                    look,
                )
                .on_click(cx.listener(|shell, _, _, cx| {
                    shell.change_the_schedule(|choices| choices.by = By::Date, cx);
                })),
            )
            .child(
                parts::segment(
                    "schedule-by-weekday",
                    look.tr("schedule-by-weekday"),
                    by == By::Weekday,
                    look,
                )
                .on_click(cx.listener(|shell, _, _, cx| {
                    shell.change_the_schedule(|choices| choices.by = By::Weekday, cx);
                })),
            ),
    );
    let days = match by {
        By::Date => days
            .child(
                parts::chips("schedule-dates", look.tr("schedule-dates"), look).children(
                    (1..=31u32).map(|date| {
                        let day = date.to_string();
                        parts::chip(
                            ("schedule-date", date as usize),
                            look.tr_with("schedule-date", &[("day", &day)]),
                            day,
                            choices.dates.contains(&date),
                            look,
                        )
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.change_the_schedule(
                                |choices| toggle(&mut choices.dates, date),
                                cx,
                            );
                        }))
                    }),
                ),
            )
            .when(form.some_months_lack_a_date(), |days| {
                days.child(small(
                    "schedule-short-months",
                    look.tr("schedule-short-months"),
                    look,
                ))
            }),
        By::Weekday => days
            .child(
                parts::segmented("schedule-nth", look.tr("schedule-nth"), look)
                    .flex_wrap()
                    .children(Nth::ALL.into_iter().enumerate().map(|(at, nth)| {
                        parts::segment(
                            ("schedule-nth", at),
                            look.tr(nth_id(nth)),
                            nth == choices.nth,
                            look,
                        )
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.change_the_schedule(|choices| choices.nth = nth, cx);
                        }))
                    })),
            )
            .child(
                parts::segmented(
                    "schedule-nth-weekday",
                    look.tr("schedule-nth-weekday"),
                    look,
                )
                .flex_wrap()
                .children(MONDAY_FIRST.into_iter().enumerate().map(|(at, weekday)| {
                    parts::segment(
                        ("schedule-nth-weekday", at),
                        capitalized(&weekday_short_words(weekday, look)),
                        weekday == choices.nth_weekday,
                        look,
                    )
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        shell.change_the_schedule(|choices| choices.nth_weekday = weekday, cx);
                    }))
                })),
            ),
    };
    body.push(days.into_any_element());
    wrong_under(Field::Dates, refused, body, look);
}

/// A schedule's dialog: how often it repeats, then what that asks; or
/// `watch.toml`'s fields in Advanced. What is wrong is said under the
/// field it is about.
fn schedule_body(
    dialog: &Dialog,
    refused: Option<&Refused>,
    look: &Look,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    let Fields::Schedule(fields) = &dialog.fields else {
        return Vec::new();
    };
    let Some(form) = dialog.schedule_form(cx) else {
        return Vec::new();
    };
    let choices = &form.choices;
    let frequency = choices.frequency;
    let mut body: Vec<AnyElement> = vec![titled(
        look.tr("schedule-frequency"),
        parts::segmented("schedule-frequency", look.tr("schedule-frequency"), look)
            .flex_wrap()
            .children(Frequency::ALL.into_iter().map(|one| {
                let id = frequency_id(one);
                parts::segment(id, look.tr(id), one == frequency, look).on_click(cx.listener(
                    move |shell, _, window, cx| shell.choose_the_frequency(one, window, cx),
                ))
            }))
            .into_any_element(),
        look,
    )];
    let floor = |body: &mut Vec<AnyElement>| {
        if form.counts_from_the_last_run() {
            body.push(small("schedule-floor", look.tr("schedule-floor"), look));
        }
    };
    match frequency {
        Frequency::Day => {
            let mode = choices.day_mode;
            body.push(titled(
                look.tr("schedule-day-mode"),
                parts::segmented("schedule-day-mode", look.tr("schedule-day-mode"), look)
                    .flex_wrap()
                    .child(
                        parts::segment(
                            "schedule-day-at",
                            look.tr("schedule-day-at"),
                            mode == DayMode::AtTimes,
                            look,
                        )
                        .on_click(cx.listener(|shell, _, _, cx| {
                            shell.change_the_schedule(|c| c.day_mode = DayMode::AtTimes, cx);
                        })),
                    )
                    .child(
                        parts::segment(
                            "schedule-day-repeat",
                            look.tr("schedule-day-repeat"),
                            mode == DayMode::Repeating,
                            look,
                        )
                        .on_click(cx.listener(|shell, _, _, cx| {
                            shell.change_the_schedule(|c| c.day_mode = DayMode::Repeating, cx);
                        })),
                    )
                    .into_any_element(),
                look,
            ));
            match mode {
                DayMode::AtTimes => {
                    body.push(counted(
                        look.tr("schedule-count-days"),
                        &fields.count,
                        look,
                        cx,
                    ));
                    wrong_under(Field::Count, refused, &mut body, look);
                    floor(&mut body);
                    body.push(times_list(fields, look, cx));
                    wrong_under(Field::Times, refused, &mut body, look);
                }
                DayMode::Repeating => {
                    body.push(counted(
                        look.tr("schedule-count-repeat"),
                        &fields.count,
                        look,
                        cx,
                    ));
                    body.push(titled(
                        look.tr("schedule-unit"),
                        parts::segmented("schedule-unit", look.tr("schedule-unit"), look)
                            .flex_wrap()
                            .children(Unit::ALL.into_iter().map(|unit| {
                                let id = unit_id(unit);
                                parts::segment(id, look.tr(id), unit == choices.unit, look)
                                    .on_click(cx.listener(move |shell, _, _, cx| {
                                        shell.change_the_schedule(|c| c.unit = unit, cx);
                                    }))
                            }))
                            .into_any_element(),
                        look,
                    ));
                    wrong_under(Field::Count, refused, &mut body, look);
                    floor(&mut body);
                }
            }
        }
        Frequency::Week => {
            body.push(counted(
                look.tr("schedule-count-weeks"),
                &fields.count,
                look,
                cx,
            ));
            wrong_under(Field::Count, refused, &mut body, look);
            floor(&mut body);
            body.push(titled(
                look.tr("schedule-weekdays"),
                parts::chips("schedule-weekdays", look.tr("schedule-weekdays"), look)
                    .children(MONDAY_FIRST.into_iter().enumerate().map(|(at, weekday)| {
                        parts::chip(
                            ("schedule-weekday", at),
                            capitalized(&weekday_words(weekday, look)),
                            capitalized(&weekday_short_words(weekday, look)),
                            choices.weekdays.contains(&weekday),
                            look,
                        )
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.change_the_schedule(|c| toggle(&mut c.weekdays, weekday), cx);
                        }))
                    }))
                    .into_any_element(),
                look,
            ));
            wrong_under(Field::Weekdays, refused, &mut body, look);
            body.push(times_list(fields, look, cx));
            wrong_under(Field::Times, refused, &mut body, look);
        }
        Frequency::Month => {
            let every = choices.every_months;
            body.push(titled(
                look.tr("schedule-every-months"),
                parts::segmented(
                    "schedule-every-months",
                    look.tr("schedule-every-months"),
                    look,
                )
                .flex_wrap()
                .children(EVERY_MONTHS.into_iter().map(|count| {
                    let label = if count == 1 {
                        look.tr("schedule-every-month")
                    } else {
                        look.tr_with("schedule-every-n-months", &[("count", &count.to_string())])
                    };
                    parts::segment(
                        ("schedule-every-months", count as usize),
                        label,
                        count == every,
                        look,
                    )
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        shell.change_the_schedule(|c| c.every_months = count, cx);
                    }))
                }))
                .into_any_element(),
                look,
            ));
            if every > 1 {
                let options: Vec<String> = (1..=12)
                    .map(|month| capitalized(&month_words(month, look)))
                    .collect();
                let chosen = choices.first_month.clamp(1, 12) as usize - 1;
                body.push(
                    group(Some(look.tr("schedule-first-month")), look)
                        .child(parts::select(
                            "schedule-first-month",
                            look.tr("schedule-first-month"),
                            &options,
                            chosen,
                            &fields.first_month,
                            look,
                            |shell: &mut Shell, window, cx| {
                                shell.toggle_the_first_months(window, cx)
                            },
                            |shell: &mut Shell, at, window, cx| {
                                shell.choose_the_first_month(at, window, cx);
                            },
                            |shell: &mut Shell, window, cx| {
                                shell.close_the_first_months(window, cx);
                            },
                            cx,
                        ))
                        .into_any_element(),
                );
                let months: Vec<String> = form
                    .lands_on()
                    .into_iter()
                    .map(|month| month_words(month, look))
                    .collect();
                body.push(small(
                    "schedule-lands-on",
                    look.tr_with("schedule-lands-on", &[("months", &and_list(&months, look))]),
                    look,
                ));
            }
            month_days(&form, refused, &mut body, look, cx);
            body.push(times_list(fields, look, cx));
            wrong_under(Field::Times, refused, &mut body, look);
        }
        Frequency::Year => {
            body.push(titled(
                look.tr("schedule-months"),
                parts::chips("schedule-months", look.tr("schedule-months"), look)
                    .children((1..=12u32).map(|month| {
                        parts::chip(
                            ("schedule-month", month as usize),
                            capitalized(&month_words(month, look)),
                            capitalized(&month_short_words(month, look)),
                            choices.months.contains(&month),
                            look,
                        )
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell.change_the_schedule(|c| toggle(&mut c.months, month), cx);
                        }))
                    }))
                    .into_any_element(),
                look,
            ));
            wrong_under(Field::Months, refused, &mut body, look);
            month_days(&form, refused, &mut body, look, cx);
            body.push(times_list(fields, look, cx));
            wrong_under(Field::Times, refused, &mut body, look);
        }
        Frequency::Advanced => {
            let advanced = &fields.advanced;
            if form.kept {
                let kept = look.tr("schedule-advanced-kept");
                body.push(
                    div()
                        .id("schedule-advanced-kept")
                        .test_support()
                        .role(Role::Note)
                        .aria_label(kept.clone())
                        .text_size(sp(13.))
                        .line_height(sp(19.))
                        .text_color(color(look.palette.text))
                        .child(kept)
                        .into_any_element(),
                );
            }
            body.push(small(
                "dialog-schedule-note",
                look.tr("dialog-schedule-note"),
                look,
            ));
            for (id, label, input, field) in [
                (
                    "schedule-every",
                    look.tr("schedule-every"),
                    &advanced.every,
                    Field::Every,
                ),
                (
                    "schedule-on",
                    look.tr("schedule-on"),
                    &advanced.on,
                    Field::On,
                ),
                (
                    "schedule-days-field",
                    look.tr("schedule-days-field"),
                    &advanced.days,
                    Field::DaysTyped,
                ),
                (
                    "schedule-months-field",
                    look.tr("schedule-months-field"),
                    &advanced.months,
                    Field::MonthsTyped,
                ),
                (
                    "schedule-at",
                    look.tr("schedule-at"),
                    &advanced.at,
                    Field::At,
                ),
                (
                    "schedule-cron",
                    look.tr("schedule-cron"),
                    &advanced.cron,
                    Field::Cron,
                ),
                (
                    "schedule-jitter",
                    look.tr("schedule-jitter"),
                    &advanced.jitter,
                    Field::Jitter,
                ),
            ] {
                body.push(labeled(id, label, input, look));
                wrong_under(field, refused, &mut body, look);
            }
        }
    }
    wrong_under(Field::Whole, refused, &mut body, look);
    body
}

fn filter_body(dialog: &Dialog, look: &Look, cx: &mut Context<Shell>) -> Vec<AnyElement> {
    let Fields::Filter { only } = &dialog.fields else {
        return Vec::new();
    };
    let chosen = *only;
    vec![
        group(Some(look.tr("filter-choice")), look)
            .child(
                parts::segmented("filter-choice", look.tr("filter-choice"), look)
                    .flex_wrap()
                    .children(Only::ALL.into_iter().map(|one| {
                        parts::segment(
                            Item::Filter(one).id(),
                            look.tr(Item::Filter(one).id()),
                            one == chosen,
                            look,
                        )
                        .on_click(
                            cx.listener(move |shell, _, _, cx| shell.choose_the_filter(one, cx)),
                        )
                    })),
            )
            .into_any_element(),
    ]
}

/// A row of the names suggested under an account's field: the account's
/// picture as the cache keeps it, its username and its name.
fn known_row(account: &Account, look: &Look) -> (String, String, AnyElement) {
    let palette = look.palette;
    let name = account.named.username.clone().unwrap_or_default();
    let label = match &account.full_name {
        Some(full) => format!("@{name}, {full}"),
        None => format!("@{name}"),
    };
    let shown = div()
        .flex()
        .items_center()
        .gap(sp(10.))
        .min_w_0()
        .child(parts::avatar(account, 28., look, 1.0))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .text_left()
                .child(
                    div()
                        .text_size(sp(13.5))
                        .line_height(sp(19.))
                        .truncate()
                        .child(format!("@{name}")),
                )
                .children(account.full_name.clone().map(|full| {
                    div()
                        .text_size(sp(12.5))
                        .line_height(sp(17.))
                        .text_color(color(palette.secondary))
                        .truncate()
                        .child(full)
                })),
        )
        .into_any_element();
    (name, label, shown)
}

/// The row that takes what is typed as it is: somebody snob has not read.
fn another_row(name: &str, look: &Look) -> (String, String, AnyElement) {
    let palette = look.palette;
    let label = look.tr_with("action-someone-else", &[("username", name)]);
    let shown = div()
        .flex()
        .items_center()
        .gap(sp(10.))
        .min_w_0()
        .child(
            div()
                .flex_none()
                .size(px(28.))
                .rounded_full()
                .bg(color(palette.frame))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(assets::PROFILE, 16.).text_color(color(palette.secondary))),
        )
        .child(div().min_w_0().truncate().child(label.clone()))
        .into_any_element();
    (name.to_string(), label, shown)
}

/// The username of an action, and under it, while it is typed, the accounts
/// known whose name begins with it, then "another person". Up and Down go
/// from the field into the list and along it, Enter or a press takes a row,
/// Escape closes it.
fn username_field(
    username: &Entity<InputState>,
    typed: &str,
    suggest: &Suggesting,
    look: &Look,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let label = look.tr("action-username");
    let rows = suggest.rows_for(typed);
    let open = suggest.open && !rows.is_empty();
    let list = open.then(|| {
        let rows: Vec<AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(at, row)| {
                let (name, label, shown) = match row {
                    Suggestion::Known(found) => known_row(&suggest.found[found], look),
                    Suggestion::Another(name) => another_row(&name, look),
                };
                parts::suggestion(("action-suggestion", at), label, look)
                    .when_some(suggest.rows.get(at), |row, focus| row.track_focus(focus))
                    .child(shown)
                    .on_click(cx.listener(move |shell, _, window, cx| {
                        shell.take_the_suggestion(name.clone(), window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let moving = div()
            .key_context("SnobSuggest")
            .on_action(cx.listener(|shell, _: &SuggestUp, window, cx| {
                shell.move_in_the_suggestions(false, window, cx);
            }))
            .on_action(cx.listener(|shell, _: &SuggestDown, window, cx| {
                shell.move_in_the_suggestions(true, window, cx);
            }))
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(rows)
            .into_any_element();
        parts::suggestions(
            "action-suggestions",
            look.tr("action-suggestions"),
            &suggest.field,
            vec![moving],
            look,
            |shell: &mut Shell, window, cx| {
                shell.close_the_suggestions(window, cx);
            },
            cx,
        )
    });
    let field = div()
        .key_context("SnobSuggest")
        .on_action(cx.listener(|shell, _: &SuggestUp, window, cx| {
            shell.move_in_the_suggestions(false, window, cx);
        }))
        .on_action(cx.listener(|shell, _: &SuggestDown, window, cx| {
            shell.move_in_the_suggestions(true, window, cx);
        }))
        .child(
            Input::new(username)
                .id("action-username")
                .aria_label(label.clone()),
        );
    group(Some(label), look)
        .child(parts::drawn_at(&suggest.field, open, field).w_full())
        .children(list)
        .into_any_element()
}

fn action_body(dialog: &Dialog, look: &Look, cx: &mut Context<Shell>) -> Vec<AnyElement> {
    let Fields::Action {
        username,
        save_to,
        suggest,
        ..
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
            group(Some(look.tr("action-whose")), look)
                .child(
                    parts::segmented("action-whose", look.tr("action-whose"), look)
                        .child(
                            parts::segment("action-mine", look.tr("action-mine"), form.mine, look)
                                .on_click(
                                    cx.listener(|shell, _, _, cx| shell.read_as_mine(true, cx)),
                                ),
                        )
                        .child(
                            parts::segment(
                                "action-other",
                                look.tr("action-other"),
                                !form.mine,
                                look,
                            )
                            .on_click(cx.listener(|shell, _, _, cx| shell.read_as_mine(false, cx))),
                        ),
                )
                .into_any_element(),
        );
    }
    if !form.mine {
        body.push(username_field(username, &form.username, suggest, look, cx));
    }
    if form.needs_consent() {
        let given = form.given();
        body.push(
            group(None, look)
                .child(
                    parts::checkbox(
                        "action-consent",
                        look.tr_with("action-consent", &[("name", form.name())]),
                        given,
                        look,
                    )
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        shell.consent_to_the_lists(!given, cx);
                    })),
                )
                .child(small(
                    "action-consent-note",
                    look.tr("action-consent-note"),
                    look,
                ))
                .into_any_element(),
        );
    }
    if form.saves_files() {
        let label = look.tr("action-folder");
        body.push(
            group(Some(label.clone()), look)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(save_to).id("action-folder").aria_label(label)),
                        )
                        .child(
                            parts::small_button(
                                "action-choose",
                                look.tr("action-choose"),
                                true,
                                look,
                            )
                            .flex_none()
                            .on_click(cx.listener(
                                |shell, _, window, cx| {
                                    shell.choose_the_folder(window, cx);
                                },
                            )),
                        ),
                )
                .child(small(
                    "action-folder-note",
                    look.tr("action-folder-note"),
                    look,
                ))
                .into_any_element(),
        );
    }
    if form.saves_videos() {
        let compat = form.quality == Some(VideoQuality::Compat);
        body.push(
            group(Some(look.tr("action-quality")), look)
                .child(
                    parts::segmented("action-quality", look.tr("action-quality"), look)
                        .flex_wrap()
                        .child(
                            parts::segment("action-best", look.tr("action-best"), !compat, look)
                                .on_click(cx.listener(|shell, _, _, cx| {
                                    shell.choose_the_quality(Some(VideoQuality::Best), cx);
                                })),
                        )
                        .child(
                            parts::segment("action-compat", look.tr("action-compat"), compat, look)
                                .on_click(cx.listener(|shell, _, _, cx| {
                                    shell.choose_the_quality(Some(VideoQuality::Compat), cx);
                                })),
                        ),
                )
                .into_any_element(),
        );
    }
    body
}

/// What a webhook's format is called.
fn format_id(format: WebhookFormat) -> &'static str {
    match format {
        WebhookFormat::Json => "webhook-format-json",
        WebhookFormat::Slack => "webhook-format-slack",
        WebhookFormat::Discord => "webhook-format-discord",
        WebhookFormat::Ntfy => "webhook-format-ntfy",
        WebhookFormat::Text => "webhook-format-text",
    }
}

/// A webhook's dialog. `watched` is whether it is `watch.toml`'s, the one
/// automation whose webhook is given the token and the signing key the
/// keyring keeps; **an automation made in the app never gets them**, and its
/// dialog says so.
fn webhook_body(
    dialog: &Dialog,
    watched: bool,
    look: &Look,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
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
        group(Some(look.tr("webhook-format")), look)
            .child(
                parts::segmented("webhook-format", look.tr("webhook-format"), look)
                    .flex_wrap()
                    .children(WebhookFormat::ALL.into_iter().map(|one| {
                        parts::segment(
                            ("webhook-format-one", one as usize),
                            look.tr(format_id(one)),
                            one == chosen,
                            look,
                        )
                        .on_click(
                            cx.listener(move |shell, _, _, cx| shell.choose_the_format(one, cx)),
                        )
                    })),
            )
            .into_any_element(),
        group(None, look)
            .child(
                parts::checkbox(
                    "webhook-heartbeat",
                    look.tr("webhook-heartbeat"),
                    beating,
                    look,
                )
                .on_click(cx.listener(|shell, _, _, cx| shell.toggle_the_heartbeat(cx))),
            )
            .child(small(
                "webhook-heartbeat-note",
                look.tr("webhook-heartbeat-note"),
                look,
            ))
            .into_any_element(),
    ];
    let rows = headers.iter().enumerate().map(|(at, row)| {
        let number = (at + 1).to_string();
        let numbered = |id: &'static str| look.tr_with(id, &[("number", &number)]);
        group(Some(numbered("webhook-header-row")), look)
            .id(("webhook-header", at))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(8.))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&row.name)
                                .id(("webhook-header-name", at))
                                .aria_label(numbered("webhook-header-name")),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&row.value)
                                .id(("webhook-header-value", at))
                                .aria_label(numbered("webhook-header-value")),
                        ),
                    )
                    .child(
                        parts::icon_button(
                            ("webhook-header-remove", at),
                            numbered("webhook-header-remove"),
                            assets::CLOSE,
                            true,
                            look,
                        )
                        .flex_none()
                        .on_click(
                            cx.listener(move |shell, _, _, cx| shell.remove_a_header(at, cx)),
                        ),
                    ),
            )
    });
    body.push(
        group(None, look)
            .child(
                div()
                    .id("webhook-headers-title")
                    .test_support()
                    .role(Role::Heading)
                    .aria_level(3)
                    .aria_label(look.tr("webhook-headers"))
                    .text_size(sp(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color(look.palette.text))
                    .child(look.tr("webhook-headers")),
            )
            .child(small(
                "webhook-headers-note",
                look.tr("webhook-headers-note"),
                look,
            ))
            .when(!watched, |headers| {
                let note = look.tr("webhook-keyring-note");
                headers.child(
                    div()
                        .id("webhook-keyring-note")
                        .test_support()
                        .role(Role::Note)
                        .aria_label(note.clone())
                        .text_size(sp(12.5))
                        .line_height(sp(18.))
                        .text_color(color(look.palette.secondary))
                        .child(note),
                )
            })
            .children(rows)
            .child(
                div().pt(sp(2.)).child(
                    parts::small_icon_button(
                        "webhook-header-add",
                        look.tr("webhook-header-add"),
                        assets::ADD,
                        look,
                    )
                    .on_click(cx.listener(|shell, _, window, cx| shell.add_a_header(window, cx))),
                ),
            )
            .into_any_element(),
    );
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Settings;

    #[test]
    fn every_choice_of_the_schedule_dialog_is_named_in_both_languages() {
        for locale in ["en", "es"] {
            let look = Look::of(
                Settings {
                    locale: locale.to_string(),
                    ..Settings::default()
                },
                false,
            );
            let ids = Frequency::ALL
                .into_iter()
                .map(frequency_id)
                .chain(Unit::ALL.into_iter().map(unit_id))
                .chain(Nth::ALL.into_iter().map(nth_id));
            for id in ids {
                assert_ne!(look.tr(id), id, "{locale}");
            }
        }
    }
}
