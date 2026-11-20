//! The editor: a canvas with no edge, moved and zoomed, that holds the
//! automation's steps and the arrows between them, under a floating panel of
//! the steps that can be added.
//!
//! **A step is a button**: Tab reaches it, Enter opens its menu, and the
//! menu holds everything the pointer does besides moving it (join it to
//! another, take an arrow off, configure, disable, remove). **A step that
//! the automation's problems are about is marked** with the error's outline
//! and its sentence in the step's name.

use std::time::{Duration, Instant};

use gpui_kit::base::{Button, TestSupportExt as _};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, ClickEvent, ContentMask, Context, Corners, CursorStyle, Div, FontWeight,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, Role, ScrollEvent, Stateful, Transformation, Window, canvas, div, fill, point, px,
    radians, relative, size,
};
use snob_core::automation::{Node, NodeId, Stage};

use super::automations::{floating, tool_button};
use super::words::{stage_name, step_detail, step_name};
use crate::state::automations::{
    ARROW_PANS, Bend, Canvas, NODE_H, NODE_W, comes_in_at, head, heading, inputs_for, leaves_at,
    route,
};
use crate::state::shelf::{At, has_a_trigger};
use crate::theme::motion;
use crate::theme::tokens::shape;
use crate::ui::assets;
use crate::ui::automations::{Edit, Editor, Item, Said, Steps};
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts;
use crate::ui::shell::{Phase, Shell};

use super::words::unjoined_words;

/// The space between the editor's floating parts and the panel's edges.
const INSET: f32 = 16.0;

/// How long after one step of the panel the next comes in.
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
                crate::ui::frames::ask(window);
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

/// The dot an arrow starts from and the one it comes into, across, in
/// logical pixels; the one an arrow being drawn sticks to grows to
/// [`PORT_SNAPPED`], and to [`PORT_OFFERED`] when it may take that arrow.
const PORT: f32 = 12.0;
const PORT_OFFERED: f32 = 14.0;
const PORT_SNAPPED: f32 = 16.0;

/// One arrow: the way it takes, in the canvas's own units, how far before
/// its end its head stops (the input's dot, in logical pixels), and what
/// ink.
struct Arrow {
    route: Vec<Bend>,
    short_of: f32,
    faint: bool,
    held: bool,
}

/// Every arrow, drawn as a curve that leaves a step sideways and comes
/// into the next one, with a head **turned along the curve's end** and
/// its tip at the input's dot.
fn arrows(canvas_now: Canvas, all: Vec<Arrow>, look: &Look) -> AnyElement {
    let palette = look.palette;
    canvas(
        move |_, _, _| {},
        move |bounds, (), window, _| {
            let middle = bounds.center();
            let on_screen = |at: (f32, f32)| {
                let on = canvas_now.on_screen(at);
                (f32::from(middle.x) + on.0, f32::from(middle.y) + on.1)
            };
            let screen = |at: (f32, f32)| {
                let (x, y) = on_screen(at);
                point(px(x), px(y))
            };
            let zoom = canvas_now.zoom();
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for arrow in &all {
                    let Some(end) = arrow.route.last().map(|bend| bend[3]) else {
                        continue;
                    };
                    let ink = if arrow.held {
                        color(palette.accent)
                    } else if arrow.faint {
                        color_at(palette.secondary, 0.35)
                    } else {
                        color_at(palette.secondary, 0.9)
                    };
                    let mut line = PathBuilder::stroke(px((2.0 * zoom).max(1.0)));
                    line.move_to(screen(arrow.route[0][0]));
                    for [_, ca, cb, b] in &arrow.route {
                        line.cubic_bezier_to(screen(*b), screen(*ca), screen(*cb));
                    }
                    if let Ok(line) = line.build() {
                        window.paint_path(line, ink);
                    }
                    let way = heading(&arrow.route);
                    let end = on_screen(end);
                    let tip = (
                        end.0 - way.0 * arrow.short_of,
                        end.1 - way.1 * arrow.short_of,
                    );
                    let [tip, one, other] = head(tip, way, (9.0 * zoom).max(4.0));
                    let mut drawn = PathBuilder::fill();
                    drawn.move_to(point(px(tip.0), px(tip.1)));
                    drawn.line_to(point(px(one.0), px(one.1)));
                    drawn.line_to(point(px(other.0), px(other.1)));
                    drawn.close();
                    if let Ok(drawn) = drawn.build() {
                        window.paint_path(drawn, ink);
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
/// counted: **one that sticks to an input ends there**, else at the
/// pointer.
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
                route: route(leaves_at(from), comes_in_at(to), false),
                short_of: PORT / 2.0 + 1.0,
                faint: !(on && off),
                held: false,
            })
        })
        .collect();
    if let Some(wire) = &edit.wire
        && let Some((from, _)) = at(wire.from)
    {
        let stuck = wire.snapped.and_then(at).map(|(to, _)| comes_in_at(to));
        all.push(Arrow {
            route: route(leaves_at(from), stuck.unwrap_or(wire.to), stuck.is_none()),
            short_of: if stuck.is_some() {
                PORT_SNAPPED / 2.0 + 1.0
            } else {
                0.0
            },
            faint: false,
            held: true,
        });
    }
    all
}

