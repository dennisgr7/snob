//! The automations' own pages and their editor, driven in process on a
//! scripted backend: the list that opens a page with its history and its
//! flow in words, the editor that keeps every change at once and takes it
//! back, the menu and the dialogs over a step, and the question asked on
//! leaving what cannot run.

#![cfg(feature = "testing")]

mod common;

use std::time::Duration;

use common::{App, disabled, open_with};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Role, TestAppContext, px};
use snob_core::Epoch;
use snob_core::automation::{Account, Automation, Read, Step};
use snob_core::watch::schedule::Spec;
use snob_desktop::backend::scripted::{Asked, Scripted, account};
use snob_desktop::backend::{Next, Run};
use snob_desktop::state::flow::Screen;
use snob_desktop::system::Settings;
use snob_desktop::theme::tokens::shape::FOCUS_ROOM;

fn still() -> Settings {
    Settings {
        reduce_motion: true,
        ..Settings::default()
    }
}

/// Lets the moves under way end, and the answers of the backend come.
fn settle(app: &App, cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    app.in_window(cx, |_, _| ());
    cx.run_until_parked();
    app.in_window(cx, |_, _| ());
}

fn every(text: &str) -> Spec {
    Spec {
        every: Some(snob_core::duration::parse(text).unwrap()),
        ..Spec::default()
    }
}

/// A trigger every six hours that leads to an action on the lists.
fn daily() -> Automation {
    let mut automation = Automation::new("a1", "Daily lists");
    let trigger = automation.insert(Step::Schedule { when: every("6h") }, (0, 0));
    let action = automation.insert(
        Step::Read {
            account: Some(Account::own(Some(42.into()))),
            what: Read::Lists,
        },
        (260, 0),
    );
    automation.connect(trigger, action).unwrap();
    automation
}

fn run_of(automation: &str, at: i64, outcome: Option<&str>) -> Run {
    Run {
        id: at,
        automation: automation.to_string(),
        started_at: Epoch::new(at),
        ended_at: outcome.map(|_| Epoch::new(at + 60)),
        outcome: outcome.map(str::to_string),
        requests: 12,
        summary: "Read 120 followers.\nNobody left.".to_string(),
        running: false,
        found: None,
    }
}

fn scripted(automations: Vec<Automation>) -> Scripted {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.introduced(42);
        script.next_run(Next::Never);
        for automation in automations {
            script.automation(automation);
        }
    });
    backend
}

async fn in_list(cx: &mut TestAppContext, backend: Scripted) -> App {
    let app = open_with(cx, backend, still());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    settle(&app, cx);
    app
}

/// The editor, open on the first automation of the list.
async fn in_editor(cx: &mut TestAppContext, backend: Scripted) -> App {
    let app = in_list(cx, backend).await;
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    app.click(cx, "automation-edit");
    settle(&app, cx);
    app
}

/// A press on the control `id`, whatever kind of id it has.
fn click(app: &App, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) {
    let id = id.into();
    app.in_window(cx, |window, cx| window.click(id, cx));
}

fn shown(app: &App, cx: &mut TestAppContext, id: &'static str) -> bool {
    app.in_window(cx, |window, _| {
        window.try_find(id).is_some_and(|found| found.visible())
    })
}

