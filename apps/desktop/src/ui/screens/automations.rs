//! The automations section, in three stages (`state::automations::stage`):
//! the introduction, once an account, its title **centered on the window**,
//! under the brand, over a grid that drifts; the list, its title at the top
//! left, a search in the middle and "Create" at the right; and the editor,
//! a canvas with no edge that is moved and zoomed, under a floating panel
//! of the steps an automation can hold.
//!
//! **The grid drifts only while the window is looked at**, and never under
//! "reduce motion": it is decoration, and a frame drawn for it where nobody
//! looks would keep the app from resting (`state::rest`).
//!
//! **The steps are what is to come, and do nothing**: nothing is put on the
//! canvas and nothing runs.

use std::time::{Duration, Instant};

use gpui_kit::base::{Button, TestSupportExt as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, ContentMask, Context, Corners, CursorStyle, Div, FontWeight, KeyDownEvent,
    KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Role, ScrollEvent, Transformation,
    Window, canvas, div, fill, point, px, radians, relative, size,
};

use crate::state::automations::{self as state, ARROW_PANS, Canvas, Stage};
use crate::theme::motion;
use crate::theme::tokens::{Scheme, shape};
use crate::ui::assets;
use crate::ui::automations::{DRIFT_FRAME, Page, Steps};
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts::{self, Kind, icon};
use crate::ui::shell::{Phase, Shell};

/// The space between the editor's floating parts and the panel's edges.
const INSET: f32 = 16.0;

/// How long after one step of an unfolded kind the next comes in.
const STEP_STAGGER: Duration = Duration::from_millis(40);

/// The steps of each kind, as the panel lists them: the message of its
/// name, of what it does, and its drawing.
fn steps_of(steps: Steps) -> &'static [(&'static str, &'static str, &'static str)] {
    match steps {
        Steps::Triggers => &[
            (
                "trigger-schedule",
                "trigger-schedule-note",
                assets::SCHEDULE,
            ),
            (
                "trigger-followers",
                "trigger-followers-note",
                assets::FOLLOWERS,
            ),
            ("trigger-webhook", "trigger-webhook-note", assets::WEBHOOK),
        ],
        Steps::Conditions => &[
            (
                "condition-filter",
                "condition-filter-note",
                assets::CONDITION,
            ),
            ("condition-branch", "condition-branch-note", assets::BRANCH),
        ],
        Steps::Actions => &[
            ("action-webhook", "action-webhook-note", assets::WEBHOOK),
            ("action-api", "action-api-note", assets::API),
            ("action-notify", "action-notify-note", assets::NOTIFY),
        ],
    }
}

