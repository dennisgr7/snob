//! The automations' own pages and their editor, driven in process on a
//! scripted backend: the list that opens a page with its history and its
//! flow in words, the editor that keeps every change at once and takes it
//! back, the menu and the dialogs over a step, and the question asked on
//! leaving what cannot run.

#![cfg(feature = "testing")]

mod common;

use std::time::Duration;

use common::{App, disabled, focused, open_with};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Role, TestAppContext, VisualTestContext, px};
use snob_core::Epoch;
use snob_core::automation::{Account, Automation, Read, Step};
use snob_core::watch::schedule::Spec;
use snob_desktop::backend::scripted::{Asked, Scripted, account};
use snob_desktop::backend::{Next, Run};
use snob_desktop::state::flow::Screen;
use snob_desktop::state::rest::QUIET_FOR;
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
        assert_eq!(
            row.label(),
            Some("Daily lists. Followers and following (Your account). Every 6 hours")
        );
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
            Some("Starts. On a schedule. Every 6 hours")
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
            Some("Starts. On a schedule. Every 6 hours")
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
            Some("Disconnect from Followers and following")
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

/// A trigger joined to the lists of the account in use and to those of
/// somebody else: two steps of one name.
fn two_lists() -> Automation {
    let mut automation = daily();
    let other = automation.insert(
        Step::Read {
            account: Some(Account {
                username: Some("someone".to_string()),
                ..Account::own(Some(42.into()))
            }),
            what: Read::Lists,
        },
        (260, 120),
    );
    automation.connect(1, other).unwrap();
    automation
}

/// The labels of the entries of a step's menu that take an arrow off it.
fn disconnect_labels(app: &App, cx: &mut TestAppContext, step: usize) -> Vec<String> {
    app.in_window(cx, |window, cx| window.right_click(("node", step), cx));
    settle(app, cx);
    let labels = app.in_window(cx, |window, _| {
        (1..=4usize)
            .filter_map(|to| window.try_find(("node-menu-disconnect", to)))
            .map(|entry| entry.label().unwrap_or_default().to_string())
            .collect()
    });
    app.press(cx, "escape");
    settle(app, cx);
    labels
}

#[gpui_kit::test]
async fn a_step_to_disconnect_from_is_named_without_its_detail(cx: &mut TestAppContext) {
    let app = in_editor(cx, scripted(vec![daily()])).await;
    assert_eq!(
        disconnect_labels(&app, cx, 1),
        ["Disconnect from Followers and following"]
    );
}

#[gpui_kit::test]
async fn a_step_to_connect_to_is_named_without_its_detail(cx: &mut TestAppContext) {
    let mut automation = daily();
    automation.insert(
        Step::Webhook(snob_core::automation::Webhook {
            url: "https://example.com/hook".to_string(),
            ..snob_core::automation::Webhook::default()
        }),
        (520, 0),
    );
    let app = in_editor(cx, scripted(vec![automation])).await;
    app.in_window(cx, |window, cx| window.right_click(("node", 2usize), cx));
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find(("node-menu-connect", 3usize)).label(),
            Some("Connect to Send to a webhook")
        );
    });
}

#[gpui_kit::test]
async fn two_steps_of_one_name_to_disconnect_from_are_told_apart_without_parentheses(
    cx: &mut TestAppContext,
) {
    let app = in_editor(cx, scripted(vec![two_lists()])).await;
    let labels = disconnect_labels(&app, cx, 1);
    assert_eq!(labels.len(), 2, "{labels:?}");
    assert_ne!(labels[0], labels[1]);
    for label in &labels {
        assert!(label.starts_with("Disconnect from Followers and following"));
        assert!(!label.contains('('), "{label}");
    }
}

#[gpui_kit::test]
async fn two_steps_alike_in_name_and_detail_to_disconnect_from_are_told_apart_by_their_place(
    cx: &mut TestAppContext,
) {
    let mut automation = daily();
    for y in [0, 120] {
        let hook = automation.insert(
            Step::Webhook(snob_core::automation::Webhook::default()),
            (520, y),
        );
        automation.connect(2, hook).unwrap();
    }
    let app = in_editor(cx, scripted(vec![automation])).await;
    let labels = disconnect_labels(&app, cx, 2);
    assert_eq!(labels.len(), 2, "{labels:?}");
    assert!(labels[0].ends_with(" \u{b7} 1"), "{labels:?}");
    assert!(labels[1].ends_with(" \u{b7} 2"), "{labels:?}");
    for label in &labels {
        assert!(!label.contains('('), "{label}");
    }
}

#[gpui_kit::test]
async fn a_step_to_disconnect_from_is_named_without_its_detail_in_spanish(cx: &mut TestAppContext) {
    let app = open_with(cx, scripted(vec![daily()]), spanish());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    settle(&app, cx);
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    app.click(cx, "automation-edit");
    settle(&app, cx);
    assert_eq!(
        disconnect_labels(&app, cx, 1),
        ["Desconectar de Seguidores y seguidos"] // i18n-allow
    );
}

/// The bounds of the element `id`.
fn bounds_of(
    app: &App,
    cx: &mut TestAppContext,
    id: impl Into<gpui_kit::ElementId>,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let id = id.into();
    app.in_window(cx, |window, _| window.find(id).bounds())
}

