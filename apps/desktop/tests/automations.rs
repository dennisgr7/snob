//! The automations section, driven in process: its introduction, shown once
//! an account and centered on the window, the list and its search, and the
//! editor's canvas with its zoom and its steps. What is checked is what a
//! person or a screen reader would find, by name and by role.

#![cfg(feature = "testing")]

mod common;

use std::time::Duration;

use common::{App, disabled, focused, open_with};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Modifiers, PlatformInput, Role, ScrollDelta, ScrollEvent, TestAppContext,
    VisualTestContext, point, px,
};
use snob_desktop::backend::scripted::{Asked, Scripted, account};
use snob_desktop::state::flow::Screen;
use snob_desktop::state::rest::QUIET_FOR;
use snob_desktop::system::Settings;

/// Settings with no motion: what a stage shows is there at once.
fn still() -> Settings {
    Settings {
        reduce_motion: true,
        ..Settings::default()
    }
}

/// The app open on the automations section, its first stage drawn.
async fn in_automations(cx: &mut TestAppContext, backend: Scripted, settings: Settings) -> App {
    let app = open_with(cx, backend, settings);
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    cx.run_until_parked();
    app
}

fn shown(app: &App, cx: &mut TestAppContext, id: &'static str) -> bool {
    app.in_window(cx, |window, _| {
        window.try_find(id).is_some_and(|found| found.visible())
    })
}

/// The middle of `id` across the window.
fn middle_of(app: &App, cx: &mut TestAppContext, id: &'static str) -> f32 {
    app.in_window(cx, |window, _| {
        let bounds = window.find(id).bounds();
        f32::from(bounds.origin.x + bounds.size.width / 2.0)
    })
}

/// Lets the moves under way end.
fn settle(app: &App, cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    app.in_window(cx, |_, _| ());
}

#[gpui_kit::test]
async fn the_introduction_is_centered_on_the_window_under_the_brand(cx: &mut TestAppContext) {
    let app = in_automations(cx, Scripted::with(vec![account(42, "me", true)]), still()).await;
    assert_eq!(app.title(cx), ("Automations".to_string(), true));

    // On the window's middle, where the brand is, not on the panel's, which
    // the rail pushes right.
    let (title, brand) = (middle_of(&app, cx, "title"), middle_of(&app, cx, "brand"));
    assert!(
        (title - brand).abs() < 1.0,
        "the title at {title}, the brand at {brand}"
    );
    let panel = app.in_window(cx, |window, _| {
        let bounds = window.find("panel").bounds();
        f32::from(bounds.origin.x + bounds.size.width / 2.0)
    });
    assert!(panel - title > 20.0, "not the panel's middle, {panel}");
    assert!((middle_of(&app, cx, "automations-begin") - brand).abs() < 1.0);

    app.in_window(cx, |window, _| {
        let begin = window.find("automations-begin");
        assert_eq!(begin.role(), Some(Role::Button));
        assert_eq!(begin.label(), Some("Get started"));
        assert!(
            window.try_find("automations-search").is_none(),
            "no list yet"
        );
    });
}

#[gpui_kit::test]
async fn get_started_is_remembered_for_the_account_and_the_introduction_never_comes_back(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(vec![
        account(42, "me", true),
        account(9001, "someone", false),
    ]);
    let app = in_automations(cx, backend.clone(), still()).await;
    app.click(cx, "automations-begin");
    settle(&app, cx);

    assert!(
        backend
            .asked()
            .contains(&Asked::RememberIntroduced(42.into()))
    );
    assert_eq!(
        app.title(cx),
        ("Automations".to_string(), true),
        "focus on it"
    );
    assert!(shown(&app, cx, "automations-search"));
    assert!(shown(&app, cx, "automations-create"));
    assert!(!shown(&app, cx, "automations-begin"));

    // Away and back: straight to the list.
    app.click(cx, "rail-home");
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    settle(&app, cx);
    assert!(!shown(&app, cx, "automations-begin"));
    assert!(shown(&app, cx, "automations-create"));

    // The other account is owed it in its turn.
    app.click(cx, "account");
    app.click(cx, "menu-switch");
    app.in_window(cx, |window, cx| window.click(("menu-account", 0usize), cx));
    let shell = app.shell.clone();
    app.until(cx, move |_, cx| {
        shell
            .read(cx)
            .active()
            .is_some_and(|active| active.named.pk == 9001.into())
    })
    .await;
    settle(&app, cx);
    app.click(cx, "rail-automations");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    settle(&app, cx);
    assert!(shown(&app, cx, "automations-begin"), "each account its own");
}