#[gpui_kit::test]
async fn the_list_opens_a_page_with_its_history_its_flow_in_words_and_a_pencil(
    cx: &mut TestAppContext,
) {
    let backend = scripted(vec![daily()]);
    backend.script(|script| {
        script.run(run_of("a1", 1_790_000_000, Some("ok")));
        script.run(run_of("a1", 1_789_000_000, Some("rate_limited")));
    });
    let app = in_list(cx, backend.clone()).await;
    app.in_window(cx, |window, _| {
        let row = window.find(("automation", 0usize));
        assert_eq!(row.role(), Some(Role::Button));
        assert_eq!(row.label(), Some("Daily lists. Steps: 2"));
    });

    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    assert_eq!(app.title(cx), ("Daily lists".to_string(), true));
    assert!(
        backend
            .asked()
            .contains(&Asked::AutomationRuns("a1".to_string()))
    );
    app.in_window(cx, |window, _| {
        let first = window.find(("automation-run-row", 0usize));
        assert_eq!(first.role(), Some(Role::ListItem));
        let said = first.label().unwrap_or_default();
        assert!(
            said.contains("Finished") && said.contains("Requests: 12"),
            "{said}"
        );
        let second = window.find(("automation-run-row", 1usize));
        assert!(
            second
                .label()
                .unwrap_or_default()
                .contains("Paused by Instagram")
        );

        let steps = window.find("automation-steps");
        assert_eq!(steps.role(), Some(Role::List));
        assert_eq!(
            window.find(("automation-step", 0usize)).label(),
            Some("Starts. On a schedule. Every 6h")
        );
        assert_eq!(
            window.find(("automation-step", 1usize)).label(),
            Some("Reads. Followers and following. Your account")
        );
        let pencil = window.find("automation-edit");
        assert_eq!(pencil.role(), Some(Role::Button));
        assert_eq!(pencil.label(), Some("Edit the automation"));
        let size = pencil.bounds().size;
        assert!(size.width >= gpui_kit::px(24.) && size.height >= gpui_kit::px(24.));
    });

    app.press(cx, "escape");
    settle(&app, cx);
    assert!(
        shown(&app, cx, "automations-create"),
        "Escape goes back to the list"
    );
}

#[gpui_kit::test]
async fn a_step_added_is_kept_at_once_and_undo_takes_it_back(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    let app = in_editor(cx, backend.clone()).await;
    assert_eq!(app.title(cx), ("Edit automation".to_string(), true));
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find(("node", 1usize)).label(),
            Some("Starts. On a schedule. Every 6h")
        );
        assert_eq!(window.find(("node", 2usize)).role(), Some(Role::Button));
    });
    assert!(disabled(&app, cx, "editor-undo"), "nothing to undo yet");

    app.click(cx, "editor-triggers");
    settle(&app, cx);
    click(&app, cx, ("trigger-schedule", 0usize));
    settle(&app, cx);
    assert_eq!(
        backend.automations()[0].nodes.len(),
        3,
        "kept without being asked"
    );
    assert!(!disabled(&app, cx, "editor-undo"));

    app.click(cx, "editor-undo");
    settle(&app, cx);
    assert_eq!(
        backend.automations()[0],
        daily(),
        "and taken back, and kept"
    );
    assert!(disabled(&app, cx, "editor-undo"));
    app.click(cx, "editor-redo");
    settle(&app, cx);
    assert_eq!(backend.automations()[0].nodes.len(), 3);
}

#[gpui_kit::test]
async fn the_menu_of_a_step_disables_joins_and_removes(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    let app = in_editor(cx, backend.clone()).await;
    app.in_window(cx, |window, cx| window.right_click(("node", 1usize), cx));
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("node-menu").role(), Some(Role::Menu));
        assert_eq!(window.find("node-menu-enabled").label(), Some("Disable"));
        assert_eq!(
            window.find("node-menu-configure").label(),
            Some("Trigger configuration")
        );
        assert_eq!(
            window.find(("node-menu-disconnect", 2usize)).label(),
            Some("Disconnect from Followers and following (Your account)")
        );
    });
    app.click(cx, "node-menu-enabled");
    settle(&app, cx);
    assert!(!backend.automations()[0].nodes[0].enabled);
    assert!(!shown(&app, cx, "node-menu"), "the menu closes");

    app.in_window(cx, |window, cx| window.right_click(("node", 1usize), cx));
    settle(&app, cx);
    assert_eq!(app.label_of(cx, "node-menu-enabled"), "Enable");
    click(&app, cx, ("node-menu-disconnect", 2usize));
    settle(&app, cx);
    assert!(backend.automations()[0].edges.is_empty());

    app.in_window(cx, |window, cx| window.right_click(("node", 2usize), cx));
    settle(&app, cx);
    assert_eq!(app.label_of(cx, "node-menu-configure"), "Configure");
    app.click(cx, "node-menu-remove");
    settle(&app, cx);
    assert_eq!(backend.automations()[0].nodes.len(), 1);
}