#[gpui_kit::test]
async fn a_step_takes_arrows_on_its_left_and_gives_them_on_its_right(cx: &mut TestAppContext) {
    let app = in_editor(cx, scripted(vec![daily()])).await;
    let (trigger, action) = (
        bounds_of(&app, cx, ("node", 1usize)),
        bounds_of(&app, cx, ("node", 2usize)),
    );
    let input = bounds_of(&app, cx, ("node-input", 2usize)).center();
    assert!(
        (input.x - action.left()).abs() < px(1.),
        "on the left edge: {input:?} {action:?}"
    );
    assert!(
        (input.y - action.center().y).abs() < px(1.),
        "half way down"
    );
    let output = bounds_of(&app, cx, ("node-output", 1usize)).center();
    assert!((output.x - trigger.right()).abs() < px(1.), "{output:?}");
    assert!((output.y - trigger.center().y).abs() < px(1.));
    app.in_window(cx, |window, _| {
        assert!(
            window.try_find(("node-input", 1usize)).is_none(),
            "a trigger takes no arrow"
        );
    });
}

#[gpui_kit::test]
async fn an_arrow_let_go_near_an_input_joins_the_steps_and_far_from_one_does_not(
    cx: &mut TestAppContext,
) {
    use gpui_kit::point;
    let mut open = daily();
    open.edges.clear();
    let backend = scripted(vec![open]);
    let app = in_editor(cx, backend.clone()).await;
    let handle = bounds_of(&app, cx, ("node-output", 1usize)).center();
    let input = bounds_of(&app, cx, ("node-input", 2usize)).center();
    let joined = |backend: &Scripted| backend.automations().first().map_or(0, |a| a.edges.len());

    // Let go well above the input, on no step: nothing is joined.
    drag(
        &app,
        cx,
        handle,
        &[
            handle + point(px(10.), px(0.)),
            input - point(px(0.), px(100.)),
        ],
    );
    assert_eq!(joined(&backend), 0, "far from any input");

    // Near it, then away past where it lets go: nothing either.
    drag(
        &app,
        cx,
        handle,
        &[
            input - point(px(20.), px(0.)),
            input - point(px(40.), px(60.)),
        ],
    );
    assert_eq!(joined(&backend), 0, "it let go");

    // Near it, then a little further, where it still holds: joined.
    drag(
        &app,
        cx,
        handle,
        &[
            input - point(px(20.), px(0.)),
            input - point(px(45.), px(0.)),
        ],
    );
    assert_eq!(joined(&backend), 1, "joined to the step it stuck to");
    let edge = &backend.automations()[0].edges[0];
    assert_eq!((edge.from, edge.to), (1, 2));
}

/// `daily` with its trigger's schedule `when`.
fn on_schedule(when: Spec) -> Automation {
    let mut automation = daily();
    automation.nodes[0].step = Step::Schedule { when };
    automation
}

/// The dialog of the trigger, open.
fn configure_the_trigger(app: &App, cx: &mut TestAppContext) {
    app.in_window(cx, |window, cx| window.right_click(("node", 1usize), cx));
    app.click(cx, "node-menu-configure");
    settle(app, cx);
}

fn the_trigger(backend: &Scripted) -> Step {
    backend.automations()[0].nodes[0].step.clone()
}

#[gpui_kit::test]
async fn the_schedule_dialog_says_what_is_wrong_in_the_persons_words_and_saves_what_builds(
    cx: &mut TestAppContext,
) {
    let backend = scripted(vec![on_schedule(Spec::default())]);
    let app = in_editor(cx, backend.clone()).await;
    configure_the_trigger(&app, cx);
    app.in_window(cx, |window, _| {
        let dialog = window.find("step-dialog");
        assert_eq!(dialog.role(), Some(Role::Dialog));
        assert_eq!(dialog.label(), Some("Trigger configuration"));
        assert_eq!(window.find("dialog-problems").role(), Some(Role::Status));
        assert_eq!(
            window.find(("schedule-time", 0usize)).focused(),
            Some(true),
            "a schedule that says nothing yet starts at its time"
        );
    });
    assert!(disabled(&app, cx, "dialog-save"), "no schedule is said yet");

    app.in_window(cx, |window, cx| window.input("soon", cx));
    settle(&app, cx);
    assert!(disabled(&app, cx, "dialog-save"), "not a time");
    let why = "\u{201c}soon\u{201d} is not a time of day; write it as 09:00.";
    assert_eq!(app.label_of(cx, "dialog-problems"), why);
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("dialog-save").description(), Some(why));
    });

    app.in_window(cx, |window, cx| {
        window.press("ctrl-a", cx);
        window.input("09:00", cx);
    });
    settle(&app, cx);
    assert!(!disabled(&app, cx, "dialog-save"));
    assert!(!shown(&app, cx, "dialog-problems"));
    app.click(cx, "dialog-save");
    settle(&app, cx);
    assert_eq!(
        the_trigger(&backend),
        Step::Schedule {
            when: Spec {
                at: vec!["09:00".to_string()],
                ..Spec::default()
            }
        }
    );
    assert!(!shown(&app, cx, "step-dialog"));
}

#[gpui_kit::test]
async fn a_wrong_hour_is_said_in_the_apps_language_and_save_says_why_it_is_off(
    cx: &mut TestAppContext,
) {
    let backend = scripted(vec![on_schedule(Spec::default())]);
    let app = open_with(cx, backend, spanish());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    settle(&app, cx);
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    app.click(cx, "automation-edit");
    settle(&app, cx);
    configure_the_trigger(&app, cx);
    app.in_window(cx, |window, cx| window.input("25:00", cx));
    settle(&app, cx);
    let why = "\u{ab}25:00\u{bb} no es una hora del d\u{ed}a; escr\u{ed}bela como 09:00."; // i18n-allow
    assert_eq!(app.label_of(cx, "dialog-problems"), why);
    assert!(disabled(&app, cx, "dialog-save"));
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("dialog-save").description(), Some(why));
    });
}