#[gpui_kit::test]
async fn an_account_introduced_before_a_restart_goes_straight_to_the_list(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.introduced(42);
    });
    let app = in_automations(cx, backend.clone(), still()).await;
    assert!(!shown(&app, cx, "automations-begin"));
    assert!(shown(&app, cx, "automations-create"));
    assert!(
        !backend
            .asked()
            .iter()
            .any(|asked| matches!(asked, Asked::RememberIntroduced(_))),
        "nothing to remember again"
    );
}

#[gpui_kit::test]
async fn the_list_has_its_title_at_the_top_left_and_says_what_a_search_did_not_find(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.introduced(42);
    });
    let app = in_automations(cx, backend, still()).await;
    app.in_window(cx, |window, _| {
        let panel = window.find("panel").bounds();
        let title = window.find("title").bounds();
        let search = window.find("automations-search").bounds();
        let create = window.find("automations-create").bounds();
        assert!(title.origin.x - panel.origin.x < px(40.), "at the left");
        assert!(title.origin.y - panel.origin.y < px(40.), "at the top");
        assert!(title.right() < search.origin.x && search.right() < create.origin.x);
        assert!(create.right() > panel.right() - px(40.), "at the right");
        assert_eq!(window.find("automations-create").label(), Some("Create"));
        let empty = window.find("automations-empty");
        assert_eq!(empty.role(), Some(Role::Heading));
        assert_eq!(empty.label(), Some("No automations yet"));
    });

    app.click(cx, "automations-search");
    app.in_window(cx, |window, cx| window.input("daily report", cx));
    cx.run_until_parked();
    app.in_window(cx, |window, _| {
        assert!(window.try_find("automations-empty").is_none());
        let none = window.find("automations-none");
        assert_eq!(none.role(), Some(Role::Status));
        assert_eq!(none.label(), Some("No automation is called “daily report”"));
    });
}

#[gpui_kit::test]
async fn create_opens_a_canvas_whose_zoom_has_bounds_and_escape_comes_back(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.introduced(42);
    });
    let app = in_automations(cx, backend, still()).await;
    app.click(cx, "automations-create");
    settle(&app, cx);
    assert_eq!(app.title(cx), ("Edit automation".to_string(), true));

    let zoom = |app: &App, cx: &mut TestAppContext| app.label_of(cx, "editor-zoom-reset");
    assert_eq!(zoom(&app, cx), "Zoom at 100 %. Back to 100 %");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("editor-canvas").role(), Some(Role::Group));
        assert_eq!(window.find("editor-zoom").role(), Some(Role::Toolbar));
        for id in [
            "editor-back",
            "editor-undo",
            "editor-redo",
            "editor-zoom-out",
            "editor-zoom-reset",
            "editor-zoom-in",
            "editor-fit",
            "editor-shelf-search",
        ] {
            let control = window.find(id);
            assert_eq!(control.role(), Some(Role::Button), "{id}");
            assert!(!control.label().unwrap_or_default().is_empty(), "{id}");
            let size = control.bounds().size;
            assert!(size.width >= px(24.) && size.height >= px(24.), "{id}");
        }
        // Held at the triggers, as there is none yet: no way back to the
        // kinds, and a word on why.
        assert_eq!(window.find("editor-steps-title").label(), Some("Triggers"));
        assert!(window.try_find("editor-shelf-back").is_none());
        assert!(window.try_find("editor-triggers").is_none());
        assert_eq!(
            window.find("editor-shelf-first").label(),
            Some("Every automation starts with a trigger.")
        );
        let schedule = window.find(("trigger-schedule", 0usize));
        assert_eq!(schedule.role(), Some(Role::Button));
        assert_eq!(schedule.label(), Some("Add: On a schedule"));
        assert_eq!(
            window.find(("trigger-manual", 1usize)).label(),
            Some("Add: Manually")
        );
        let soon = window.find(("trigger-webhook", 2usize));
        assert_eq!(soon.disabled(), Some(true), "nothing runs behind it");
    });

    app.click(cx, "editor-zoom-in");
    settle(&app, cx);
    assert_eq!(zoom(&app, cx), "Zoom at 110 %. Back to 100 %");
    for _ in 0..12 {
        app.click(cx, "editor-zoom-out");
        settle(&app, cx);
    }
    assert_eq!(zoom(&app, cx), "Zoom at 25 %. Back to 100 %");
    assert!(
        disabled(&app, cx, "editor-zoom-out"),
        "no further than that"
    );
    app.click(cx, "editor-zoom-reset");
    settle(&app, cx);
    assert_eq!(zoom(&app, cx), "Zoom at 100 %. Back to 100 %");

    app.press(cx, "escape");
    settle(&app, cx);
    assert_eq!(app.title(cx), ("Automations".to_string(), true));
    assert!(shown(&app, cx, "automations-create"));
}