#[gpui_kit::test]
async fn the_schedule_dialog_says_the_engines_reason_as_it_is_typed_and_saves_what_builds(
    cx: &mut TestAppContext,
) {
    let mut empty = daily();
    empty.nodes[0].step = Step::Schedule {
        when: Spec::default(),
    };
    let backend = scripted(vec![empty]);
    let app = in_editor(cx, backend.clone()).await;
    app.in_window(cx, |window, cx| window.right_click(("node", 1usize), cx));
    app.click(cx, "node-menu-configure");
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        let dialog = window.find("step-dialog");
        assert_eq!(dialog.role(), Some(Role::Dialog));
        assert_eq!(dialog.label(), Some("Trigger configuration"));
        assert_eq!(window.find("dialog-problems").role(), Some(Role::Status));
    });
    assert!(disabled(&app, cx, "dialog-save"), "no schedule is said yet");

    app.in_window(cx, |window, cx| window.input("soon", cx));
    settle(&app, cx);
    assert!(disabled(&app, cx, "dialog-save"), "not a duration");

    app.in_window(cx, |window, cx| {
        window.press("ctrl-a", cx);
        window.input("6h", cx);
    });
    settle(&app, cx);
    assert!(shown(&app, cx, "dialog-save"));
    assert!(!disabled(&app, cx, "dialog-save"));
    assert!(!shown(&app, cx, "dialog-problems"));
    app.click(cx, "dialog-save");
    settle(&app, cx);
    assert_eq!(
        backend.automations()[0].nodes[0].step,
        Step::Schedule { when: every("6h") }
    );
    assert!(!shown(&app, cx, "step-dialog"));
}

#[gpui_kit::test]
async fn a_dialogs_focused_field_has_room_for_its_ring_on_every_side_and_stays_in_its_place(
    cx: &mut TestAppContext,
) {
    let backend = scripted(vec![daily()]);
    let app = in_editor(cx, backend).await;
    app.in_window(cx, |window, cx| window.right_click(("node", 1usize), cx));
    app.click(cx, "node-menu-configure");
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        let field = window.find("schedule-every");
        assert_eq!(field.focused(), Some(true));
        let field = field.bounds();
        let card = window.find("step-dialog").bounds();
        let clip = window.find(("step-dialog", 2usize)).bounds();
        let room = px(FOCUS_ROOM);

        // Where it was before the room: inside the card's outline and its
        // padding of 24 at the default text size.
        assert_eq!(field.left() - card.left(), px(1. + 24.));
        assert_eq!(card.right() - field.right(), px(1. + 24.));

        // What clips the dialog's body leaves the ring its reach.
        assert_eq!(field.left() - clip.left(), room);
        assert_eq!(clip.right() - field.right(), room);
        assert!(field.top() - clip.top() >= room, "{field:?} in {clip:?}");
        assert!(
            clip.bottom() - field.bottom() >= room,
            "{field:?} in {clip:?}"
        );
    });
}

#[gpui_kit::test]
async fn leaving_what_cannot_run_asks_and_discarding_puts_it_back(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    let app = in_editor(cx, backend.clone()).await;
    app.in_window(cx, |window, cx| window.right_click(("node", 2usize), cx));
    app.click(cx, "node-menu-remove");
    settle(&app, cx);
    assert_eq!(backend.automations()[0].nodes.len(), 1);

    app.click(cx, "editor-back");
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        let question = window.find("leave-question");
        assert_eq!(question.role(), Some(Role::Dialog));
        assert_eq!(
            window.find(("leave-problem", 0usize)).label(),
            Some("No trigger leads to an action, so a run would read nothing.")
        );
    });
    app.click(cx, "leave-keep");
    settle(&app, cx);
    assert!(!shown(&app, cx, "leave-question"));
    assert!(shown(&app, cx, "editor-canvas"), "still editing");

    app.press(cx, "escape");
    settle(&app, cx);
    assert!(shown(&app, cx, "leave-question"), "Escape asks too");
    app.click(cx, "leave-discard");
    settle(&app, cx);
    assert_eq!(
        backend.automations()[0],
        daily(),
        "as it was when it opened"
    );
    assert_eq!(app.title(cx), ("Daily lists".to_string(), true));
}

