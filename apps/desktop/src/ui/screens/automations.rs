//! The automations section, in three stages (`state::automations::stage`):
//! the introduction, once an account, its title **centered on the window**,
//! under the brand, over a grid that drifts; the list, its title at the top
//! left, a search in the middle and "Create" at the right; and the editor,
//! a canvas with no edge that is moved and zoomed, under a floating panel
//! of the steps an automation can hold (`automation_editor`); and an
//! automation's own page between the list and the editor
//! (`automation_page`).
//!
//! **The grid drifts only while the window is looked at**, and never under
//! "reduce motion": it is decoration, and a frame drawn for it where nobody
//! looks would keep the app from resting (`state::rest`).

use std::time::{Duration, Instant};

use gpui_kit::base::{Button, TestSupportExt as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, ContentMask, Context, Corners, Div, FontWeight, Role, Window, canvas, div,
    fill, point, px, size,
};

use crate::state::automations::{self as state, Stage};
use crate::theme::motion;
use crate::theme::tokens::{Scheme, shape};
use crate::ui::assets;
use crate::ui::automations::{DRIFT_FRAME, Page};
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts::{self, Kind, icon};
use crate::ui::shell::{Phase, Shell};

/// How far a stage has come in `since` it was first drawn, asking for the
/// next frame while it moves: as a screen comes in, inside the screen's
/// own coming. The first stage drawn does not come in twice.
fn stage_phase(page: &Page, now: Instant, look: &Look, window: &mut Window) -> Phase {
    if page.left.is_none() {
        return Phase::THERE;
    }
    let moved = look.moved(motion::ENTER, 14.0, 1.0);
    let at = now.saturating_duration_since(page.since).as_secs_f32() / moved.lasts.as_secs_f32();
    if at < 1.0 {
        window.request_animation_frame();
    }
    let eased = motion::ease_out(at.min(1.0));
    Phase {
        presence: eased,
        rise: moved.travels * (1.0 - eased),
        scale: 1.0,
    }
}

/// The screen's phase and the stage's together.
fn within(screen: Phase, stage: Phase) -> Phase {
    Phase {
        presence: screen.presence * stage.presence,
        rise: screen.rise + stage.rise,
        scale: screen.scale,
    }
}