#[gpui_kit::test]
async fn every_month_on_the_first_monday_saves_the_days_the_engine_reads(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    let app = in_editor(cx, backend.clone()).await;
    configure_the_trigger(&app, cx);
    assert_eq!(
        app.in_window(cx, |window, _| window
            .find("schedule-frequency-day")
            .checked()),
        Some(true),
        "every six hours is a day's"
    );
    app.click(cx, "schedule-frequency-month");
    settle(&app, cx);
    app.click(cx, "schedule-by-weekday");
    settle(&app, cx);
    click(&app, cx, ("schedule-nth", 0usize));
    click(&app, cx, ("schedule-nth-weekday", 0usize));
    click(&app, cx, ("schedule-time", 0usize));
    app.in_window(cx, |window, cx| window.input("09:00", cx));
    settle(&app, cx);
    assert!(!disabled(&app, cx, "dialog-save"));
    app.click(cx, "dialog-save");
    settle(&app, cx);
    assert_eq!(
        the_trigger(&backend),
        Step::Schedule {
            when: Spec {
                at: vec!["09:00".to_string()],
                days: vec!["first mon".to_string()],
                ..Spec::default()
            }
        }
    );
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find(("node", 1usize)).label(),
            Some("Starts. On a schedule. The first Monday of the month at 09:00"),
            "the canvas says the days"
        );
    });
}

#[gpui_kit::test]
async fn every_few_months_starts_from_the_month_chosen_and_escape_closes_the_months_first(
    cx: &mut TestAppContext,
) {
    let backend = scripted(vec![daily()]);
    let app = in_editor(cx, backend.clone()).await;
    configure_the_trigger(&app, cx);
    app.click(cx, "schedule-frequency-month");
    settle(&app, cx);
    click(&app, cx, ("schedule-every-months", 3usize));
    settle(&app, cx);
    app.click(cx, "schedule-first-month");
    settle(&app, cx);
    assert!(shown(&app, cx, "schedule-first-month-options"));
    app.press(cx, "escape");
    settle(&app, cx);
    assert!(!shown(&app, cx, "schedule-first-month-options"));
    assert!(shown(&app, cx, "step-dialog"), "the dialog stays");

    app.click(cx, "schedule-first-month");
    settle(&app, cx);
    click(&app, cx, ("schedule-first-month-option", 1usize));
    settle(&app, cx);
    assert_eq!(
        app.label_of(cx, "schedule-first-month"),
        "Starting in: February"
    );
    click(&app, cx, ("schedule-time", 0usize));
    app.in_window(cx, |window, cx| window.input("08:00", cx));
    settle(&app, cx);
    app.click(cx, "dialog-save");
    settle(&app, cx);
    assert_eq!(
        the_trigger(&backend),
        Step::Schedule {
            when: Spec {
                at: vec!["08:00".to_string()],
                months: ["feb", "may", "aug", "nov"].map(str::to_string).to_vec(),
                days: vec!["1".to_string()],
                ..Spec::default()
            }
        }
    );
}

#[gpui_kit::test]
async fn a_cron_automation_opens_in_advanced_and_is_saved_unchanged(cx: &mut TestAppContext) {
    let cron = Spec {
        cron: Some("0  9 * * 1-5".to_string()),
        jitter: Some(Duration::from_secs(120)),
        ..Spec::default()
    };
    let backend = scripted(vec![on_schedule(cron.clone())]);
    let app = in_editor(cx, backend.clone()).await;
    configure_the_trigger(&app, cx);
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find("schedule-frequency-advanced").checked(),
            Some(true)
        );
        assert_eq!(
            window.find("schedule-advanced-kept").label(),
            Some("The choices cannot show this schedule, so it is kept here as it is written.")
        );
        assert_eq!(window.find("schedule-every").focused(), Some(true));
    });
    assert!(!disabled(&app, cx, "dialog-save"));
    app.click(cx, "dialog-save");
    settle(&app, cx);
    assert_eq!(the_trigger(&backend), Step::Schedule { when: cron });
}