#[gpui_kit::test]
async fn pressing_the_rail_with_an_unfinished_automation_asks_and_goes_on_after_a_discard(
    cx: &mut TestAppContext,
) {
    let backend = scripted(vec![daily()]);
    let app = in_editor(cx, backend.clone()).await;
    app.in_window(cx, |window, cx| window.right_click(("node", 1usize), cx));
    app.click(cx, "node-menu-remove");
    settle(&app, cx);

    app.click(cx, "rail-home");
    settle(&app, cx);
    assert!(shown(&app, cx, "leave-question"));
    let shell = app.shell.clone();
    let now = app.in_window(cx, |_, cx| shell.read(cx).shown().clone());
    assert_eq!(
        now,
        Screen::Automations,
        "held until the question is answered"
    );
    app.click(cx, "leave-discard");
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    settle(&app, cx);
    assert_eq!(backend.automations()[0], daily());
}

#[gpui_kit::test]
async fn a_new_automation_is_kept_once_something_is_done_and_discarding_removes_it(
    cx: &mut TestAppContext,
) {
    let backend = scripted(Vec::new());
    let app = in_list(cx, backend.clone()).await;
    app.click(cx, "automations-create");
    settle(&app, cx);
    assert!(
        backend.automations().is_empty(),
        "nothing kept before a change"
    );

    click(&app, cx, ("trigger-schedule", 0usize));
    settle(&app, cx);
    let kept = backend.automations();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].name, "New automation");
    let id = kept[0].id.clone();

    app.click(cx, "editor-back");
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find(("leave-problem", 0usize)).label(),
            Some("No trigger leads to an action, so a run would read nothing.")
        );
    });
    app.click(cx, "leave-discard");
    settle(&app, cx);
    assert!(backend.asked().contains(&Asked::RemoveAutomation(id)));
    assert!(backend.automations().is_empty());
    assert!(
        shown(&app, cx, "automations-empty"),
        "back to an empty list"
    );
}

#[gpui_kit::test]
async fn the_keyboard_opens_a_steps_menu_and_undoes_and_redoes_what_was_done(
    cx: &mut TestAppContext,
) {
    let backend = scripted(vec![daily()]);
    let app = in_editor(cx, backend.clone()).await;
    click(&app, cx, ("node", 1usize));
    settle(&app, cx);
    app.press(cx, "enter");
    settle(&app, cx);
    assert!(
        shown(&app, cx, "node-menu"),
        "Enter opens the menu of the step"
    );
    app.click(cx, "node-menu-enabled");
    settle(&app, cx);
    assert!(!backend.automations()[0].nodes[0].enabled);

    app.press(cx, "ctrl-z");
    settle(&app, cx);
    assert!(
        backend.automations()[0].nodes[0].enabled,
        "Ctrl+Z takes it back"
    );
    app.press(cx, "ctrl-shift-z");
    settle(&app, cx);
    assert!(
        !backend.automations()[0].nodes[0].enabled,
        "and Ctrl+Shift+Z does it again"
    );
    app.press(cx, "ctrl-z");
    settle(&app, cx);
    app.press(cx, "ctrl-y");
    settle(&app, cx);
    assert!(!backend.automations()[0].nodes[0].enabled, "Ctrl+Y too");
}

#[gpui_kit::test]
async fn a_name_typed_renames_the_automation_and_an_empty_one_is_what_leaving_asks_about(
    cx: &mut TestAppContext,
) {
    let backend = scripted(vec![daily()]);
    let app = in_editor(cx, backend.clone()).await;
    click(&app, cx, "editor-name");
    app.in_window(cx, |window, cx| {
        window.press("ctrl-a", cx);
        window.input("Weekly lists", cx);
    });
    settle(&app, cx);
    assert_eq!(backend.automations()[0].name, "Weekly lists");

    app.in_window(cx, |window, cx| {
        window.press("ctrl-a", cx);
        window.press("backspace", cx);
    });
    settle(&app, cx);
    assert_eq!(backend.automations()[0].name, "", "kept as it is typed");
    app.click(cx, "editor-back");
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find(("leave-problem", 0usize)).label(),
            Some("It has no name.")
        );
    });
    app.click(cx, "leave-discard");
    settle(&app, cx);
    assert_eq!(backend.automations()[0], daily());
}

