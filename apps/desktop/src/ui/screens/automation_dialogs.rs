//! What opens over the editor: the small menu of a step, the configuration
//! of a step in a dialog, and the question asked when what is left cannot
//! run. **Each keeps focus inside it, closes on Escape, and hands focus back
//! to the canvas** (`Shell::dismiss`).
//!
//! **A dialog holds its fields until "Save"**: nothing of what is typed is
//! kept before, and "Save" is there only while the engine's own rules find
//! nothing wrong with what was typed.

use gpui_kit::base::{Button, FocusTrapElement as _, POPUP_PRIORITY, TestSupportExt as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, AnyElement, Context, Entity, FocusHandle, FontWeight, MouseButton, Role, anchored,
    deferred, div, px,
};
use snob_core::automation::{Only, Step, VideoQuality, WebhookFormat, can_join};
use snob_core::watch::schedule::Nth;

use super::words::{
    MONDAY_FIRST, and_list, month_short_words, month_words, problem_words, step_detail, step_name,
    weekday_short_words, weekday_words, wrong_words,
};
use crate::state::schedule::{
    By, DayMode, EVERY_MONTHS, Field, Frequency, Refused, ScheduleForm, Unit, toggle,
};
use crate::state::steps::problems_of;
use crate::theme::tokens::{frame, shape};
use crate::ui::assets;
use crate::ui::automations::Item;
use crate::ui::automations_editor::{Dialog, Fields, ScheduleFields};
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
        .child(parts::room_for_focus(
            div()
                .id((id, 2usize))
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(sp(12.))
                .children(body),
        ))
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
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(label_text(label.clone(), look))
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
    div()
        .flex()
        .flex_col()
        .gap(sp(6.))
        .child(label_text(look.tr("schedule-times"), look))
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
    body.push(
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
            )
            .into_any_element(),
    );
    match by {
        By::Date => {
            body.push(
                parts::chips("schedule-dates", look.tr("schedule-dates"), look)
                    .children((1..=31u32).map(|date| {
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
                    }))
                    .into_any_element(),
            );
            if form.some_months_lack_a_date() {
                body.push(small(
                    "schedule-short-months",
                    look.tr("schedule-short-months"),
                    look,
                ));
            }
        }
        By::Weekday => {
            body.push(
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
                    }))
                    .into_any_element(),
            );
            body.push(
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
                }))
                .into_any_element(),
            );
        }
    }
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
    let mut body: Vec<AnyElement> = vec![
        parts::segmented("schedule-frequency", look.tr("schedule-frequency"), look)
            .flex_wrap()
            .children(Frequency::ALL.into_iter().map(|one| {
                let id = frequency_id(one);
                parts::segment(id, look.tr(id), one == frequency, look).on_click(cx.listener(
                    move |shell, _, window, cx| shell.choose_the_frequency(one, window, cx),
                ))
            }))
            .into_any_element(),
    ];
    let floor = |body: &mut Vec<AnyElement>| {
        if form.counts_from_the_last_run() {
            body.push(small("schedule-floor", look.tr("schedule-floor"), look));
        }
    };
    match frequency {
        Frequency::Day => {
            let mode = choices.day_mode;
            body.push(
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
            );
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
                    body.push(
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
                    );
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
            body.push(
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
            );
            wrong_under(Field::Weekdays, refused, &mut body, look);
            body.push(times_list(fields, look, cx));
            wrong_under(Field::Times, refused, &mut body, look);
        }
        Frequency::Month => {
            let every = choices.every_months;
            body.push(
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
            );
            if every > 1 {
                let options: Vec<String> = (1..=12)
                    .map(|month| capitalized(&month_words(month, look)))
                    .collect();
                let chosen = choices.first_month.clamp(1, 12) as usize - 1;
                body.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(label_text(look.tr("schedule-first-month"), look))
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
            body.push(
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
            );
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
            parts::checkbox(
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
        parts::checkbox(
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