#[gpui_kit::test]
async fn a_dialogs_focused_field_has_room_for_its_ring_on_every_side_and_stays_in_its_place(
    cx: &mut TestAppContext,
) {
    // A cron opens in Advanced, its first field across the dialog.
    let backend = scripted(vec![on_schedule(Spec {
        cron: Some("0 9 * * 1".to_string()),
        ..Spec::default()
    })]);
    let app = in_editor(cx, backend).await;
    configure_the_trigger(&app, cx);
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

/// An automation run only by hand, on the fans of somebody else, that
/// keeps what changed.
fn by_hand() -> Automation {
    let mut automation = Automation::new("a2", "Fans by hand");
    let trigger = automation.insert(Step::Manual {}, (0, 0));
    let read = automation.insert(
        Step::Read {
            account: Some(Account {
                viewer: Some(42.into()),
                username: Some("other".to_string()),
                consent: None,
            }),
            what: Read::Fans,
        },
        (260, 0),
    );
    let filter = automation.insert(
        Step::Filter {
            only: snob_core::automation::Only::Changes,
        },
        (520, 0),
    );
    automation.connect(trigger, read).unwrap();
    automation.connect(read, filter).unwrap();
    automation
}

/// What a monitor's run found, as the engine keeps it: two came and one
/// went of 120 followers, and the 98 followed did not change.
fn found_lists() -> String {
    use snob_engine::automations::said::{ListSaid, Said};
    let names = |names: &[&str]| names.iter().map(|name| (*name).to_string()).collect();
    let said = Said::Tick {
        account: Some("me".to_string()),
        pk: 42.into(),
        own: true,
        refused: false,
        nothing_stored: false,
        followers: Some(ListSaid {
            basis: "compare".to_string(),
            total: 120,
            added: 2,
            removed: 1,
            added_names: names(&["new_friend", "late_comer"]),
            removed_names: names(&["gone_away"]),
        }),
        following: Some(ListSaid {
            basis: "unchanged".to_string(),
            total: 98,
            added: 0,
            removed: 0,
            added_names: Vec::new(),
            removed_names: Vec::new(),
        }),
        renamed: 0,
        posts: None,
    };
    serde_json::to_string(&[said]).unwrap()
}

/// 2026-09-21 at 14:13:20 UTC, and an hour later.
const STARTED: i64 = 1_790_000_000;
const AN_HOUR_LATER: i64 = STARTED + 3600;

fn going(automation: &str, at: i64) -> Run {
    Run {
        summary: String::new(),
        running: true,
        ..run_of(automation, at, None)
    }
}

/// How many times the history of `id` was read.
fn reads_of(backend: &Scripted, id: &str) -> usize {
    backend
        .asked()
        .iter()
        .filter(|asked| **asked == Asked::AutomationRuns(id.to_string()))
        .count()
}

/// The lines under the run at `at` of the history, as they are read.
fn lines_of(app: &App, cx: &mut TestAppContext, at: usize) -> Vec<String> {
    let name = gpui_kit::SharedString::from(format!("automation-run-{at}-said"));
    app.in_window(cx, |window, _| {
        (0..)
            .map_while(|line| window.try_find((name.clone(), line)))
            .map(|found| found.label().unwrap_or_default().trim().to_string())
            .collect()
    })
}

fn spanish() -> Settings {
    Settings {
        locale: "es".to_string(),
        ..still()
    }
}

#[gpui_kit::test]
async fn the_list_is_a_grid_of_cards_each_with_its_name_its_steps_and_when_it_runs(
    cx: &mut TestAppContext,
) {
    let app = in_list(cx, scripted(vec![daily(), by_hand()])).await;
    app.in_window(cx, |window, _| {
        let first = window.find(("automation", 0usize));
        let second = window.find(("automation", 1usize));
        assert_eq!(first.role(), Some(Role::Button));
        assert_eq!(
            first.label(),
            Some("Daily lists. Followers and following (Your account). Every 6 hours")
        );
        assert_eq!(
            second.label(),
            Some("Fans by hand. Fans (@other) \u{2192} Only what changed. By hand")
        );
        let (one, two) = (first.bounds(), second.bounds());
        assert_eq!(one.origin.y, two.origin.y, "side by side");
        assert!(two.origin.x > one.right());
        assert_eq!(one.size.height, two.size.height, "a row is as tall");

        let search = window.find("automations-search").bounds();
        let create = window.find("automations-create").bounds();
        assert!(
            (create.size.height - search.size.height).abs() <= px(1.),
            "Create is {:?} tall, the search {:?}",
            create.size.height,
            search.size.height
        );
        assert!(create.size.width < px(120.), "as wide as its label");
    });
}

#[gpui_kit::test]
async fn a_card_says_its_steps_and_when_it_runs_in_the_apps_language(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily(), by_hand()]);
    let app = open_with(cx, backend, spanish());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find(("automation", 0usize)).label(),
            Some("Daily lists. Seguidores y seguidos (Tu cuenta). Cada 6 horas") // i18n-allow
        );
        assert_eq!(
            window.find(("automation", 1usize)).label(),
            Some("Fans by hand. Fans (@other) \u{2192} Solo lo que cambi\u{f3}. A mano") // i18n-allow
        );
    });
}

#[gpui_kit::test]
async fn a_card_says_the_days_and_months_a_schedule_runs_on(cx: &mut TestAppContext) {
    let monthly = on_schedule(Spec {
        days: vec!["1".to_string(), "15".to_string()],
        months: vec!["mar".to_string(), "sep".to_string()],
        at: vec!["08:00".to_string()],
        ..Spec::default()
    });
    let app = in_list(cx, scripted(vec![monthly])).await;
    assert_eq!(
        app.in_window(cx, |window, _| window
            .find(("automation", 0usize))
            .label()
            .map(str::to_string)),
        Some(
            "Daily lists. Followers and following (Your account). \
             On the 1st and 15th of March and September at 08:00"
                .to_string()
        )
    );
}

#[gpui_kit::test]
async fn two_cards_sit_one_under_the_other_when_the_text_is_large(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily(), by_hand()]);
    let large = Settings {
        text_scale: 2.0,
        ..still()
    };
    let app = open_with(cx, backend, large);
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        let one = window.find(("automation", 0usize)).bounds();
        let two = window.find(("automation", 1usize)).bounds();
        assert!(two.origin.y > one.bottom(), "one under the other");
        assert_eq!(one.origin.x, two.origin.x);
    });
}