#[gpui_kit::test]
async fn a_failure_to_keep_is_said_under_the_canvas(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    backend.script(|script| {
        script.save_fails("the disk is full");
    });
    let app = in_editor(cx, backend.clone()).await;
    app.click(cx, "editor-triggers");
    settle(&app, cx);
    click(&app, cx, ("trigger-schedule", 0usize));
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        let said = window.find("editor-said");
        assert_eq!(said.role(), Some(Role::Status));
        assert_eq!(
            said.label(),
            Some("The automation could not be kept: the disk is full")
        );
    });
}

/// What the pointer does between a press and a release: it goes to each
/// of `path` in turn with the left button held.
fn drag(
    app: &App,
    cx: &mut TestAppContext,
    from: gpui_kit::Point<gpui_kit::Pixels>,
    path: &[gpui_kit::Point<gpui_kit::Pixels>],
) {
    use gpui_kit::{
        Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput,
    };
    let modifiers = Modifiers::default();
    app.in_window(cx, |window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: from,
                pressed_button: None,
                modifiers,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: from,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        for at in path {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: *at,
                    pressed_button: Some(MouseButton::Left),
                    modifiers,
                }),
                cx,
            );
        }
        let last = path.last().copied().unwrap_or(from);
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: last,
                modifiers,
                click_count: 1,
            }),
            cx,
        );
    });
    settle(app, cx);
}

#[gpui_kit::test]
async fn a_step_dragged_is_kept_where_it_is_dropped_and_an_arrow_is_drawn_from_its_handle(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{point, px};
    let mut open = daily();
    open.edges.clear();
    let backend = scripted(vec![open.clone()]);
    let app = in_editor(cx, backend.clone()).await;
    let at = |app: &App, cx: &mut TestAppContext, id: usize| {
        app.in_window(cx, |window, _| window.find(("node", id)).bounds())
    };

    // Dropped 80 pixels lower, which is that much more at the canvas's
    // own size or less: it moved down and not across.
    let node = at(&app, cx, 1);
    let from = node.center();
    drag(
        &app,
        cx,
        from,
        &[from + point(px(0.), px(40.)), from + point(px(0.), px(80.))],
    );
    let moved = backend.automations()[0].nodes[0].at;
    assert_eq!(moved.0, open.nodes[0].at.0, "not across");
    assert!(moved.1 > open.nodes[0].at.1, "down: {moved:?}");
    assert!(at(&app, cx, 1).origin.y > node.origin.y);

    // An arrow from the first step's handle, at its right edge, dropped
    // on the second.
    let first = at(&app, cx, 1);
    let handle = point(first.right(), first.center().y);
    let target = at(&app, cx, 2).center();
    drag(&app, cx, handle, &[handle + point(px(20.), px(0.)), target]);
    assert_eq!(backend.automations()[0].edges.len(), 1, "joined");
    assert_eq!(
        backend.automations()[0].nodes[0].at,
        moved,
        "the step did not move with it"
    );

    // The same again says why it is refused.
    drag(&app, cx, handle, &[target]);
    assert_eq!(backend.automations()[0].edges.len(), 1);
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find("editor-said").label(),
            Some("Those two steps are already joined.")
        );
    });
}

#[gpui_kit::test]
async fn the_bands_way_back_leaves_the_editor_then_the_automations_page(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    let app = in_editor(cx, backend).await;
    assert!(!disabled(&app, cx, "band-back"));
    app.click(cx, "band-back");
    settle(&app, cx);
    assert!(
        shown(&app, cx, "automation-edit"),
        "on the automation's page"
    );
    app.click(cx, "band-back");
    settle(&app, cx);
    assert!(shown(&app, cx, "automations-create"), "on the list");
}
