//! The editor: a canvas with no edge, moved and zoomed, that holds the
//! automation's steps and the arrows between them, under a floating panel of
//! the steps that can be added.
//!
//! **A step is a button**: Tab reaches it, Enter opens its menu, and the
//! menu holds everything the pointer does besides moving it (join it to
//! another, take an arrow off, configure, disable, remove). **A step that
//! the automation's problems are about is marked** with the error's outline
//! and its sentence in the step's name.

use std::time::Duration;

use gpui_kit::base::{Button, TestSupportExt as _};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, ClickEvent, ContentMask, Context, Corners, CursorStyle, FontWeight,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, Role, ScrollEvent, Transformation, Window, canvas, div, fill, point, px, radians,
    relative, size,
};
use snob_core::automation::Node;

use super::automations::{floating, tool_button};
use super::words::{stage_name, step_detail, step_name};
use crate::state::automations::{ARROW_PANS, Canvas, NODE_H, NODE_W, comes_in_at, leaves_at};
use crate::theme::motion;
use crate::theme::tokens::shape;
use crate::ui::assets;
use crate::ui::automations::{Edit, Item, Said, Steps};
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts;
use crate::ui::shell::{Phase, Shell};

use super::words::unjoined_words;

/// The space between the editor's floating parts and the panel's edges.
const INSET: f32 = 16.0;

/// How long after one step of an unfolded kind the next comes in.
const STEP_STAGGER: Duration = Duration::from_millis(40);

/// A kind of step's heading, its id and its words.
fn steps_heading(steps: Steps, look: &Look) -> (&'static str, String) {
    let id = match steps {
        Steps::Triggers => "editor-triggers",
        Steps::Actions => "editor-actions",
        Steps::Filters => "editor-filters",
        Steps::Outputs => "editor-outputs",
    };
    (id, look.tr(id))
}

/// Where the step `node` is drawn: where it is held while it is moved.
fn placed(edit: &Edit, node: &Node) -> (i32, i32) {
    match &edit.grab {
        Some(grab) if grab.id == node.id => grab.at,
        _ => node.at,
    }
}

/// The dots of the canvas, which also tell where it was last drawn.
fn dots(
    canvas_now: Canvas,
    look: &Look,
    bounds: std::rc::Rc<std::cell::Cell<Bounds<gpui_kit::Pixels>>>,
) -> AnyElement {
    let palette = look.palette;
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
            // Another size is seen by the next frame, which fits the
            // canvas again when it is to be.
            if bounds.get().size != at.size {
                window.request_animation_frame();
            }
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
    .into_any_element()
}

/// One arrow: from `a` to `b`, in the canvas's own units, and what ink.
struct Arrow {
    from: (f32, f32),
    to: (f32, f32),
    faint: bool,
    held: bool,
}