#[gpui_kit::test]
async fn the_panel_goes_into_a_kind_and_back_once_there_is_a_trigger(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.introduced(42);
    });
    let app = in_automations(cx, backend, still()).await;
    app.click(cx, "automations-create");
    settle(&app, cx);
    app.in_window(cx, |window, cx| {
        window.click(("trigger-manual", 1usize), cx)
    });
    settle(&app, cx);

    // The first trigger lets the panel go: the four kinds, each a way in.
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find("editor-steps-title").label(),
            Some("Add a step")
        );
        for (id, label) in [
            ("editor-triggers", "Triggers"),
            ("editor-actions", "Actions"),
            ("editor-filters", "Filters"),
            ("editor-outputs", "Outputs"),
        ] {
            let kind = window.find(id);
            assert_eq!(kind.role(), Some(Role::Button), "{id}");
            assert_eq!(kind.label(), Some(label));
        }
    });
    assert!(
        focused(&app, cx, "editor-triggers"),
        "focus on the first kind"
    );

    app.click(cx, "editor-outputs");
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("editor-steps-title").label(), Some("Outputs"));
        let soon = window.find(("output-api", 1usize));
        assert_eq!(soon.label(), Some("Call an API, coming soon"));
        assert_eq!(soon.disabled(), Some(true), "nothing runs behind it");
    });
    assert!(focused(&app, cx, "editor-shelf-back"));
    app.click(cx, "editor-shelf-back");
    settle(&app, cx);
    assert!(shown(&app, cx, "editor-actions"));
    assert!(
        focused(&app, cx, "editor-outputs"),
        "focus on the kind left"
    );

    // The magnifier opens a search in the kind shown; Escape closes it, and
    // then goes back to the kinds before it leaves the editor.
    app.click(cx, "editor-actions");
    settle(&app, cx);
    let title = app.in_window(cx, |window, _| window.find("editor-steps-title").bounds());
    app.click(cx, "editor-shelf-search");
    settle(&app, cx);
    assert!(shown(&app, cx, "editor-shelf-field"));
    assert!(
        focused(&app, cx, "editor-shelf-field"),
        "typed into at once"
    );
    app.in_window(cx, |window, _| {
        // On the title's line, over it: no line of its own.
        assert!(window.try_find("editor-steps-title").is_none());
        let field = window.find("editor-shelf-field").bounds();
        let magnifier = window.find("editor-shelf-search").bounds();
        assert!(
            (field.center().y - magnifier.center().y).abs() < px(2.),
            "{field:?} {magnifier:?}"
        );
        assert!(
            field.left() <= title.left() + px(2.) && field.right() <= magnifier.left(),
            "{field:?} {title:?}"
        );
        let below = window.find(("read-lists", 0usize)).bounds();
        assert!(below.top() >= field.bottom(), "the steps stay under it");
    });
    app.in_window(cx, |window, cx| window.input("picture", cx));
    settle(&app, cx);
    app.in_window(cx, |window, _| {
        assert!(window.try_find(("read-picture", 5usize)).is_some());
        assert!(window.try_find(("read-fans", 2usize)).is_none());
    });
    app.in_window(cx, |window, cx| window.input("zzz", cx));
    settle(&app, cx);
    assert_eq!(
        app.label_of(cx, "editor-shelf-none"),
        "No step matches \u{201c}picturezzz\u{201d}"
    );
    app.press(cx, "escape");
    settle(&app, cx);
    assert!(!shown(&app, cx, "editor-shelf-field"));
    app.in_window(cx, |window, _| {
        assert!(
            window.try_find(("read-fans", 2usize)).is_some(),
            "all again"
        );
        assert_eq!(
            window.find("editor-steps-title").label(),
            Some("Actions"),
            "the title back"
        );
    });
    assert!(focused(&app, cx, "editor-shelf-search"));
    app.press(cx, "escape");
    settle(&app, cx);
    assert!(shown(&app, cx, "editor-filters"), "back at the kinds");
    assert!(
        focused(&app, cx, "editor-actions"),
        "focus on the kind left"
    );
    assert_eq!(app.title(cx).0, "Edit automation", "still in the editor");
}