/// How a step's input looks while an arrow is drawn: offered to it, stuck
/// to it, or no place for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Port {
    Offered,
    Snapped,
    Off,
}

/// How the input of `node` looks under the arrow `edit` draws, if any,
/// which may stick to the steps `offered` (`inputs_for`).
fn port_of(edit: &Edit, node: &Node, offered: &[NodeId]) -> Option<Port> {
    let wire = edit.wire.as_ref()?;
    Some(if wire.snapped == Some(node.id) {
        Port::Snapped
    } else if offered.contains(&node.id) {
        Port::Offered
    } else {
        Port::Off
    })
}

/// A dot an arrow leaves or comes into, `across` logical pixels, in a box
/// of 24 a side astride the step's edge.
fn port_dot(across: f32, look: &Look) -> Div {
    let palette = look.palette;
    div()
        .size(px(across))
        .rounded_full()
        .border_2()
        .border_color(color(palette.panel))
        .bg(color(palette.accent))
}

/// One step on the canvas: a button that moves with the pointer, opens its
/// menu on a right press or Enter, and carries the handle an arrow is drawn
/// from, on its right, and the dot arrows come into, on its left.
#[expect(clippy::too_many_arguments, reason = "one step's worth of the canvas")]
fn node_view(
    node: &Node,
    at: (i32, i32),
    canvas_now: Canvas,
    flagged: bool,
    selected: bool,
    port: Option<Port>,
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
    // The dots sit astride the step's outer edge, which its border is
    // inside of.
    let border = (if flagged || selected { 2.0 } else { 1.0 }) * zoom.max(0.5);
    let leads_on = node.step.stage() != Stage::Output;
    let led_to = node.step.stage() != Stage::Trigger;
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
        .border(px(border))
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
        .children(led_to.then(|| {
            let across = match port {
                Some(Port::Snapped) => PORT_SNAPPED,
                Some(Port::Offered) => PORT_OFFERED,
                Some(Port::Off) | None => PORT,
            };
            div()
                .id(("node-input", id as usize))
                .test_support()
                .absolute()
                .left(px(-12. - border))
                .top(relative(0.5))
                .mt(px(-12.))
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .when(port == Some(Port::Off), |input| input.opacity(0.3))
                .child(
                    port_dot(across, look).when(port == Some(Port::Snapped), |dot| {
                        dot.shadow(vec![gpui_kit::BoxShadow {
                            color: color_at(palette.accent, 0.3),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(0.),
                            spread_radius: px(3.),
                            inset: false,
                        }])
                    }),
                )
        }))
        .children(leads_on.then(|| {
            div()
                .id(("node-output", id as usize))
                .test_support()
                .absolute()
                .right(px(-12. - border))
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
                .child(port_dot(PORT, look))
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
    // The panel follows the automation: held at the triggers while it has
    // none, back at the kinds once one comes, an undo or a removal included.
    if let Some(page) = &mut shell.automations
        && let Some(edit) = &page.edit
        && page
            .editor
            .shelf
            .follow(has_a_trigger(edit.state.automation()))
    {
        page.editor.shelf_since = Some(now);
        page.editor.search = None;
    }
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
    let offered: Vec<NodeId> = edit
        .wire
        .map(|wire| inputs_for(automation, wire.from))
        .unwrap_or_default()
        .into_iter()
        .map(|(id, _)| id)
        .collect();

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
                port_of(edit, node, &offered),
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

    let searching = editor.search.is_some();
    let shelf = shelf(editor, now, reduce, look, window, cx);
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
                .overflow_x_hidden()
                .p(sp(8.))
                // A press anywhere else closes the search; one in the panel
                // acts as usual, as a step put on the canvas closes it.
                .when(searching, |panel| {
                    panel.on_mouse_down_out(cx.listener(|shell, _: &MouseDownEvent, window, cx| {
                        shell.close_the_search_away(window, cx);
                    }))
                })
                .child(shelf),
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

/// How far into coming in something is, `elapsed` after it started to,
/// staggered `at` places after the first; a frame is asked for until it is
/// in. All the way in for what came with the editor.
fn coming_in(
    elapsed: Option<Duration>,
    at: usize,
    lasts: Duration,
    reduce: bool,
    window: &mut Window,
) -> f32 {
    let Some(elapsed) = elapsed else {
        return 1.0;
    };
    let starts = if reduce {
        Duration::ZERO
    } else {
        STEP_STAGGER * at as u32
    };
    let through = elapsed.saturating_sub(starts).as_secs_f32() / lasts.as_secs_f32();
    if through < 1.0 {
        crate::ui::frames::ask(window);
    }
    motion::ease_out(through)
}

/// The panel of steps: the kinds of step, each a way in, or the steps of
/// one under the way back and a magnifier. What it shows slides in from
/// the side it was reached from, and fades in alone under "reduce motion".
fn shelf(
    editor: &Editor,
    now: Instant,
    reduce: bool,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let since = editor
        .shelf_since
        .map(|at| now.saturating_duration_since(at));
    let moved = look.moved(motion::ENTER, 16.0, 1.0);
    let came = coming_in(since, 0, moved.lasts, reduce, window);
    let side = if editor.shelf.forward() { 1.0 } else { -1.0 };
    let body = match editor.shelf.at() {
        At::Kinds => kinds(editor, since, reduce, look, window, cx),
        At::In(kind) => steps_of(
            Steps::of(kind),
            editor,
            since,
            now,
            reduce,
            look,
            window,
            cx,
        ),
    };
    div()
        .relative()
        .left(px(side * moved.travels * (1.0 - came)))
        .opacity(came)
        .flex()
        .flex_col()
        .gap(sp(2.))
        .children(body)
        .into_any_element()
}

/// The drawing a kind of step is known by in the panel.
fn kind_drawing(steps: Steps) -> &'static str {
    match steps {
        Steps::Triggers => assets::AUTOMATIONS,
        Steps::Actions => assets::FOLLOWERS,
        Steps::Filters => assets::CONDITION,
        Steps::Outputs => assets::WEBHOOK,
    }
}

/// The square a step's or a kind's drawing sits in: tinted with the accent
/// for triggers, as what starts an automation.
fn drawing_box(drawing: &'static str, trigger: bool, look: &Look) -> Div {
    let palette = look.palette;
    div()
        .flex_none()
        .size(sp(30.))
        .rounded(px(shape::CONTROL_RADIUS))
        .flex()
        .items_center()
        .justify_center()
        .bg(if trigger {
            color_at(palette.accent, 0.14)
        } else {
            color_at(palette.text, 0.06)
        })
        .child(parts::icon(drawing, 16.).text_color(color(palette.text)))
}

/// The panel's heading: what it shows, named once on the screen.
fn shelf_heading(name: String, look: &Look) -> Stateful<Div> {
    div()
        .id("editor-steps-title")
        .role(Role::Heading)
        .aria_level(2)
        .aria_label(name.clone())
        .flex_1()
        .min_w_0()
        .font_weight(FontWeight::BOLD)
        .text_size(sp(14.))
        .text_color(color(look.palette.text))
        .truncate()
        .child(name)
}

/// The four kinds of step, each a wide button into its steps.
fn kinds(
    editor: &Editor,
    since: Option<Duration>,
    reduce: bool,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    let palette = look.palette;
    let moved = look.moved(motion::ENTER, 6.0, 1.0);
    let mut shown = vec![
        shelf_heading(look.tr("editor-steps-title"), look)
            .px(sp(8.))
            .pt(sp(6.))
            .pb(sp(8.))
            .test_support()
            .into_any_element(),
    ];
    for steps in Steps::ALL {
        let (id, name) = steps_heading(steps, look);
        let note = look.tr(&format!("{id}-note"));
        let presence = coming_in(since, steps.at() + 1, moved.lasts, reduce, window);
        shown.push(
            Button::new(id)
                .accessibility_label(name.clone())
                .track_focus(&editor.kinds[steps.at()])
                .w_full()
                .relative()
                .top(px(moved.travels * (1.0 - presence)))
                .opacity(presence)
                .flex()
                .items_center()
                .justify_start()
                .gap(sp(10.))
                .px(sp(8.))
                .py(sp(8.))
                .rounded(px(shape::CONTROL_RADIUS))
                .focus_visible(parts::focus_ring(&palette))
                .cursor_pointer()
                .hover(move |style| style.bg(color_at(palette.text, 0.05)))
                .child(drawing_box(
                    kind_drawing(steps),
                    steps == Steps::Triggers,
                    look,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(2.))
                        .text_left()
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
                                .child(note),
                        ),
                )
                .child(
                    parts::icon(assets::CHEVRON, 14.)
                        .flex_none()
                        .text_color(color(palette.secondary))
                        .with_transformation(Transformation::rotate(radians(
                            -std::f32::consts::FRAC_PI_2,
                        ))),
                )
                .on_click(cx.listener(move |shell, _, window, cx| {
                    shell.enter_the_steps(steps, window, cx);
                }))
                .into_any_element(),
        );
    }
    shown
}

/// The steps of one kind: the way back (none while the panel is held at
/// the triggers), the kind's name, a magnifier that opens a search in
/// them **over that name, on its line**, and each step that the search
/// finds, coming in one after the other.
#[expect(clippy::too_many_arguments, reason = "one frame's worth of the panel")]
fn steps_of(
    steps: Steps,
    editor: &Editor,
    since: Option<Duration>,
    now: Instant,
    reduce: bool,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    let palette = look.palette;
    let (id, name) = steps_heading(steps, look);
    let held = editor.shelf.held();
    let search = editor.search.as_ref();
    let typed = search
        .map(|search| search.input.read(cx).value().to_string())
        .unwrap_or_default();
    // The search opens over the title, on its line: the field grows out of
    // the magnifier, from the right, as the title fades under it; closed,
    // the title comes back. Under "reduce motion" both only fade.
    let moved = look.moved(motion::INTERACTION, 8.0, 1.0);
    let title_row = match search {
        Some(search) => {
            let opened = now.saturating_duration_since(search.since);
            let grown = coming_in(Some(opened), 0, moved.lasts, reduce, window);
            let width = if reduce { 1.0 } else { 0.2 + 0.8 * grown };
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_end()
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .when(held, |title| title.px(sp(8.)))
                        .opacity(1.0 - grown)
                        .font_weight(FontWeight::BOLD)
                        .text_size(sp(14.))
                        .text_color(color(palette.text))
                        .child(div().min_w_0().truncate().child(name.clone())),
                )
                .child(
                    div().w(relative(width)).opacity(grown).child(
                        Input::new(&search.input)
                            .id("editor-shelf-field")
                            .aria_label(look.tr("editor-shelf-search")),
                    ),
                )
                .into_any_element()
        }
        None => {
            let back = coming_in(
                editor
                    .search_closed
                    .map(|closed| now.saturating_duration_since(closed)),
                0,
                moved.lasts,
                reduce,
                window,
            );
            shelf_heading(name.clone(), look)
                .when(held, |heading| heading.px(sp(8.)))
                .relative()
                .left(px(moved.travels * (1.0 - back)))
                .opacity(back)
                .test_support()
                .into_any_element()
        }
    };
    let header = div()
        .flex()
        .items_center()
        .gap(sp(4.))
        .pb(sp(4.))
        .children((!held).then(|| {
            tool_button(
                "editor-shelf-back",
                look.tr("editor-shelf-back"),
                assets::BACK,
                true,
                look,
            )
            .track_focus(&editor.shelf_back)
            .on_click(cx.listener(|shell, _, window, cx| {
                shell.back_to_the_kinds(window, cx);
            }))
        }))
        .child(title_row)
        .child(
            tool_button(
                "editor-shelf-search",
                look.tr("editor-shelf-search"),
                assets::SEARCH,
                true,
                look,
            )
            .track_focus(&editor.shelf_search)
            .aria_expanded(search.is_some())
            .on_click(cx.listener(|shell, _, window, cx| {
                shell.toggle_the_search(window, cx);
            })),
        );

    let first = held.then(|| {
        div()
            .id("editor-shelf-first")
            .test_support()
            .role(Role::Note)
            .aria_label(look.tr("editor-shelf-first"))
            .px(sp(8.))
            .pb(sp(6.))
            .text_size(sp(12.))
            .line_height(sp(16.))
            .text_color(color(palette.secondary))
            .child(look.tr("editor-shelf-first"))
    });

    let found: Vec<(usize, Item)> = steps
        .items()
        .into_iter()
        .enumerate()
        .filter(|(_, item)| {
            let name = look.tr(item.id());
            let note = look.tr(&format!("{}-note", item.id()));
            crate::state::shelf::found(&name, &note, &typed)
        })
        .collect();
    let moved = look.moved(motion::ENTER, 6.0, 1.0);
    let travels = if reduce { 0.0 } else { 6.0 };
    let list = if found.is_empty() {
        let none = look.tr_with("editor-shelf-none", &[("typed", typed.trim())]);
        div()
            .id("editor-shelf-none")
            .test_support()
            .role(Role::Status)
            .aria_label(none.clone())
            .px(sp(8.))
            .py(sp(10.))
            .text_size(sp(12.5))
            .text_color(color(palette.secondary))
            .child(none)
            .into_any_element()
    } else {
        div()
            .id((id, 1usize))
            .test_support()
            .role(Role::List)
            .aria_label(name)
            .flex()
            .flex_col()
            .gap(sp(2.))
            .pb(sp(6.))
            .children(found.into_iter().enumerate().map(|(shown, (at, item))| {
                let presence = coming_in(since, shown + 1, moved.lasts, reduce, window);
                item_row(item, steps, (id, at), presence, travels, look, cx)
            }))
            .into_any_element()
    };

    let mut shown = vec![header.into_any_element()];
    shown.extend(first.map(IntoElement::into_any_element));
    shown.push(list);
    shown
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
                .on_click(cx.listener(move |shell, _, window, cx| {
                    shell.insert_a_step(item.clone(), window, cx);
                }))
        })
        .child(drawing_box(drawing, steps == Steps::Triggers, look))
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