#[gpui_kit::test]
async fn a_run_going_says_running_and_is_read_again_until_it_ends(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    backend.script(|script| {
        script.run(going("a1", STARTED));
    });
    let app = in_list(cx, backend.clone()).await;
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        let row = window.find(("automation-run-row", 0usize));
        let said = row.label().unwrap_or_default();
        assert!(said.ends_with(". Running"), "{said}");
        assert!(!said.contains("Requests"), "{said}");
    });

    // Read again every two seconds while it goes.
    let before = reads_of(&backend, "a1");
    for _ in 0..6 {
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
    }
    assert!(
        reads_of(&backend, "a1") >= before + 5,
        "{} reads in 12 s",
        reads_of(&backend, "a1") - before
    );

    // It ends: said once read again, and not read any more.
    let found = found_lists();
    backend.script(|script| {
        script.finish_run(STARTED, "ok", Some(&found));
    });
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    app.in_window(cx, |window, _| {
        let said = window
            .find(("automation-run-row", 0usize))
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(said.contains("Finished. Requests: 12"), "{said}");
    });
    assert_eq!(
        lines_of(&app, cx, 0).first().map(String::as_str),
        Some("Followers \u{b7} Your account: 2 came, 1 went, 120 accounts in all.")
    );
    let ended = reads_of(&backend, "a1");
    cx.executor().advance_clock(Duration::from_secs(10));
    cx.run_until_parked();
    assert_eq!(reads_of(&backend, "a1"), ended, "nothing goes on: no read");
}

#[gpui_kit::test]
async fn a_page_left_while_a_run_goes_reads_its_runs_no_more(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    backend.script(|script| {
        script.run(going("a1", STARTED));
    });
    let app = in_list(cx, backend.clone()).await;
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    app.press(cx, "escape");
    settle(&app, cx);
    assert!(shown(&app, cx, "automations-create"), "on the list");
    let left = reads_of(&backend, "a1");
    cx.executor().advance_clock(Duration::from_secs(10));
    cx.run_until_parked();
    assert_eq!(reads_of(&backend, "a1"), left);
}

#[gpui_kit::test]
async fn run_now_says_it_started_until_its_run_shows_up_in_the_history(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    backend.script(|script| {
        script.run(run_of("a1", STARTED - 3600, Some("ok")));
    });
    let app = in_list(cx, backend.clone()).await;
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    backend.script(|script| {
        script.starts_run(going("a1", STARTED));
    });
    app.click(cx, "automation-run");
    cx.run_until_parked();
    app.in_window(cx, |_, _| ());
    cx.run_until_parked();
    assert!(
        !shown(&app, cx, "automation-started"),
        "it is there already"
    );
    app.in_window(cx, |window, _| {
        let said = window
            .find(("automation-run-row", 0usize))
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(said.ends_with(". Running"), "{said}");
    });
}

#[gpui_kit::test]
async fn a_run_that_never_shows_up_after_run_now_is_said_to_have_not(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    let app = in_list(cx, backend.clone()).await;
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    app.click(cx, "automation-run");
    cx.run_until_parked();
    assert!(shown(&app, cx, "automation-started"));
    let before = reads_of(&backend, "a1");
    for _ in 0..16 {
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
    }
    app.in_window(cx, |_, _| ());
    assert!(reads_of(&backend, "a1") > before, "it was looked for");
    assert!(!shown(&app, cx, "automation-started"));
    assert!(shown(&app, cx, "automation-not-seen"));
    let given_up = reads_of(&backend, "a1");
    cx.executor().advance_clock(Duration::from_secs(10));
    cx.run_until_parked();
    assert_eq!(reads_of(&backend, "a1"), given_up, "and no longer");
}

/// The page of `daily`, told the moment it is an hour after [`STARTED`].
async fn on_the_page_an_hour_later(
    cx: &mut TestAppContext,
    backend: Scripted,
    settings: Settings,
) -> App {
    let app = open_with(cx, backend, settings);
    cx.update(|cx| snob_desktop::ui::now::fix_now(Epoch::new(AN_HOUR_LATER), cx));
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    settle(&app, cx);
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    app
}

fn found_and_kept_before() -> Scripted {
    let backend = scripted(vec![daily()]);
    let found = found_lists();
    backend.script(|script| {
        script.run(Run {
            found: Some(found),
            ..run_of("a1", STARTED, Some("ok"))
        });
        // Eleven days before.
        script.run(run_of("a1", 1_789_000_000, Some("ok")));
    });
    backend
}

#[gpui_kit::test]
async fn a_run_with_what_it_found_kept_is_said_in_the_apps_words_and_one_kept_before_in_its_own(
    cx: &mut TestAppContext,
) {
    let app = on_the_page_an_hour_later(cx, found_and_kept_before(), still()).await;
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find(("automation-run-row", 0usize)).label(),
            Some("Today at 14:13. Finished. Requests: 12")
        );
        assert_eq!(
            window.find(("automation-run-row", 1usize)).label(),
            Some("September 10 at 00:26. Finished. Requests: 12")
        );
    });
    assert_eq!(
        lines_of(&app, cx, 0),
        [
            "Followers \u{b7} Your account: 2 came, 1 went, 120 accounts in all.",
            "+ @new_friend",
            "+ @late_comer",
            "\u{2212} @gone_away",
            "Following \u{b7} Your account: no changes, 98 accounts.",
        ]
    );
    assert_eq!(
        lines_of(&app, cx, 1),
        ["Read 120 followers.", "Nobody left."]
    );
}

#[gpui_kit::test]
async fn what_a_run_found_and_when_are_said_in_spanish_too(cx: &mut TestAppContext) {
    let app = on_the_page_an_hour_later(cx, found_and_kept_before(), spanish()).await;
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find(("automation-run-row", 0usize)).label(),
            Some("Hoy a las 14:13. Termin\u{f3}. Peticiones: 12") // i18n-allow
        );
    });
    assert_eq!(
        lines_of(&app, cx, 0).first().map(String::as_str),
        Some(
            "Seguidores \u{b7} Tu cuenta: entraron 2 y salieron 1; 120 cuentas en total." // i18n-allow
        )
    );
}

