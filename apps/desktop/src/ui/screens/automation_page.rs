//! One automation's page: its run history on the left, a summary of its
//! flow in words beside it, and the pencil that opens its editor.
//!
//! **The history is what the engine kept of the runs**
//! (`Backend::automation_runs`), the newest first, and nothing here sends
//! anything: "Run now" starts `snob watch once` as the command line does.

use gpui_kit::base::{ObservedElement, TestSupportExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Div, FontWeight, Role, Stateful, div, px};
use snob_core::automation::Automation;

use super::automations::tool_button;
use super::words::{automation_name, moment, outcome_words, stage_name, step_detail, step_name};
use crate::backend::{Next, Run};
use crate::theme::tokens::shape;
use crate::ui::assets;
use crate::ui::automations::Item;
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts;
use crate::ui::shell::{Phase, Shell};

pub fn page(shell: &mut Shell, phase: Phase, look: &Look, cx: &mut Context<Shell>) -> AnyElement {
    let palette = look.palette;
    let Some(page) = &shell.automations else {
        return div().into_any_element();
    };
    let Some(open) = &page.open else {
        return div().into_any_element();
    };
    let Some(automation) = page.automations.iter().find(|one| one.id == open.id) else {
        return div().into_any_element();
    };
    let name = automation_name(automation, look);
    let runs = open.runs.clone();
    let next = open.next;
    let started = open.started.clone();

    let run_now = parts::small_button("automation-run", look.tr("automation-run"), true, look)
        .on_click(cx.listener(|shell, _, window, cx| shell.run_the_automation(window, cx)));
    let pencil = tool_button(
        "automation-edit",
        look.tr("automation-edit"),
        assets::EDIT,
        true,
        look,
    )
    .on_click(cx.listener(|shell, _, window, cx| shell.edit_the_automation(window, cx)));
    let back = tool_button(
        "automation-back",
        look.tr("automation-back"),
        assets::BACK,
        true,
        look,
    )
    .on_click(cx.listener(|shell, _, window, cx| shell.close_the_automation(window, cx)));

    let header = div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(12.))
        .px(sp(32.))
        .pt(sp(26.))
        .pb(sp(16.))
        .child(back)
        .child(
            div()
                .flex_1()
                .min_w(sp(160.))
                .flex()
                .child(parts::title(&shell.title, name, look).text_left()),
        )
        .child(run_now)
        .child(pencil);

    let said = started.map(|started| {
        let (id, text) = match started {
            Ok(()) => ("automation-started", look.tr("automation-started")),
            Err(why) => (
                "automation-not-started",
                look.tr_with("automation-not-started", &[("why", &why)]),
            ),
        };
        div()
            .px(sp(32.))
            .pb(sp(8.))
            .text_size(sp(13.))
            .text_color(color(palette.secondary))
            .child(parts::said(id, text))
    });

    let history = history(runs, look);
    let flow = flow(automation, next, look);
    let body = div()
        .id("automation-body")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_wrap()
        .items_start()
        .gap(sp(24.))
        .px(sp(32.))
        .pb(sp(32.))
        .child(history)
        .child(flow);

    div()
        .id("automation-page")
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .relative()
        .top(px(phase.rise))
        .opacity(phase.presence)
        .child(header)
        .children(said)
        .child(body)
        .into_any_element()
}

/// A card that holds one half of the page.
fn card(id: &'static str, heading: String, look: &Look) -> ObservedElement<Stateful<Div>> {
    let palette = look.palette;
    div()
        .id(id)
        .test_support()
        .role(Role::Region)
        .aria_label(heading.clone())
        .flex_1()
        .min_w(sp(300.))
        .flex()
        .flex_col()
        .gap(sp(10.))
        .p(sp(18.))
        .rounded(px(shape::CARD_RADIUS))
        .border_1()
        .border_color(color(palette.line))
        .bg(color(palette.panel))
        .child(
            div()
                .id((id, 1usize))
                .test_support()
                .role(Role::Heading)
                .aria_level(2)
                .aria_label(heading.clone())
                .font_weight(FontWeight::BOLD)
                .text_size(sp(15.))
                .text_color(color(palette.text))
                .child(heading),
        )
}

/// A line of small print.
fn note(id: &'static str, text: String, look: &Look) -> Div {
    div()
        .text_size(sp(13.))
        .text_color(color(look.palette.secondary))
        .child(parts::said(id, text))
}