/// Every arrow, drawn as a curve that leaves a step sideways and comes
/// into the next one, with a head.
fn arrows(canvas_now: Canvas, all: Vec<Arrow>, look: &Look) -> AnyElement {
    let palette = look.palette;
    canvas(
        move |_, _, _| {},
        move |bounds, (), window, _| {
            let middle = bounds.center();
            let screen = |at: (f32, f32)| {
                let on = canvas_now.on_screen(at);
                point(middle.x + px(on.0), middle.y + px(on.1))
            };
            let zoom = canvas_now.zoom();
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for arrow in &all {
                    let ink = if arrow.held {
                        color(palette.accent)
                    } else if arrow.faint {
                        color_at(palette.secondary, 0.35)
                    } else {
                        color_at(palette.secondary, 0.9)
                    };
                    let reach = ((arrow.to.0 - arrow.from.0).abs() * 0.5).max(50.0);
                    let (a, b) = (screen(arrow.from), screen(arrow.to));
                    let ca = screen((arrow.from.0 + reach, arrow.from.1));
                    let cb = screen((arrow.to.0 - reach, arrow.to.1));
                    let mut line = PathBuilder::stroke(px((2.0 * zoom).max(1.0)));
                    line.move_to(a);
                    line.cubic_bezier_to(b, ca, cb);
                    if let Ok(line) = line.build() {
                        window.paint_path(line, ink);
                    }
                    let head = (9.0 * zoom).max(4.0);
                    let mut tip = PathBuilder::fill();
                    tip.move_to(b);
                    tip.line_to(point(b.x - px(head * 1.5), b.y - px(head * 0.8)));
                    tip.line_to(point(b.x - px(head * 1.5), b.y + px(head * 0.8)));
                    tip.close();
                    if let Ok(tip) = tip.build() {
                        window.paint_path(tip, ink);
                    }
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// What the arrows are, with the step held and the arrow being drawn
/// counted.
fn arrows_of(edit: &Edit) -> Vec<Arrow> {
    let automation = edit.state.automation();
    let at = |id| {
        automation
            .node(id)
            .map(|node| (placed(edit, node), node.enabled))
    };
    let mut all: Vec<Arrow> = automation
        .edges
        .iter()
        .filter_map(|edge| {
            let ((from, on), (to, off)) = (at(edge.from)?, at(edge.to)?);
            Some(Arrow {
                from: leaves_at(from),
                to: comes_in_at(to),
                faint: !(on && off),
                held: false,
            })
        })
        .collect();
    if let Some(wire) = &edit.wire
        && let Some((from, _)) = at(wire.from)
    {
        all.push(Arrow {
            from: leaves_at(from),
            to: wire.to,
            faint: false,
            held: true,
        });
    }
    all
}

/// One step on the canvas: a button that moves with the pointer, opens its
/// menu on a right press or Enter, and carries the handle an arrow is drawn
/// from.
fn node_view(
    node: &Node,
    at: (i32, i32),
    canvas_now: Canvas,
    flagged: bool,
    selected: bool,
    look: &Look,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let palette = look.palette;
    let zoom = canvas_now.zoom();
    let (left, top) = canvas_now.on_screen((at.0 as f32, at.1 as f32));
    let id = node.id;
    let item = Item::of(&node.step);
    let name = step_name(&node.step, look);
    let detail = step_detail(&node.step, look);
    let stage = stage_name(node.step.stage(), look);
    let said = [
        Some(stage),
        Some(name.clone()),
        detail.clone(),
        (!node.enabled).then(|| look.tr("node-disabled")),
        flagged.then(|| look.tr("node-has-problem")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(". ");
    let outline = if flagged {
        palette.error
    } else if selected {
        palette.accent
    } else {
        palette.control_line
    };
    let leads_on = node.step.stage() != snob_core::automation::Stage::Output;
    Button::new(("node", id as usize))
        .accessibility_label(said)
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(NODE_W * zoom))
        .h(px(NODE_H * zoom))
        .px(px(10. * zoom))
        .gap(px(10. * zoom))
        .flex()
        .items_center()
        .justify_start()
        .rounded(px(shape::CARD_RADIUS * zoom))
        .border(px(
            (if flagged || selected { 2.0 } else { 1.0 }) * zoom.max(0.5)
        ))
        .border_color(color(outline))
        .bg(color(palette.panel))
        .shadow_sm()
        .opacity(if node.enabled { 1.0 } else { 0.5 })
        .focus_visible(parts::focus_ring(&palette))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |shell, event: &MouseDownEvent, _, cx| {
                if !event.modifiers.control && !shell.space_is_held() {
                    shell.grab_a_step(id, event.position, cx);
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |shell, event: &MouseDownEvent, window, cx| {
                shell.open_the_node_menu(id, event.position, window, cx);
            }),
        )
        .on_click(cx.listener(move |shell, event: &ClickEvent, window, cx| {
            if event.is_keyboard() {
                shell.open_the_node_menu_below(id, window, cx);
            }
        }))
        .child(
            div()
                .flex_none()
                .size(px(30. * zoom))
                .rounded(px(shape::CONTROL_RADIUS * zoom))
                .flex()
                .items_center()
                .justify_center()
                .bg(color_at(palette.accent, 0.14))
                .child(parts::icon(item.drawing(), 16. * zoom).text_color(color(palette.text))),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .items_start()
                .child(
                    div()
                        .text_size(sp(13. * zoom))
                        .line_height(sp(17. * zoom))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(palette.text))
                        .overflow_hidden()
                        .child(name),
                )
                .children(detail.map(|detail| {
                    div()
                        .text_size(sp(11.5 * zoom))
                        .line_height(sp(15. * zoom))
                        .text_color(color(palette.secondary))
                        .overflow_hidden()
                        .child(detail)
                })),
        )
        .children(leads_on.then(|| {
            div()
                .absolute()
                .right(px(-12.))
                .top(relative(0.5))
                .mt(px(-12.))
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .cursor(CursorStyle::Crosshair)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |shell, event: &MouseDownEvent, _, cx| {
                        shell.begin_a_wire(id, event.position, cx);
                    }),
                )
                .child(
                    div()
                        .size(px(12.))
                        .rounded_full()
                        .border_2()
                        .border_color(color(palette.panel))
                        .bg(color(palette.accent)),
                )
        }))
        .into_any_element()
}

/// The refusal or the failure to keep, under the canvas.
fn said_line(said: &Said, look: &Look) -> AnyElement {
    let palette = look.palette;
    let text = match said {
        Said::Unjoined(why) => unjoined_words(*why, look),
        Said::NotKept(why) => look.tr_with("editor-not-kept", &[("why", why)]),
    };
    floating(look)
        .id("editor-said")
        .test_support()
        .role(Role::Status)
        .aria_label(text.clone())
        .absolute()
        .bottom(px(INSET + 52.))
        .left(relative(0.5))
        .px(sp(14.))
        .py(sp(8.))
        .text_size(sp(13.))
        .text_color(color(palette.text))
        .border_color(color(palette.error))
        .child(text)
        .into_any_element()
}

/// The start marker, where nothing is on the canvas yet.
fn start(canvas_now: Canvas, look: &Look) -> AnyElement {
    let palette = look.palette;
    let zoom = canvas_now.zoom();
    let (pan_x, pan_y) = canvas_now.pan();
    div()
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
                        .child(
                            parts::icon(assets::ADD, 18. * zoom).text_color(color(palette.text)),
                        ),
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
        )
        .into_any_element()
}

/// The editor: the canvas, and over it the way back with the name, the
/// undo, the zoom, and the steps.
pub fn editor(
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
    let Some(edit) = &page.edit else {
        return div().into_any_element();
    };
    let editor = &page.editor;
    let canvas_now = editor.canvas_now(window, cx);
    // A window of another size shows the canvas as it was fitted, which is
    // no longer to fit: it is fitted again, once this frame is drawn.
    let drawn = editor.bounds.get().size;
    if editor.fitted && drawn != editor.fitted_for.get() && drawn.width > px(0.) {
        editor.fitted_for.set(drawn);
        cx.spawn_in(window, async move |this, cx| {
            this.update(cx, |this, cx| this.fit_the_canvas_if_untouched(cx))
                .ok();
        })
        .detach();
    }
    let cursor = if editor.dragging.is_some() || edit.grab.is_some() {
        CursorStyle::ClosedHand
    } else if editor.space || editor.ctrl {
        CursorStyle::OpenHand
    } else {
        CursorStyle::Arrow
    };
    // How far the floating parts still have to come.
    let away = 1.0 - phase.presence;
    let flagged = edit.state.flagged();
    let automation = edit.state.automation();

    let nodes: Vec<AnyElement> = automation
        .nodes
        .iter()
        .map(|node| {
            node_view(
                node,
                placed(edit, node),
                canvas_now,
                flagged.contains(&node.id),
                edit.selected == Some(node.id),
                look,
                cx,
            )
        })
        .collect();
    // The nodes' own place is the middle of the canvas: their positions
    // are measured from it.
    let layer = div()
        .absolute()
        .left(relative(0.5))
        .top(relative(0.5))
        .size_0()
        .children(nodes);

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
            cx.listener(|shell, event: &MouseUpEvent, _, cx| {
                shell.let_go_of_the_canvas(event.position, cx);
            }),
        )
        .on_mouse_up(
            MouseButton::Middle,
            cx.listener(|shell, event: &MouseUpEvent, _, cx| {
                shell.let_go_of_the_canvas(event.position, cx);
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|shell, event: &MouseUpEvent, _, cx| {
                shell.let_go_of_the_canvas(event.position, cx);
            }),
        )
        .on_mouse_up_out(
            MouseButton::Middle,
            cx.listener(|shell, event: &MouseUpEvent, _, cx| {
                shell.let_go_of_the_canvas(event.position, cx);
            }),
        )
        .on_scroll(cx.listener(|shell, event: &ScrollEvent, _, cx| {
            let by = f32::from(event.delta.pixel_delta(px(20.)).y);
            shell.wheel_the_canvas(by, event.position, cx);
            cx.stop_propagation();
        }))
        .on_key_down(cx.listener(|shell, event: &KeyDownEvent, window, cx| {
            let keys = &event.keystroke;
            if keys.modifiers.control || keys.modifiers.platform {
                match (keys.key.as_str(), keys.modifiers.shift) {
                    ("z", false) => shell.undo_the_change(window, cx),
                    ("z", true) | ("y", false) => shell.redo_the_change(window, cx),
                    _ => return,
                }
                cx.stop_propagation();
                return;
            }
            if keys.modifiers.alt {
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
        .child(dots(canvas_now, look, editor.bounds.clone()))
        .child(arrows(canvas_now, arrows_of(edit), look))
        .children(automation.nodes.is_empty().then(|| start(canvas_now, look)))
        .child(layer);

    let top = top_bar(shell, edit, phase, away, look, cx);
    let zoom_bar = zoom_bar(canvas_now, phase, away, look, cx);

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
                        .id("editor-steps-title")
                        .test_support()
                        .role(Role::Heading)
                        .aria_level(2)
                        .aria_label(look.tr("editor-steps-title"))
                        .px(sp(8.))
                        .pt(sp(6.))
                        .pb(sp(8.))
                        .font_weight(FontWeight::BOLD)
                        .text_size(sp(14.))
                        .text_color(color(palette.text))
                        .child(look.tr("editor-steps-title")),
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
        .children(edit.said.as_ref().map(|said| said_line(said, look)))
        .child(panel)
        .into_any_element()
}

/// The way back, undo and redo, the title, and the name's field, at the top
/// left.
fn top_bar(
    shell: &Shell,
    edit: &Edit,
    phase: Phase,
    away: f32,
    look: &Look,
    cx: &mut Context<Shell>,
) -> AnyElement {
    floating(look)
        .absolute()
        .top(px(INSET - 12.0 * away))
        .left(px(INSET))
        .opacity(phase.presence)
        .flex()
        .items_center()
        .gap(sp(8.))
        .p(px(4.))
        .pr(sp(10.))
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
            tool_button(
                "editor-undo",
                look.tr("editor-undo"),
                assets::UNDO,
                edit.state.can_undo(),
                look,
            )
            .on_click(cx.listener(|shell, _, window, cx| shell.undo_the_change(window, cx))),
        )
        .child(
            tool_button(
                "editor-redo",
                look.tr("editor-redo"),
                assets::REDO,
                edit.state.can_redo(),
                look,
            )
            .on_click(cx.listener(|shell, _, window, cx| shell.redo_the_change(window, cx))),
        )
        .child(
            parts::title(&shell.title, look.tr("editor-title"), look)
                .text_size(sp(16.))
                .line_height(sp(22.))
                .text_left(),
        )
        .child(
            div().w(sp(220.)).child(
                Input::new(&edit.name)
                    .id("editor-name")
                    .aria_label(look.tr("editor-name")),
            ),
        )
        .into_any_element()
}

/// The zoom, at the bottom left.
fn zoom_bar(
    canvas_now: Canvas,
    phase: Phase,
    away: f32,
    look: &Look,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let palette = look.palette;
    let percent = canvas_now.percent().to_string();
    floating(look)
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
                    shell.fit_the_canvas(true, cx);
                }),
            ),
        )
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
            parts::icon(assets::CHEVRON, 14.)
                .text_color(color(palette.secondary))
                .with_transformation(Transformation::rotate(radians(
                    -std::f32::consts::FRAC_PI_2 * (1.0 - turned),
                ))),
        )
        .child(name.clone())
        .on_click(cx.listener(move |shell, _, _, cx| shell.toggle_the_steps(steps, cx)));

    let items = open.then(|| {
        div()
            .id((id, 1usize))
            .test_support()
            .role(Role::List)
            .aria_label(name)
            .flex()
            .flex_col()
            .gap(sp(2.))
            .pb(sp(6.))
            .children(steps.items().into_iter().enumerate().map(|(at, item)| {
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
                item_row(item, steps, (id, at), presence, travels, look, cx)
            }))
    });

    div()
        .flex()
        .flex_col()
        .child(heading)
        .children(items)
        .into_any_element()
}