#[gpui_kit::test]
async fn a_test_build_told_the_moment_it_is_reads_no_run_again_by_itself(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    backend.script(|script| {
        script.run(going("a1", STARTED));
    });
    let app = on_the_page_an_hour_later(cx, backend.clone(), still()).await;
    app.in_window(cx, |window, _| {
        let said = window
            .find(("automation-run-row", 0usize))
            .label()
            .unwrap_or_default()
            .to_string();
        assert_eq!(said, "Today at 14:13. Running");
    });
    let shown = reads_of(&backend, "a1");
    cx.executor().advance_clock(Duration::from_secs(10));
    cx.run_until_parked();
    assert_eq!(reads_of(&backend, "a1"), shown, "held still for a picture");
}

#[gpui_kit::test]
async fn the_pencil_is_a_button_as_tall_as_run_now(cx: &mut TestAppContext) {
    let app = in_list(cx, scripted(vec![daily()])).await;
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        let pencil = window.find("automation-edit");
        assert_eq!(pencil.role(), Some(Role::Button));
        let run = window.find("automation-run").bounds();
        assert!(
            (pencil.bounds().size.height - run.size.height).abs() <= px(1.),
            "as tall as Run now"
        );
    });
}

/// The page of `daily` with a run going, left by the person until the app
/// rests, with what it read by then.
async fn resting_on_a_run_going(cx: &mut TestAppContext, backend: &Scripted) -> (App, usize) {
    backend.script(|script| {
        script.run(going("a1", STARTED));
    });
    let app = in_list(cx, backend.clone()).await;
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    assert!(first_run_said(&app, cx).ends_with(". Running"));
    VisualTestContext::from_window(app.window, cx).deactivate_window();
    cx.executor().advance_clock(QUIET_FOR * 2);
    cx.run_until_parked();
    assert!(app.system.is_resting());
    let rested = reads_of(backend, "a1");
    cx.executor().advance_clock(Duration::from_secs(10));
    cx.run_until_parked();
    assert_eq!(reads_of(backend, "a1"), rested, "no read at rest");
    let found = found_lists();
    backend.script(|script| {
        script.finish_run(STARTED, "ok", Some(&found));
    });
    (app, rested)
}

fn first_run_said(app: &App, cx: &mut TestAppContext) -> String {
    app.in_window(cx, |window, _| {
        window
            .find(("automation-run-row", 0usize))
            .label()
            .unwrap_or_default()
            .to_string()
    })
}

#[gpui_kit::test]
async fn a_resting_app_reads_no_run_and_a_frame_that_wakes_it_reads_on(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    let (app, rested) = resting_on_a_run_going(cx, &backend).await;

    // A frame drawn, the window still not looked at.
    app.in_window(cx, |_, _| ());
    assert!(!app.system.is_resting());
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert!(reads_of(&backend, "a1") > rested, "read once awake");
    let said = first_run_said(&app, cx);
    assert!(said.contains("Finished"), "{said}");
}