/// A kind of step's heading, its id and its words.
fn steps_heading(steps: Steps, look: &Look) -> (&'static str, String) {
    match steps {
        Steps::Triggers => ("editor-triggers", look.tr("editor-triggers")),
        Steps::Conditions => ("editor-conditions", look.tr("editor-conditions")),
        Steps::Actions => ("editor-actions", look.tr("editor-actions")),
    }
}

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
    let stage = state::stage(owed, page.editing);
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
        Stage::Editor => editor(shell, phase, &look, window, cx),
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
/// automations, none until there is a backend for them.
fn list(shell: &mut Shell, phase: Phase, look: &Look, cx: &mut Context<Shell>) -> AnyElement {
    let palette = look.palette;
    let Some(page) = &shell.automations else {
        return div().into_any_element();
    };
    let search = page.search.clone();
    let typed = search.read(cx).value().to_string();
    let found: Vec<String> = state::shown(&page.automations, &typed)
        .into_iter()
        .map(|automation| automation.name.clone())
        .collect();
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

    let body = if found.is_empty() {
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
            .children(found.into_iter().enumerate().map(|(at, name)| {
                div()
                    .id(("automation", at))
                    .test_support()
                    .role(Role::ListItem)
                    .aria_label(name.clone())
                    .p(sp(14.))
                    .rounded(px(shape::CARD_RADIUS))
                    .border_1()
                    .border_color(color(palette.line))
                    .font_weight(FontWeight::MEDIUM)
                    .child(name)
            }))
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
fn tool_button(
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
fn floating(look: &Look) -> Div {
    div()
        .occlude()
        .rounded(px(shape::CARD_RADIUS))
        .border_1()
        .border_color(color(look.palette.line))
        .bg(color(look.palette.panel))
        .shadow_md()
}

/// The editor: the canvas, and over it the way back with the title, the
/// zoom, and the steps.
fn editor(
    shell: &mut Shell,
    phase: Phase,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let palette = look.palette;
    let reduce = look.settings.reduce_motion;
    let now = cx.background_executor().now();
    let Some(page) = &shell.automations else {
        return div().into_any_element();
    };
    let editor = &page.editor;
    let canvas_now = editor.canvas_now(window, cx);
    let bounds = editor.bounds.clone();
    let cursor = if editor.dragging.is_some() {
        CursorStyle::ClosedHand
    } else if editor.space || editor.ctrl {
        CursorStyle::OpenHand
    } else {
        CursorStyle::Arrow
    };
    // How far the floating parts still have to come.
    let away = 1.0 - phase.presence;

    let dots = {
        let ink = color_at(
            palette.control_line,
            if look.settings.more_contrast {
                0.8
            } else {
                0.5
            },
        );
        let zoom = canvas_now.zoom();
        canvas(
            move |_, _, _| {},
            move |at, (), window, _| {
                bounds.set(at);
                let grid = canvas_now.grid((f32::from(at.size.width), f32::from(at.size.height)));
                let dot = (1.2 + 0.6 * zoom.min(1.5)).round().max(1.0);
                window.with_content_mask(Some(ContentMask { bounds: at }), |window| {
                    let mut y = grid.first.1;
                    while y < f32::from(at.size.height) + grid.gap {
                        let mut x = grid.first.0;
                        while x < f32::from(at.size.width) + grid.gap {
                            let corner = point(
                                at.origin.x + px(x - dot / 2.0),
                                at.origin.y + px(y - dot / 2.0),
                            );
                            let mut quad = fill(Bounds::new(corner, size(px(dot), px(dot))), ink);
                            quad.corner_radii = Corners::all(px(dot / 2.0));
                            window.paint_quad(quad);
                            x += grid.gap;
                        }
                        y += grid.gap;
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    };

    // Where an automation starts, at the canvas's origin: moved and zoomed
    // with it. Not a control: nothing is put there yet.
    let zoom = canvas_now.zoom();
    let (pan_x, pan_y) = canvas_now.pan();
    let start = div()
        .absolute()
        .left(px(pan_x))
        .top(px(pan_y))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .id("editor-start")
                .test_support()
                .role(Role::Note)
                .aria_label(look.tr("editor-start"))
                .w(sp(260. * zoom))
                .p(sp(18. * zoom))
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(8. * zoom))
                .rounded(px(shape::CARD_RADIUS * 1.4 * zoom))
                .border_2()
                .border_dashed()
                .border_color(color(palette.control_line))
                .bg(color(palette.panel))
                .child(
                    div()
                        .size(sp(36. * zoom))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(color_at(palette.accent, 0.14))
                        .child(icon(assets::ADD, 18. * zoom).text_color(color(palette.text))),
                )
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(sp(14. * zoom))
                        .line_height(sp(20. * zoom))
                        .text_color(color(palette.text))
                        .text_center()
                        .child(look.tr("editor-start")),
                )
                .child(
                    div()
                        .text_size(sp(12. * zoom))
                        .line_height(sp(17. * zoom))
                        .text_color(color(palette.secondary))
                        .text_center()
                        .child(look.tr("editor-hint")),
                ),
        );

    let surface = div()
        .id("editor-canvas")
        .test_support()
        .role(Role::Group)
        .aria_label(look.tr("editor-canvas"))
        .track_focus(&editor.focus)
        .absolute()
        .inset_0()
        .overflow_hidden()
        .cursor(cursor)
        .opacity(phase.presence)
        .focus_visible(move |style| style.border_2().border_color(color(palette.accent)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|shell, event: &MouseDownEvent, window, cx| {
                shell.press_on_the_canvas(event.position, event.modifiers.control, window, cx);
            }),
        )
        .on_mouse_down(
            MouseButton::Middle,
            cx.listener(|shell, event: &MouseDownEvent, window, cx| {
                shell.press_on_the_canvas(event.position, true, window, cx);
            }),
        )
        .on_mouse_move(cx.listener(|shell, event: &MouseMoveEvent, _, cx| {
            shell.point_on_the_canvas(event.position, event.modifiers.control, cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|shell, _, _, cx| shell.let_go_of_the_canvas(cx)),
        )
        .on_mouse_up(
            MouseButton::Middle,
            cx.listener(|shell, _, _, cx| shell.let_go_of_the_canvas(cx)),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|shell, _, _, cx| shell.let_go_of_the_canvas(cx)),
        )
        .on_mouse_up_out(
            MouseButton::Middle,
            cx.listener(|shell, _, _, cx| shell.let_go_of_the_canvas(cx)),
        )
        .on_scroll(cx.listener(|shell, event: &ScrollEvent, _, cx| {
            let by = f32::from(event.delta.pixel_delta(px(20.)).y);
            shell.wheel_the_canvas(by, event.position, cx);
            cx.stop_propagation();
        }))
        .on_key_down(cx.listener(|shell, event: &KeyDownEvent, _, cx| {
            let keys = &event.keystroke;
            if keys.modifiers.control || keys.modifiers.alt || keys.modifiers.platform {
                return;
            }
            match keys.key.as_str() {
                "space" => shell.hold_space_on_the_canvas(true, cx),
                "left" => shell.pan_the_canvas((ARROW_PANS, 0.0), true, cx),
                "right" => shell.pan_the_canvas((-ARROW_PANS, 0.0), true, cx),
                "up" => shell.pan_the_canvas((0.0, ARROW_PANS), true, cx),
                "down" => shell.pan_the_canvas((0.0, -ARROW_PANS), true, cx),
                "+" | "=" => shell.zoom_the_canvas(Canvas::zoomed_in, cx),
                "-" => shell.zoom_the_canvas(Canvas::zoomed_out, cx),
                "0" => shell.zoom_the_canvas(Canvas::at_its_size, cx),
                _ => return,
            }
            cx.stop_propagation();
        }))
        .on_key_up(cx.listener(|shell, event: &KeyUpEvent, _, cx| {
            if event.keystroke.key == "space" {
                shell.hold_space_on_the_canvas(false, cx);
            }
        }))
        .child(dots)
        .child(start);

    // The way back and the title, at the top left.
    let top = floating(look)
        .absolute()
        .top(px(INSET - 12.0 * away))
        .left(px(INSET))
        .opacity(phase.presence)
        .flex()
        .items_center()
        .gap(sp(8.))
        .p(px(4.))
        .pr(sp(14.))
        .child(
            tool_button(
                "editor-back",
                look.tr("editor-back"),
                assets::BACK,
                true,
                look,
            )
            .on_click(cx.listener(|shell, _, window, cx| shell.leave_the_editor(window, cx))),
        )
        .child(
            parts::title(&shell.title, look.tr("editor-title"), look)
                .text_size(sp(16.))
                .line_height(sp(22.))
                .text_left(),
        );

    // The zoom, at the bottom left.
    let percent = canvas_now.percent().to_string();
    let zoom_bar = floating(look)
        .id("editor-zoom")
        .test_support()
        .role(Role::Toolbar)
        .aria_label(look.tr("editor-zoom"))
        .absolute()
        .bottom(px(INSET - 12.0 * away))
        .left(px(INSET))
        .opacity(phase.presence)
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(4.))
        .child(
            tool_button(
                "editor-zoom-out",
                look.tr("editor-zoom-out"),
                assets::MINUS,
                canvas_now.can_zoom_out(),
                look,
            )
            .on_click(cx.listener(|shell, _, _, cx| {
                shell.zoom_the_canvas(Canvas::zoomed_out, cx);
            })),
        )
        .child(
            Button::new("editor-zoom-reset")
                .accessibility_label(look.tr_with("editor-zoom-reset", &[("percent", &percent)]))
                .h(sp(32.))
                .min_w(sp(56.))
                .px(sp(6.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(shape::CONTROL_RADIUS))
                .focus_visible(parts::focus_ring(&palette))
                .cursor_pointer()
                .hover(move |style| style.bg(color_at(palette.text, 0.07)))
                .text_size(sp(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(palette.text))
                .child(look.tr_with("editor-zoom-level", &[("percent", &percent)]))
                .on_click(cx.listener(|shell, _, _, cx| {
                    shell.zoom_the_canvas(Canvas::at_its_size, cx);
                })),
        )
        .child(
            tool_button(
                "editor-zoom-in",
                look.tr("editor-zoom-in"),
                assets::ADD,
                canvas_now.can_zoom_in(),
                look,
            )
            .on_click(cx.listener(|shell, _, _, cx| {
                shell.zoom_the_canvas(Canvas::zoomed_in, cx);
            })),
        )
        .child(
            div()
                .w(px(1.))
                .h(sp(18.))
                .mx(px(4.))
                .bg(color(palette.line)),
        )
        .child(
            tool_button("editor-fit", look.tr("editor-fit"), assets::FIT, true, look).on_click(
                cx.listener(|shell, _, _, cx| {
                    shell.zoom_the_canvas(|_| Canvas::default(), cx);
                }),
            ),
        );

    // The steps, floating at the right: the triggers unfolded, since an
    // automation starts with one.
    let sections: Vec<AnyElement> = Steps::ALL
        .into_iter()
        .map(|steps| {
            let open = editor.is_open(steps);
            let toggled = editor
                .toggled(steps)
                .map(|at| now.saturating_duration_since(at));
            steps_section(steps, open, toggled, reduce, look, window, cx)
        })
        .collect();
    let panel = div()
        .absolute()
        .top(px(INSET))
        .bottom(px(INSET))
        .right(px(INSET - 16.0 * away))
        .w(sp(288.))
        .max_w(relative(0.5))
        .opacity(phase.presence)
        .flex()
        .flex_col()
        .child(
            floating(look)
                .id("editor-steps")
                .test_support()
                .role(Role::Complementary)
                .aria_label(look.tr("editor-steps"))
                .flex_initial()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .p(sp(8.))
                .gap(sp(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(sp(8.))
                        .px(sp(8.))
                        .pt(sp(6.))
                        .pb(sp(8.))
                        .child(
                            div()
                                .id("editor-steps-title")
                                .test_support()
                                .role(Role::Heading)
                                .aria_level(2)
                                .aria_label(look.tr("editor-steps-title"))
                                .font_weight(FontWeight::BOLD)
                                .text_size(sp(14.))
                                .text_color(color(palette.text))
                                .child(look.tr("editor-steps-title")),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px(sp(8.))
                                .py(px(2.))
                                .rounded_full()
                                .border_1()
                                .border_color(color(palette.control_line))
                                .text_size(sp(11.5))
                                .text_color(color(palette.secondary))
                                .child(parts::said("editor-soon", look.tr("editor-soon"))),
                        ),
                )
                .children(sections),
        );

    div()
        .id("automations-editor")
        .size_full()
        .relative()
        .child(surface)
        .child(top)
        .child(zoom_bar)
        .child(panel)
        .into_any_element()
}

/// One kind of step in the panel: its heading, which folds and unfolds it,
/// and, unfolded, each step, coming in one after the other.
fn steps_section(
    steps: Steps,
    open: bool,
    toggled: Option<Duration>,
    reduce: bool,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let palette = look.palette;
    let (id, name) = steps_heading(steps, look);
    // The chevron turns from pointing right, folded, to down, unfolded.
    let turned = match toggled {
        Some(elapsed) if !reduce => {
            let through =
                motion::ease_out(elapsed.as_secs_f32() / motion::INTERACTION.as_secs_f32());
            if through < 1.0 {
                window.request_animation_frame();
            }
            if open { through } else { 1.0 - through }
        }
        _ => f32::from(u8::from(open)),
    };
    let heading = Button::new(id)
        .accessibility_label(name.clone())
        .aria_expanded(open)
        .w_full()
        .h(sp(34.))
        .px(sp(8.))
        .flex()
        .items_center()
        .justify_start()
        .gap(sp(8.))
        .rounded(px(shape::CONTROL_RADIUS))
        .focus_visible(parts::focus_ring(&palette))
        .cursor_pointer()
        .hover(move |style| style.bg(color_at(palette.text, 0.05)))
        .text_size(sp(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color(palette.text))
        .child(
            icon(assets::CHEVRON, 14.)
                .text_color(color(palette.secondary))
                .with_transformation(Transformation::rotate(radians(
                    -std::f32::consts::FRAC_PI_2 * (1.0 - turned),
                ))),
        )
        .child(name)
        .on_click(cx.listener(move |shell, _, _, cx| shell.toggle_the_steps(steps, cx)));

    let items = open.then(|| {
        div()
            .id((id, 1usize))
            .test_support()
            .role(Role::List)
            .aria_label(steps_heading(steps, look).1)
            .flex()
            .flex_col()
            .gap(sp(2.))
            .pb(sp(6.))
            .children(
                steps_of(steps)
                    .iter()
                    .enumerate()
                    .map(|(at, (named, note, drawing))| {
                        let presence = match toggled {
                            Some(elapsed) => {
                                let moved = look.moved(motion::ENTER, 6.0, 1.0);
                                let starts = if reduce {
                                    Duration::ZERO
                                } else {
                                    STEP_STAGGER * at as u32
                                };
                                let through = elapsed.saturating_sub(starts).as_secs_f32()
                                    / moved.lasts.as_secs_f32();
                                if through < 1.0 {
                                    window.request_animation_frame();
                                }
                                motion::ease_out(through)
                            }
                            None => 1.0,
                        };
                        let travels = if reduce { 0.0 } else { 6.0 };
                        let name = look.tr(named);
                        div()
                            .id((*named, at))
                            .test_support()
                            .role(Role::ListItem)
                            .aria_label(name.clone())
                            .relative()
                            .top(px(-travels * (1.0 - presence)))
                            .opacity(presence)
                            .flex()
                            .items_start()
                            .gap(sp(10.))
                            .px(sp(8.))
                            .py(sp(8.))
                            .rounded(px(shape::CONTROL_RADIUS))
                            .child(
                                div()
                                    .flex_none()
                                    .size(sp(30.))
                                    .rounded(px(shape::CONTROL_RADIUS))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(color_at(
                                        if steps == Steps::Triggers {
                                            palette.accent
                                        } else {
                                            palette.text
                                        },
                                        if steps == Steps::Triggers { 0.14 } else { 0.06 },
                                    ))
                                    .child(icon(drawing, 16.).text_color(color(palette.text))),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(sp(13.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(color(palette.text))
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .text_size(sp(12.))
                                            .line_height(sp(16.))
                                            .text_color(color(palette.secondary))
                                            .child(parts::said((*note, at), look.tr(note))),
                                    ),
                            )
                    }),
            )
    });

    div()
        .flex()
        .flex_col()
        .child(heading)
        .children(items)
        .into_any_element()
}