/// One step the panel offers: a button that puts it on the canvas, or, for
/// what is not there yet, a line that says so.
fn item_row(
    item: Item,
    steps: Steps,
    id: (&'static str, usize),
    presence: f32,
    travels: f32,
    look: &Look,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let palette = look.palette;
    let soon = matches!(item, Item::Soon(_));
    let name = look.tr(item.id());
    let note = look.tr(&format!("{}-note", item.id()));
    let said = if soon {
        look.tr_with("editor-item-soon", &[("name", &name)])
    } else {
        look.tr_with("editor-item-add", &[("name", &name)])
    };
    let drawing = item.drawing();
    Button::new((item.id(), id.1))
        .accessibility_label(said)
        .disabled(soon)
        .w_full()
        .relative()
        .top(px(-travels * (1.0 - presence)))
        .opacity(presence * if soon { 0.6 } else { 1.0 })
        .flex()
        .items_start()
        .justify_start()
        .gap(sp(10.))
        .px(sp(8.))
        .py(sp(8.))
        .rounded(px(shape::CONTROL_RADIUS))
        .focus_visible(parts::focus_ring(&palette))
        .when(!soon, |button| {
            button
                .cursor_pointer()
                .hover(move |style| style.bg(color_at(palette.text, 0.05)))
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.insert_a_step(item.clone(), cx);
                }))
        })
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
                .child(parts::icon(drawing, 16.).text_color(color(palette.text))),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(2.))
                .text_left()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(6.))
                        .child(
                            div()
                                .text_size(sp(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(color(palette.text))
                                .child(name),
                        )
                        .children(soon.then(|| {
                            div()
                                .flex_none()
                                .px(sp(6.))
                                .rounded_full()
                                .border_1()
                                .border_color(color(palette.control_line))
                                .text_size(sp(11.))
                                .text_color(color(palette.secondary))
                                .child(look.tr("editor-soon"))
                        })),
                )
                .child(
                    div()
                        .text_size(sp(12.))
                        .line_height(sp(16.))
                        .text_color(color(palette.secondary))
                        .child(note),
                ),
        )
        .into_any_element()
}