#[gpui_kit::test]
async fn a_resting_app_looked_at_again_reads_its_runs_at_once(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    let (app, rested) = resting_on_a_run_going(cx, &backend).await;
    cx.update_window(app.window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    assert!(!app.system.is_resting());
    assert!(reads_of(&backend, "a1") > rested, "read once looked at");
    let said = first_run_said(&app, cx);
    assert!(said.contains("Finished"), "{said}");
}

/// The dialog of the step `node`, open.
fn configure(app: &App, cx: &mut TestAppContext, node: usize) {
    app.in_window(cx, |window, cx| window.right_click(("node", node), cx));
    app.click(cx, "node-menu-configure");
    settle(app, cx);
}

/// What was asked of the accounts known, in order.
fn asked_known(backend: &Scripted) -> Vec<String> {
    backend
        .asked()
        .into_iter()
        .filter_map(|asked| match asked {
            Asked::KnownAccounts(typed) => Some(typed),
            _ => None,
        })
        .collect()
}

fn label_at(app: &App, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) -> String {
    let id = id.into();
    app.in_window(cx, |window, _| {
        window.find(id).label().unwrap_or_default().to_string()
    })
}

fn focused_at(app: &App, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) -> bool {
    let id = id.into();
    app.in_window(cx, |window, _| window.find(id).focused() == Some(true))
}

/// `daily` with its action reading `username`'s lists, the yes given at
/// `consent`.
fn on_another(username: &str, consent: Option<i64>) -> Automation {
    let mut automation = daily();
    automation.nodes[1].step = Step::Read {
        account: Some(Account {
            username: Some(username.to_string()),
            viewer: Some(42.into()),
            consent: consent.map(Epoch::new),
        }),
        what: Read::Lists,
    };
    automation
}

#[gpui_kit::test]
async fn typing_two_letters_suggests_the_known_accounts_with_another_person_last_and_enter_takes_one(
    cx: &mut TestAppContext,
) {
    let backend = scripted(vec![daily()]);
    backend.script(|script| {
        script.known(vec![
            account(7, "anna", false),
            account(8, "annabel", false),
            account(9, "bob", false),
        ]);
    });
    let app = in_editor(cx, backend.clone()).await;
    configure(&app, cx, 2);
    app.click(cx, "action-other");
    settle(&app, cx);
    click(&app, cx, "action-username");
    settle(&app, cx);
    assert!(
        asked_known(&backend).is_empty(),
        "nothing typed, nothing asked"
    );
    assert!(!shown(&app, cx, "action-suggestions"));
    let before = backend.asked().len();

    app.in_window(cx, |window, cx| window.input("an", cx));
    settle(&app, cx);
    assert_eq!(asked_known(&backend), ["an"], "one ask for the keys typed");
    assert!(shown(&app, cx, "action-suggestions"));
    let rows: Vec<String> = (0..3usize)
        .map(|at| label_at(&app, cx, ("action-suggestion", at)))
        .collect();
    assert_eq!(
        rows,
        [
            "@anna, Invented anna",
            "@annabel, Invented annabel",
            "Another person @an",
        ]
    );
    assert!(
        app.in_window(cx, |window, _| window
            .try_find(("action-suggestion", 3usize))
            .is_none()),
        "bob does not begin with what is typed"
    );

    app.press(cx, "down");
    assert!(focused_at(&app, cx, ("action-suggestion", 0usize)));
    app.press(cx, "down");
    assert!(focused_at(&app, cx, ("action-suggestion", 1usize)));
    app.press(cx, "enter");
    settle(&app, cx);
    assert!(!shown(&app, cx, "action-suggestions"));
    assert!(focused(&app, cx, "action-username"), "back in the field");
    assert_eq!(
        app.label_of(cx, "action-consent"),
        "I agree that this automation reads the followers and the following of @annabel each \
         time it runs."
    );
    assert_eq!(
        backend.asked()[before..],
        [Asked::KnownAccounts("an".to_string())],
        "choosing a name sends nothing"
    );
}

#[gpui_kit::test]
async fn a_click_on_a_suggestion_fills_the_field_and_closes_the_list(cx: &mut TestAppContext) {
    let backend = scripted(vec![daily()]);
    backend.script(|script| {
        script.known(vec![
            account(7, "anna", false),
            account(8, "annabel", false),
        ]);
    });
    let app = in_editor(cx, backend).await;
    configure(&app, cx, 2);
    app.click(cx, "action-other");
    settle(&app, cx);
    click(&app, cx, "action-username");
    app.in_window(cx, |window, cx| window.input("an", cx));
    settle(&app, cx);
    assert!(shown(&app, cx, "action-suggestions"));
    click(&app, cx, ("action-suggestion", 1usize));
    settle(&app, cx);
    assert!(!shown(&app, cx, "action-suggestions"));
    assert_eq!(
        app.in_window(cx, |window, _| window
            .find("action-username")
            .value()
            .map(str::to_string)),
        Some("annabel".to_string())
    );
}

#[gpui_kit::test]
async fn escape_closes_the_suggestions_before_the_dialog(cx: &mut TestAppContext) {
    let backend = scripted(vec![on_another("someone", None)]);
    let app = in_editor(cx, backend).await;
    configure(&app, cx, 2);
    click(&app, cx, "action-username");
    app.in_window(cx, |window, cx| window.input("x", cx));
    settle(&app, cx);
    assert_eq!(
        label_at(&app, cx, ("action-suggestion", 0usize)),
        "Another person @someonex"
    );
    app.press(cx, "escape");
    settle(&app, cx);
    assert!(!shown(&app, cx, "action-suggestions"));
    assert!(shown(&app, cx, "step-dialog"), "the dialog stays");
    app.press(cx, "escape");
    settle(&app, cx);
    assert!(!shown(&app, cx, "step-dialog"));
}

#[gpui_kit::test]
async fn changing_the_account_unticks_the_yes_and_asks_it_again_for_the_new_name(
    cx: &mut TestAppContext,
) {
    let backend = scripted(vec![on_another("anna", Some(100))]);
    let app = in_editor(cx, backend.clone()).await;
    configure(&app, cx, 2);
    let ticked = |app: &App, cx: &mut TestAppContext| {
        app.in_window(cx, |window, _| window.find("action-consent").checked())
    };
    assert_eq!(ticked(&app, cx), Some(true), "the yes kept with anna");
    assert!(app.label_of(cx, "action-consent").contains("@anna "));

    click(&app, cx, "action-username");
    app.in_window(cx, |window, cx| {
        window.press("ctrl-a", cx);
        window.input("bob", cx);
    });
    settle(&app, cx);
    assert_eq!(ticked(&app, cx), Some(false));
    assert!(app.label_of(cx, "action-consent").contains("@bob "));
    assert!(
        disabled(&app, cx, "dialog-save"),
        "bob's lists wait for a yes"
    );

    app.in_window(cx, |window, cx| {
        window.press("ctrl-a", cx);
        window.input("anna", cx);
    });
    settle(&app, cx);
    assert_eq!(
        ticked(&app, cx),
        Some(false),
        "a yes taken back is not given again by typing the name"
    );

    app.press(cx, "tab");
    settle(&app, cx);
    assert!(
        !shown(&app, cx, "action-suggestions"),
        "the names suggested go once the field is left, and lie over nothing"
    );
    assert!(focused(&app, cx, "action-consent"));
    app.click(cx, "action-consent");
    settle(&app, cx);
    assert_eq!(ticked(&app, cx), Some(true));
    app.click(cx, "dialog-save");
    settle(&app, cx);
    let Step::Read {
        account: Some(saved),
        ..
    } = backend.automations()[0].nodes[1].step.clone()
    else {
        panic!("an action");
    };
    assert_eq!(saved.username.as_deref(), Some("anna"));
    assert!(saved.consent.is_some());
}

/// An automation with a step of every kind that has a dialog: a schedule,
/// an action, a filter and a webhook with two headers.
fn every_kind() -> Automation {
    let mut automation = daily();
    automation.insert(
        Step::Filter {
            only: snob_core::automation::Only::Verified,
        },
        (520, 0),
    );
    automation.insert(
        Step::Webhook(snob_core::automation::Webhook {
            url: "https://example.com/hook".to_string(),
            headers: [("X-One", "1"), ("X-Two", "2")]
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .into(),
            ..Default::default()
        }),
        (780, 0),
    );
    automation
}

#[gpui_kit::test]
async fn the_headers_of_a_webhook_are_numbered_and_the_keyrings_secrets_are_said_to_stay_out(
    cx: &mut TestAppContext,
) {
    let app = in_editor(cx, scripted(vec![every_kind()])).await;
    configure(&app, cx, 4);
    assert_eq!(
        [
            label_at(&app, cx, ("webhook-header-name", 0usize)),
            label_at(&app, cx, ("webhook-header-value", 0usize)),
            label_at(&app, cx, ("webhook-header-name", 1usize)),
            label_at(&app, cx, ("webhook-header-remove", 1usize)),
        ],
        [
            "Name of header 1",
            "Value of header 1",
            "Name of header 2",
            "Remove header 2",
        ]
    );
    let said = |app: &App, cx: &mut TestAppContext| {
        app.in_window(cx, |window, _| {
            window.try_find("webhook-keyring-note").is_some()
        })
    };
    assert!(said(&app, cx), "an automation made here");

    let mut watched = every_kind();
    watched.id = snob_core::automation::WATCH_TOML.to_string();
    let app = in_editor(cx, scripted(vec![watched])).await;
    configure(&app, cx, 4);
    assert!(!said(&app, cx), "watch.toml's webhook is given them");
}

#[gpui_kit::test]
async fn the_headers_of_a_webhook_are_numbered_in_spanish_too(cx: &mut TestAppContext) {
    let app = open_with(cx, scripted(vec![every_kind()]), spanish());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    settle(&app, cx);
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    app.click(cx, "automation-edit");
    settle(&app, cx);
    configure(&app, cx, 4);
    assert_eq!(
        [
            label_at(&app, cx, ("webhook-header-name", 1usize)),
            label_at(&app, cx, ("webhook-header-value", 1usize)),
            label_at(&app, cx, ("webhook-header-remove", 0usize)),
        ],
        [
            "Nombre de la cabecera 2", // i18n-allow
            "Valor de la cabecera 2",  // i18n-allow
            "Quitar la cabecera 1",    // i18n-allow
        ]
    );
}

#[gpui_kit::test]
async fn every_steps_dialog_has_one_title_and_tab_reaches_its_controls(cx: &mut TestAppContext) {
    let app = in_editor(cx, scripted(vec![every_kind()])).await;
    let dialogs: [(usize, &str, &[&'static str]); 4] = [
        (
            1,
            "Trigger configuration",
            &["schedule-frequency-day", "schedule-count", "dialog-cancel"],
        ),
        (
            2,
            "Configure: Followers and following",
            &[
                "action-mine",
                "action-other",
                "dialog-cancel",
                "dialog-save",
            ],
        ),
        (
            3,
            "Configure: Verified accounts",
            &["filter-verified", "dialog-cancel", "dialog-save"],
        ),
        (
            4,
            "Configure: Send to a webhook",
            &[
                "webhook-url",
                "webhook-heartbeat",
                "webhook-header-add",
                "dialog-cancel",
                "dialog-save",
            ],
        ),
    ];
    for (node, title, controls) in dialogs {
        configure(&app, cx, node);
        app.in_window(cx, |window, _| {
            let dialog = gpui_kit::ElementId::from("step-dialog");
            let titles: Vec<String> = window
                .elements()
                .iter()
                .filter(|element| {
                    element.role() == Some(Role::Heading) && element.path().contains(&dialog)
                })
                .filter_map(|element| element.label().map(str::to_string))
                .collect();
            // The webhook's headers are a section of its body, a level below.
            let headings: &[&str] = if node == 4 {
                &[title, "Headers"]
            } else {
                &[title]
            };
            assert_eq!(titles, headings, "node {node}");
            assert_eq!(window.find("step-dialog").label(), Some(title));
        });
        let mut wanted = controls.to_vec();
        for _ in 0..60 {
            app.press(cx, "tab");
            wanted.retain(|id| !focused(&app, cx, id));
            if wanted.is_empty() {
                break;
            }
        }
        assert!(
            wanted.is_empty(),
            "Tab never comes to {wanted:?} in {title}"
        );
        app.press(cx, "escape");
        settle(&app, cx);
        assert!(!shown(&app, cx, "step-dialog"));
    }
}

#[gpui_kit::test]
async fn a_dialog_taller_than_the_window_opens_with_its_focused_field_in_sight_ring_and_all(
    cx: &mut TestAppContext,
) {
    let large = Settings {
        text_scale: 2.0,
        ..still()
    };
    let app = open_with(cx, scripted(vec![daily()]), large);
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    settle(&app, cx);
    click(&app, cx, ("automation", 0usize));
    settle(&app, cx);
    app.click(cx, "automation-edit");
    settle(&app, cx);
    configure(&app, cx, 1);
    assert!(focused_at(&app, cx, "schedule-count"));
    app.in_window(cx, |window, _| {
        let body = window.find(("step-dialog", 2usize)).bounds();
        let field = window.find("schedule-count").bounds();
        assert!(
            field.top() >= body.top() && field.bottom() <= body.bottom() - px(4.),
            "{field:?} whole, its ring too, in {body:?}"
        );
    });
}