/// The runs, newest first.
fn history(runs: Option<Vec<Run>>, look: &Look) -> AnyElement {
    let palette = look.palette;
    let card = card("automation-history", look.tr("automation-history"), look);
    let Some(runs) = runs else {
        let reading = look.tr("automation-reading");
        return card
            .child(note("automation-reading", reading, look))
            .into_any_element();
    };
    if runs.is_empty() {
        let none = look.tr("automation-no-runs");
        return card
            .child(note("automation-no-runs", none, look))
            .into_any_element();
    }
    card.child(
        div()
            .id("automation-runs")
            .test_support()
            .role(Role::List)
            .aria_label(look.tr("automation-history"))
            .flex()
            .flex_col()
            .gap(sp(8.))
            .children(runs.into_iter().enumerate().map(|(at, run)| {
                let outcome = outcome_words(run.outcome.as_deref(), look);
                let when = moment(run.started_at);
                let count = run.requests.to_string();
                let requests = look.tr_with("run-requests", &[("count", &count)]);
                let said = format!("{when}. {outcome}. {requests}");
                let ink = if run.outcome.as_deref() == Some("ok") {
                    palette.text
                } else {
                    palette.secondary
                };
                div()
                    .id(("automation-run-row", at))
                    .test_support()
                    .role(Role::ListItem)
                    .aria_label(said)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(sp(10.))
                    .rounded(px(shape::CONTROL_RADIUS))
                    .bg(color_at(palette.text, 0.04))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .justify_between()
                            .gap(sp(8.))
                            .text_size(sp(13.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(color(palette.text))
                                    .child(when),
                            )
                            .child(div().text_color(color(ink)).child(outcome)),
                    )
                    .child(
                        div()
                            .text_size(sp(12.))
                            .text_color(color(palette.secondary))
                            .child(requests),
                    )
                    .children(run.summary.lines().map(|line| {
                        div()
                            .text_size(sp(12.))
                            .line_height(sp(17.))
                            .text_color(color(palette.secondary))
                            .child(line.to_string())
                    }))
            })),
    )
    .into_any_element()
}

/// One step of the flow, in words.
fn step_row(at: usize, node: &snob_core::automation::Node, look: &Look) -> AnyElement {
    let palette = look.palette;
    let name = step_name(&node.step, look);
    let detail = step_detail(&node.step, look);
    let stage = stage_name(node.step.stage(), look);
    let off = (!node.enabled).then(|| look.tr("node-disabled"));
    let said = [
        Some(stage.clone()),
        Some(name.clone()),
        detail.clone(),
        off.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(". ");
    let small = |text: String, size: f32| {
        div()
            .text_size(sp(size))
            .text_color(color(palette.secondary))
            .child(text)
    };
    div()
        .id(("automation-step", at))
        .test_support()
        .role(Role::ListItem)
        .aria_label(said)
        .flex()
        .items_start()
        .gap(sp(10.))
        .opacity(if node.enabled { 1.0 } else { 0.55 })
        .child(
            div()
                .flex_none()
                .size(sp(30.))
                .rounded(px(shape::CONTROL_RADIUS))
                .flex()
                .items_center()
                .justify_center()
                .bg(color_at(palette.accent, 0.12))
                .child(
                    parts::icon(Item::of(&node.step).drawing(), 16.)
                        .text_color(color(palette.text)),
                ),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(small(stage, 12.))
                .child(
                    div()
                        .text_size(sp(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(palette.text))
                        .child(name),
                )
                .children(detail.map(|detail| small(detail, 12.5)))
                .children(off.map(|off| small(off, 12.))),
        )
        .into_any_element()
}

/// The flow in words, a column of the run at a time, and when it is next
/// due.
fn flow(automation: &Automation, next: Option<Next>, look: &Look) -> AnyElement {
    let palette = look.palette;
    let rows: Vec<AnyElement> = automation
        .columns()
        .into_iter()
        .flatten()
        .filter_map(|id| automation.node(id))
        .enumerate()
        .map(|(at, node)| step_row(at, node, look))
        .collect();
    let card = card("automation-flow", look.tr("automation-flow"), look);
    let after = next.map(|next| match next {
        Next::Now => look.tr("automation-next-now"),
        Next::At(at) => look.tr_with("automation-next-at", &[("when", &moment(at))]),
        Next::Never => look.tr("automation-next-never"),
    });
    let card = if rows.is_empty() {
        let none = look.tr("automation-empty-flow");
        card.child(note("automation-empty-flow", none, look))
    } else {
        card.child(
            div()
                .id("automation-steps")
                .test_support()
                .role(Role::List)
                .aria_label(look.tr("automation-flow"))
                .flex()
                .flex_col()
                .gap(sp(12.))
                .children(rows),
        )
    };
    card.children(after.map(|after| {
        div()
            .id("automation-next")
            .test_support()
            .role(Role::Status)
            .aria_label(after.clone())
            .pt(sp(6.))
            .text_size(sp(13.))
            .text_color(color(palette.secondary))
            .child(after)
    }))
    .into_any_element()
}