pub fn render(
    shell: &mut Shell,
    phase: Phase,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    let owed = shell
        .introduced
        .owed_to(shell.active().map(|account| account.named.pk));
    let off_center = shell.panel_off_center(cx);
    let resting = shell.is_resting();
    let now = cx.background_executor().now();
    let Some(page) = shell.automations.as_mut() else {
        return div().into_any_element();
    };
    let stage = state::stage(owed, page.open.is_some(), page.edit.is_some());
    if page.shown != Some(stage) {
        page.left = page.shown;
        page.shown = Some(stage);
        page.since = now;
    }
    let came = stage_phase(page, now, &look, window);

    // The grid drifts behind the introduction, and fades behind the list
    // that takes its place.
    let fading = page.left == Some(Stage::Intro) && came.presence < 1.0;
    let grid = (stage == Stage::Intro || fading).then(|| {
        let running = stage == Stage::Intro
            && !look.settings.reduce_motion
            && window.is_window_active()
            && !resting;
        let ran = page
            .drift
            .at(now.saturating_duration_since(page.born), running);
        if running && !page.ticking {
            page.ticking = true;
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(DRIFT_FRAME).await;
                this.update(cx, |shell, cx| {
                    if let Some(page) = &mut shell.automations {
                        page.ticking = false;
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        let shown = if stage == Stage::Intro {
            phase.presence
        } else {
            phase.presence * (1.0 - came.presence)
        };
        drifting_grid(off_center, ran, shown, &look)
    });

    let phase = within(phase, came);
    let body = match stage {
        Stage::Intro => intro(shell, phase, off_center, &look, cx),
        Stage::List => list(shell, phase, &look, cx),
        Stage::Page => super::automation_page::page(shell, phase, &look, cx),
        Stage::Editor => super::automation_editor::editor(shell, phase, &look, window, cx),
    };
    div()
        .id("automations")
        .size_full()
        .relative()
        .overflow_hidden()
        .children(grid)
        .child(body)
        .into_any_element()
}

/// The grid behind the introduction, drawn `ran` into its drift with
/// `shown` of its ink, its words' middle `off_center` left of the panel's.
fn drifting_grid(off_center: f32, ran: Duration, shown: f32, look: &Look) -> AnyElement {
    let palette = look.palette;
    // Faint enough to read words over, in each look.
    let ink = match (look.scheme, look.settings.more_contrast) {
        (_, true) => 0.22,
        (Scheme::Light, false) => 0.11,
        (Scheme::Dark, false) => 0.17,
    } * shown;
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let size_of = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let middle = (size_of.0 / 2.0 - off_center, size_of.1 / 2.0);
            let (strokes, sparks) = state::drifting_grid(size_of, middle, ran);
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for stroke in strokes {
                    let at = point(
                        bounds.origin.x + px(stroke.x),
                        bounds.origin.y + px(stroke.y),
                    );
                    window.paint_quad(fill(
                        Bounds::new(at, size(px(stroke.width), px(stroke.height))),
                        color_at(palette.text, ink * stroke.ink),
                    ));
                }
                for spark in sparks {
                    let at = point(
                        bounds.origin.x + px(spark.x - 1.5),
                        bounds.origin.y + px(spark.y - 1.5),
                    );
                    let mut dot = fill(
                        Bounds::new(at, size(px(3.), px(3.))),
                        color_at(palette.accent, 0.9 * shown * spark.ink),
                    );
                    dot.corner_radii = Corners::all(px(1.5));
                    window.paint_quad(dot);
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// The introduction: what automations are, and "Get started".
fn intro(
    shell: &mut Shell,
    phase: Phase,
    off_center: f32,
    look: &Look,
    cx: &mut Context<Shell>,
) -> AnyElement {
    // Padded on the right by what the rail takes on the left, so that the
    // middle of what is left is the window's.
    div()
        .absolute()
        .inset_0()
        .pr(px(2.0 * off_center))
        .flex()
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(sp(18.))
                .px(sp(32.))
                .py(sp(40.))
                .relative()
                .top(px(phase.rise))
                .opacity(phase.presence)
                .child(parts::title(
                    &shell.title,
                    look.tr("automations-title"),
                    look,
                ))
                .child(
                    parts::paragraph("automations-intro", look.tr("automations-intro"), look)
                        .max_w(sp(520.)),
                )
                .child(
                    div().pt(sp(6.)).child(
                        parts::button(
                            "automations-begin",
                            look.tr("automations-begin"),
                            Kind::Primary,
                            true,
                            look,
                        )
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.begin_the_automations(window, cx);
                        })),
                    ),
                ),
        )
        .into_any_element()
}

/// The list: the title, a search in the middle, "Create", and the
/// automations, each a way to its page.
fn list(shell: &mut Shell, phase: Phase, look: &Look, cx: &mut Context<Shell>) -> AnyElement {
    let palette = look.palette;
    let Some(page) = &shell.automations else {
        return div().into_any_element();
    };
    let search = page.search.clone();
    let typed = search.read(cx).value().to_string();
    let found: Vec<(String, String, String)> = state::shown(&page.automations, &typed)
        .into_iter()
        .map(|automation| {
            let steps = look.tr_with(
                "automation-steps",
                &[("count", &automation.nodes.len().to_string())],
            );
            (
                automation.id.clone(),
                super::words::automation_name(automation, look),
                steps,
            )
        })
        .collect();
    let loaded = page.loaded;
    let none_at_all = page.automations.is_empty();

    let create_label = look.tr("automations-create");
    let create = parts::plain_button(
        "automations-create",
        create_label.clone(),
        Kind::Primary,
        true,
        look,
    )
    .px(sp(16.))
    .child(icon(assets::ADD, 16.).text_color(color(palette.on_accent)))
    .child(create_label)
    .on_click(cx.listener(|shell, _, window, cx| shell.create_an_automation(window, cx)));

    // The search in the middle while the title and "Create" leave room for
    // it; under them when they do not.
    let header = div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(16.))
        .px(sp(32.))
        .pt(sp(26.))
        .pb(sp(16.))
        .child(div().flex_1().flex().child(parts::title(
            &shell.title,
            look.tr("automations-title"),
            look,
        )))
        .child(
            div().w(sp(300.)).flex_shrink_1().min_w(sp(140.)).child(
                Input::new(&search)
                    .id("automations-search")
                    .aria_label(look.tr("automations-search"))
                    .prefix(icon(assets::SEARCH, 15.).text_color(color(palette.secondary))),
            ),
        )
        .child(div().flex_1().flex().justify_end().child(create));

    let body = if !loaded {
        div().flex_1().into_any_element()
    } else if found.is_empty() {
        let (id, heading, note) = if none_at_all && typed.trim().is_empty() {
            (
                "automations-empty",
                look.tr("automations-empty"),
                Some(look.tr("automations-empty-note")),
            )
        } else {
            let said = look.tr_with("automations-none", &[("typed", typed.trim())]);
            ("automations-none", said, None)
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(sp(10.))
            .px(sp(32.))
            .pb(sp(56.))
            .child(
                div()
                    .size(sp(52.))
                    .mb(sp(6.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(color_at(palette.accent, 0.12))
                    .child(icon(assets::AUTOMATIONS, 22.).text_color(color(palette.text))),
            )
            .child(
                div()
                    .id(id)
                    .test_support()
                    .role(if note.is_some() {
                        Role::Heading
                    } else {
                        Role::Status
                    })
                    .when(note.is_some(), |heading| heading.aria_level(2))
                    .aria_label(heading.clone())
                    .font_weight(FontWeight::BOLD)
                    .text_size(sp(16.))
                    .text_color(color(palette.text))
                    .text_center()
                    .child(heading),
            )
            .children(note.map(|note| {
                parts::paragraph("automations-empty-note", note, look)
                    .max_w(sp(380.))
                    .text_size(sp(14.))
            }))
            .into_any_element()
    } else {
        div()
            .id("automations-list")
            .test_support()
            .role(Role::List)
            .aria_label(look.tr("automations-title"))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(sp(8.))
            .px(sp(32.))
            .children(
                found
                    .into_iter()
                    .enumerate()
                    .map(|(at, (id, name, steps))| {
                        let said = format!("{name}. {steps}");
                        parts::plain_button(("automation", at), said, Kind::Secondary, true, look)
                            .w_full()
                            .h_auto()
                            .py(sp(12.))
                            .justify_between()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(color(palette.text))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(sp(13.))
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(color(palette.secondary))
                                    .child(steps),
                            )
                            .on_click(cx.listener(move |shell, _, window, cx| {
                                shell.open_an_automation(id.clone(), window, cx);
                            }))
                    }),
            )
            .into_any_element()
    };

    div()
        .id("automations-page")
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .relative()
        .top(px(phase.rise))
        .opacity(phase.presence)
        .child(header)
        .child(body)
        .into_any_element()
}

/// A small square button of the editor's that shows a drawing, named for
/// whoever does not see it, with its name as a tip.
pub(super) fn tool_button(
    id: &'static str,
    label: String,
    drawing: &'static str,
    enabled: bool,
    look: &Look,
) -> Button {
    let palette = look.palette;
    let tip = label.clone();
    Button::new(id)
        .accessibility_label(label)
        .disabled(!enabled)
        .flex_none()
        .size(sp(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(shape::CONTROL_RADIUS))
        .focus_visible(parts::focus_ring(&palette))
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(move |style| style.bg(color_at(palette.text, 0.07)))
                .active(move |style| style.bg(color_at(palette.text, 0.12)))
        })
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(icon(drawing, 16.).text_color(color(if enabled {
            palette.text
        } else {
            palette.disabled_text
        })))
}

/// What floats over the canvas: a card in the panel's color, apart from it
/// by its outline and a shadow, that the pointer does not reach through.
pub(super) fn floating(look: &Look) -> Div {
    div()
        .occlude()
        .rounded(px(shape::CARD_RADIUS))
        .border_1()
        .border_color(color(look.palette.line))
        .bg(color(look.palette.panel))
        .shadow_md()
}