/// A press of the left button at `at`, and its release.
fn press_at(app: &App, cx: &mut TestAppContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
    use gpui_kit::{MouseButton, MouseDownEvent, MouseUpEvent};
    let modifiers = Modifiers::default();
    app.in_window(cx, |window, cx| {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: at,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: at,
                modifiers,
                click_count: 1,
            }),
            cx,
        );
    });
    settle(app, cx);
}

#[gpui_kit::test]
async fn a_press_outside_the_panel_closes_its_search_and_gives_the_title_back(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.introduced(42);
    });
    let app = in_automations(cx, backend, still()).await;
    app.click(cx, "automations-create");
    settle(&app, cx);
    app.click(cx, "editor-shelf-search");
    settle(&app, cx);
    assert!(shown(&app, cx, "editor-shelf-field"));

    // A press in the field itself keeps it open.
    let field = app.in_window(cx, |window, _| {
        window.find("editor-shelf-field").bounds().center()
    });
    press_at(&app, cx, field);
    assert!(shown(&app, cx, "editor-shelf-field"), "still open");

    // One on the canvas closes it.
    let canvas = app.in_window(cx, |window, _| {
        let bounds = window.find("editor-canvas").bounds();
        bounds.origin + point(px(40.), bounds.size.height / 2.0)
    });
    press_at(&app, cx, canvas);
    assert!(!shown(&app, cx, "editor-shelf-field"), "closed");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("editor-steps-title").label(), Some("Triggers"));
    });

    // One on something that takes no focus leaves it on the magnifier.
    app.click(cx, "editor-shelf-search");
    settle(&app, cx);
    assert!(focused(&app, cx, "editor-shelf-field"));
    let edge = app.in_window(cx, |window, _| {
        let bounds = window.find("editor-zoom").bounds();
        bounds.origin + point(px(2.), bounds.size.height / 2.0)
    });
    press_at(&app, cx, edge);
    assert!(!shown(&app, cx, "editor-shelf-field"), "closed");
    assert!(focused(&app, cx, "editor-shelf-search"), "focus kept");
}

#[gpui_kit::test]
async fn the_wheel_zooms_the_canvas_where_the_pointer_is(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.introduced(42);
    });
    let app = in_automations(cx, backend, still()).await;
    app.click(cx, "automations-create");
    settle(&app, cx);

    let start = |app: &App, cx: &mut TestAppContext| {
        app.in_window(cx, |window, _| window.find("editor-start").bounds())
    };
    let before = start(&app, cx);
    // Up, over a corner of what marks the start: it grows, and that corner
    // stays under the pointer.
    let corner = before.origin + point(px(4.), px(4.));
    app.in_window(cx, |window, cx| {
        window.dispatch_event(
            PlatformInput::Scroll(ScrollEvent {
                position: corner,
                delta: ScrollDelta::Pixels(point(px(0.), px(200.))),
                modifiers: Modifiers::default(),
                ..ScrollEvent::default()
            }),
            cx,
        );
    });
    settle(&app, cx);
    let after = start(&app, cx);
    assert!(after.size.width > before.size.width, "{before:?} {after:?}");
    let moved =
        (after.origin + point(px(4.), px(4.)) * (after.size.width / before.size.width)) - corner;
    assert!(
        f32::from(moved.x).abs() < 2.0 && f32::from(moved.y).abs() < 2.0,
        "{moved:?}"
    );
    let percent = app.label_of(cx, "editor-zoom-reset");
    assert!(!percent.starts_with("Zoom at 100 "), "{percent}");
}

#[gpui_kit::test]
async fn the_grid_drifts_only_while_looked_at_and_lets_the_app_rest(cx: &mut TestAppContext) {
    let app = in_automations(
        cx,
        Scripted::with(vec![account(42, "me", true)]),
        Settings::default(),
    )
    .await;
    assert!(shown(&app, cx, "automations-begin"));
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert!(!app.system.is_resting(), "looked at, it drifts");

    VisualTestContext::from_window(app.window, cx).deactivate_window();
    cx.executor().advance_clock(QUIET_FOR * 2);
    cx.run_until_parked();
    assert!(app.system.is_resting(), "not looked at, nothing is drawn");

    cx.update_window(app.window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    assert!(!app.system.is_resting());
}

#[gpui_kit::test]
async fn home_and_search_have_their_titles_where_the_settings_have_theirs(cx: &mut TestAppContext) {
    let app = open_with(cx, Scripted::with(vec![account(42, "me", true)]), still());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    settle(&app, cx);
    let at = |app: &App, cx: &mut TestAppContext| {
        app.in_window(cx, |window, _| {
            let panel = window.find("panel").bounds();
            let title = window.find("title").bounds();
            (
                f32::from(title.origin.x - panel.origin.x),
                f32::from(title.origin.y - panel.origin.y),
            )
        })
    };
    let home = at(&app, cx);
    app.click(cx, "rail-search");
    app.until_screen(cx, |screen| *screen == Screen::Search)
        .await;
    settle(&app, cx);
    let search = at(&app, cx);
    app.press(cx, snob_desktop::ui::SETTINGS_KEYS);
    app.until_screen(cx, |screen| *screen == Screen::Settings)
        .await;
    settle(&app, cx);
    let settings = at(&app, cx);
    assert_eq!(home, settings);
    assert_eq!(search, settings);
    assert!(settings.0 < 40.0 && settings.1 < 40.0, "{settings:?}");
}

#[gpui_kit::test]
async fn tab_reaches_every_control_of_the_editor(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.introduced(42);
    });
    let app = in_automations(cx, backend, still()).await;
    app.click(cx, "automations-create");
    settle(&app, cx);
    let mut wanted = vec![
        "editor-back",
        "editor-zoom-out",
        "editor-zoom-reset",
        "editor-zoom-in",
        "editor-fit",
        "editor-shelf-search",
    ];
    for _ in 0..40 {
        app.press(cx, "tab");
        wanted.retain(|id| !focused(&app, cx, id));
        if wanted.is_empty() {
            break;
        }
    }
    assert!(wanted.is_empty(), "Tab never comes to {wanted:?}");
}

#[gpui_kit::test]
async fn the_wheel_zooms_from_the_middle_of_an_empty_canvas(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.introduced(42);
    });
    let app = in_automations(cx, backend, still()).await;
    app.click(cx, "automations-create");
    settle(&app, cx);
    let middle = app.in_window(cx, |window, _| {
        window.find("editor-canvas").bounds().center()
    });
    app.in_window(cx, |window, cx| {
        window.dispatch_event(
            PlatformInput::Scroll(ScrollEvent {
                position: middle,
                delta: ScrollDelta::Pixels(point(px(0.), px(240.))),
                modifiers: Modifiers::default(),
                ..ScrollEvent::default()
            }),
            cx,
        );
    });
    settle(&app, cx);
    let percent = app.label_of(cx, "editor-zoom-reset");
    assert!(!percent.starts_with("Zoom at 100 "), "{percent}");
}
