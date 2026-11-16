//! The account's menu, the settings and the report of a crash, driven in
//! process: what a person or a screen reader finds in them, and what the
//! backend and the system are asked.

#![cfg(feature = "testing")]

mod common;

use std::time::{Duration, UNIX_EPOCH};

use common::{App, focused, open};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Role, TestAppContext, VisualTestContext, px};
use snob_core::Pk;
use snob_desktop::backend::scripted::{Asked, Scripted, account};
use snob_desktop::state::flow::{Screen, Update};
use snob_desktop::state::rest::QUIET_FOR;
use snob_desktop::state::status::Status;
use snob_desktop::system::{Event, Notice};
use snob_desktop::theme::motion::PICTURE_ARRIVES;

async fn at_home(cx: &mut TestAppContext, backend: Scripted) -> App {
    let app = open(cx, backend);
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app
}

fn two_accounts() -> Scripted {
    Scripted::with(vec![
        account(42, "me", true),
        account(9001, "someone", false),
    ])
}

fn shown(app: &App, cx: &mut TestAppContext, id: &'static str) -> bool {
    app.in_window(cx, |window, _| {
        window.try_find(id).is_some_and(|found| found.visible())
    })
}

#[gpui_kit::test]
async fn the_picture_opens_a_menu_that_says_who_is_signed_in(cx: &mut TestAppContext) {
    let app = at_home(cx, Scripted::with(vec![account(42, "me", true)])).await;
    app.in_window(cx, |window, _| {
        let button = window.find("account");
        assert_eq!(button.role(), Some(Role::Button));
        assert_eq!(button.label(), Some("Account menu"));
        assert_eq!(button.expanded(), Some(false));
        assert_eq!(
            window.find("account-photo").label(),
            Some("Profile picture of @me")
        );
    });
    assert!(!shown(&app, cx, "account-menu"));

    app.click(cx, "account");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("account").expanded(), Some(true));
        let menu = window.find("account-menu");
        assert_eq!(menu.role(), Some(Role::Menu));
        assert_eq!(menu.label(), Some("Account menu"));
        assert_eq!(
            window.find("menu-photo").label(),
            Some("Profile picture of @me")
        );
        // One account: another can be added, and there is none to switch to.
        assert_eq!(window.find("menu-add").label(), Some("Add account"));
        assert!(window.try_find("menu-switch").is_none());
        for (id, label) in [("menu-settings", "Settings"), ("menu-sign-out", "Sign out")] {
            let item = window.find(id);
            assert_eq!(item.role(), Some(Role::MenuItem));
            assert_eq!(item.label(), Some(label));
        }
    });
}

#[gpui_kit::test]
async fn escape_closes_the_menu_and_focus_goes_back_to_what_opened_it(cx: &mut TestAppContext) {
    let app = at_home(cx, two_accounts()).await;
    app.click(cx, "account");
    assert!(shown(&app, cx, "account-menu"));

    // Tab stays inside the menu for as long as it is open.
    let inside = ["menu-switch", "menu-settings", "menu-sign-out"];
    for _ in 0..7 {
        app.press(cx, "tab");
        let on_an_item = inside.iter().any(|id| focused(&app, cx, id));
        assert!(on_an_item, "focus left the menu");
    }

    app.press(cx, "escape");
    assert!(!shown(&app, cx, "account-menu"));
    assert!(focused(&app, cx, "account"));
}

#[gpui_kit::test]
async fn the_other_accounts_are_listed_and_one_is_switched_to(cx: &mut TestAppContext) {
    let backend = two_accounts();
    let app = at_home(cx, backend.clone()).await;
    app.click(cx, "account");
    app.in_window(cx, |window, _| {
        let switch = window.find("menu-switch");
        assert_eq!(switch.label(), Some("Switch account"));
        assert_eq!(switch.expanded(), Some(false));
        assert!(window.try_find(("menu-account", 0usize)).is_none());
    });

    app.click(cx, "menu-switch");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("menu-switch").expanded(), Some(true));
        let other = window.find(("menu-account", 0usize));
        assert_eq!(other.label(), Some("@someone"));
        assert_eq!(window.find("menu-add").label(), Some("Add another account"));
    });

    app.in_window(cx, |window, cx| window.click(("menu-account", 0usize), cx));
    let shell = app.shell.clone();
    app.until(cx, move |_, cx| {
        shell
            .read(cx)
            .active()
            .is_some_and(|active| active.named.pk == 9001.into())
    })
    .await;
    assert!(backend.asked().contains(&Asked::SwitchTo(9001.into())));
    assert!(!shown(&app, cx, "account-menu"));
    assert_eq!(app.label_of(cx, "announcements"), "Now using @someone");
    assert_eq!(
        app.label_of(cx, "account-photo"),
        "Profile picture of @someone"
    );
}

#[gpui_kit::test]
async fn adding_an_account_goes_to_the_start_screen_and_can_be_backed_out_of(
    cx: &mut TestAppContext,
) {
    let app = at_home(cx, Scripted::with(vec![account(42, "me", true)])).await;
    app.click(cx, "account");
    app.click(cx, "menu-add");
    app.until_screen(cx, |screen| {
        matches!(screen, Screen::Start { back: true, .. })
    })
    .await;
    // On the way to signing in there is no account menu.
    assert!(!shown(&app, cx, "account"));

    app.click(cx, "back");
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    assert!(shown(&app, cx, "account"));
}

#[gpui_kit::test]
async fn signing_out_asks_first_and_no_is_the_answer_focus_starts_on(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    let app = at_home(cx, backend.clone()).await;
    app.click(cx, "account");
    app.click(cx, "menu-sign-out");

    app.in_window(cx, |window, _| {
        let question = window.find("sign-out");
        assert_eq!(question.role(), Some(Role::AlertDialog));
        assert_eq!(question.label(), Some("Sign out of @me?"));
        assert!(
            question
                .description()
                .is_some_and(|said| said.contains("this computer"))
        );
        assert!(window.try_find("account-menu").is_none());
    });
    assert!(focused(&app, cx, "sign-out-cancel"));

    app.press(cx, "escape");
    assert!(!shown(&app, cx, "sign-out"));
    assert!(!backend.asked().contains(&Asked::SignOut(42.into())));
    assert_eq!(app.title(cx).0, "Home");

    // Asked again, and answered yes: nobody is left, so on to the start.
    app.click(cx, "account");
    app.click(cx, "menu-sign-out");
    app.click(cx, "sign-out-confirm");
    app.until_screen(cx, |screen| {
        matches!(screen, Screen::Start { back: false, .. })
    })
    .await;
    assert!(backend.asked().contains(&Asked::SignOut(42.into())));
    assert!(!shown(&app, cx, "account"));
}

#[gpui_kit::test]
async fn signing_out_of_one_account_leaves_the_other_in_use(cx: &mut TestAppContext) {
    let app = at_home(cx, two_accounts()).await;
    app.click(cx, "account");
    app.click(cx, "menu-sign-out");
    app.click(cx, "sign-out-confirm");

    let shell = app.shell.clone();
    app.until(cx, move |_, cx| {
        shell
            .read(cx)
            .active()
            .is_some_and(|active| active.named.pk == 9001.into())
    })
    .await;
    assert_eq!(app.title(cx).0, "Home");
    assert_eq!(
        app.label_of(cx, "account-photo"),
        "Profile picture of @someone"
    );
}

#[gpui_kit::test]
async fn the_settings_hold_the_log_s_switch_its_folder_and_the_report(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    let app = at_home(cx, backend.clone()).await;

    // By its shortcut, which the menu shows.
    app.press(cx, snob_desktop::ui::SETTINGS_KEYS);
    app.until_screen(cx, |screen| *screen == Screen::Settings)
        .await;
    assert_eq!(app.title(cx), ("Settings".to_string(), true));
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("settings-logs").role(), Some(Role::Heading));
        let switch = window.find("settings-detailed");
        assert_eq!(switch.role(), Some(Role::Switch));
        assert_eq!(switch.label(), Some("Detailed logging"));
        assert_eq!(switch.checked(), Some(false));
    });

    app.click(cx, "settings-detailed");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("settings-detailed").checked(), Some(true));
    });
    assert!(backend.asked().contains(&Asked::SetDetailed(true)));
    app.click(cx, "settings-detailed");
    assert!(backend.asked().contains(&Asked::SetDetailed(false)));

    app.click(cx, "settings-open-logs");
    assert_eq!(app.system.opened(), [std::path::PathBuf::from("logs")]);

    app.click(cx, "settings-copy-report");
    let system = app.system.clone();
    app.until(cx, move |_, _| system.clipboard().is_some())
        .await;
    assert!(
        app.system
            .clipboard()
            .unwrap()
            .starts_with("snob desktop: diagnostic report")
    );
    assert_eq!(
        app.label_of(cx, "announcements"),
        "Diagnostic report copied"
    );

    app.click(cx, "back");
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
}

#[gpui_kit::test]
async fn the_report_of_a_crash_is_offered_until_it_is_dismissed(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.crash("panicked at src/main.rs:12\nsomething went wrong");
    });
    let app = at_home(cx, backend.clone()).await;

    assert_eq!(
        app.label_of(cx, "crash-title"),
        "snob closed unexpectedly last time"
    );
    assert!(!shown(&app, cx, "crash-report"));
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("crash-view").expanded(), Some(false));
    });

    app.click(cx, "crash-view");
    assert!(shown(&app, cx, "crash-report"));

    app.click(cx, "crash-copy");
    assert_eq!(
        app.system.clipboard().as_deref(),
        Some("panicked at src/main.rs:12\nsomething went wrong")
    );
    assert_eq!(app.label_of(cx, "announcements"), "Report copied");
    assert!(
        !backend.asked().contains(&Asked::CrashShown),
        "still offered"
    );

    app.click(cx, "crash-dismiss");
    assert!(!shown(&app, cx, "crash-title"));
    assert!(backend.asked().contains(&Asked::CrashShown));
    assert!(focused(&app, cx, "title"));
}

#[gpui_kit::test]
async fn no_two_controls_of_a_screen_share_a_name(cx: &mut TestAppContext) {
    let app = at_home(cx, two_accounts()).await;
    let unique = |app: &App, cx: &mut TestAppContext, what: &str| {
        app.in_window(cx, |window, _| {
            let mut names: Vec<String> = window
                .elements()
                .iter()
                .filter(|element| {
                    element.visible()
                        && matches!(
                            element.role(),
                            Some(Role::Button | Role::MenuItem | Role::Switch)
                        )
                })
                .map(|element| element.label().unwrap_or_default().to_string())
                .collect();
            assert!(
                names.iter().all(|name| !name.is_empty()),
                "{what}: {names:?}"
            );
            names.sort();
            let all = names.len();
            names.dedup();
            assert_eq!(names.len(), all, "{what}: a name is used twice");
        });
    };
    unique(&app, cx, "home");
    app.click(cx, "account");
    app.click(cx, "menu-switch");
    unique(&app, cx, "the menu");
    app.click(cx, "menu-settings");
    app.until_screen(cx, |screen| *screen == Screen::Settings)
        .await;
    unique(&app, cx, "settings");
}

fn woke() -> Event {
    Event::Woke {
        slept: Duration::from_secs(3600),
    }
}

#[gpui_kit::test]
async fn a_session_found_dead_while_the_app_is_open_asks_its_account_back(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    let app = open(cx, backend.clone());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    assert_eq!(backend.session_checks(), 1, "asked about once it is home");

    backend.script(|script| {
        script.session_dies(account(42, "me", true).named);
    });
    app.system.say(woke());
    app.until_screen(cx, |screen| matches!(screen, Screen::Start { .. }))
        .await;

    assert_eq!(app.label_of(cx, "expired-photo"), "Profile picture of @me");
    assert!(
        app.system.notified().is_empty(),
        "somebody looking at the window needs no notification"
    );
}

#[gpui_kit::test]
async fn somebody_not_looking_is_told_by_the_system_that_the_session_expired(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    let app = open(cx, backend.clone());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    VisualTestContext::from_window(app.window, cx).deactivate_window();

    backend.script(|script| {
        script.session_dies(account(42, "me", true).named);
    });
    app.system.say(woke());
    app.until_screen(cx, |screen| matches!(screen, Screen::Start { .. }))
        .await;

    let notified = app.system.notified();
    assert_eq!(notified.len(), 1, "{notified:?}");
    assert_eq!(notified[0].about, Notice::SessionExpired);
    assert_eq!(notified[0].title, "Your session has expired");
    assert_eq!(notified[0].body, "Sign in again as @me to keep using snob.");
}

#[gpui_kit::test]
async fn the_session_is_not_asked_about_without_a_network_or_before_anybody_signed_in(
    cx: &mut TestAppContext,
) {
    let nobody = Scripted::with(Vec::new());
    let app = open(cx, nobody.clone());
    app.until_screen(cx, |screen| matches!(screen, Screen::Start { .. }))
        .await;
    app.system.say(woke());
    cx.run_until_parked();
    assert_eq!(
        nobody.session_checks(),
        0,
        "there is no session to ask about"
    );

    let backend = Scripted::with(vec![account(42, "me", true)]);
    let app = open(cx, backend.clone());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    let before = backend.session_checks();
    app.system.say(Event::Online(false));
    app.system.say(woke());
    cx.run_until_parked();
    assert_eq!(backend.session_checks(), before, "nothing is sent offline");

    app.system.say(Event::Online(true));
    cx.run_until_parked();
    assert_eq!(
        backend.session_checks(),
        before + 1,
        "asked once it is back"
    );
}

#[gpui_kit::test]
async fn a_version_to_update_by_hand_is_said_by_the_system_to_somebody_not_looking(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.update(Update::ByItsManager);
    });
    backend.hold();
    let app = open(cx, backend.clone());
    VisualTestContext::from_window(app.window, cx).deactivate_window();
    backend.release();
    app.until_screen(cx, |screen| *screen == Screen::Home).await;

    assert_eq!(
        app.shell.read_with(cx, |shell, _| shell.status().cloned()),
        Some(Status::UpdateAvailable)
    );
    let notified = app.system.notified();
    assert_eq!(notified.len(), 1, "{notified:?}");
    assert_eq!(notified[0].about, Notice::UpdateReady);
    assert_eq!(notified[0].title, "A new version of snob is available");
}

#[gpui_kit::test]
async fn the_app_rests_while_nobody_looks_and_nothing_moves_and_wakes_when_looked_at(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    let app = open(cx, backend.clone());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    assert!(!app.system.is_resting(), "looked at, it never rests");

    VisualTestContext::from_window(app.window, cx).deactivate_window();
    cx.executor().advance_clock(QUIET_FOR / 2);
    cx.run_until_parked();
    assert!(!app.system.is_resting(), "not before the quiet");
    cx.executor().advance_clock(QUIET_FOR);
    cx.run_until_parked();
    assert!(app.system.is_resting());

    cx.update_window(app.window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    assert!(!app.system.is_resting(), "awake as soon as it is looked at");
}

#[gpui_kit::test]
async fn a_frame_drawn_wakes_the_app_before_it_is_looked_at(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    let app = open(cx, backend.clone());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    VisualTestContext::from_window(app.window, cx).deactivate_window();
    cx.executor().advance_clock(QUIET_FOR * 2);
    cx.run_until_parked();
    assert!(app.system.is_resting());

    app.in_window(cx, |_, _| ());
    assert!(!app.system.is_resting(), "the pointer over it, an answer");
    cx.executor().advance_clock(QUIET_FOR);
    cx.run_until_parked();
    assert!(app.system.is_resting(), "and it rests again once still");
}

/// How far `pk`'s picture has come in on the last frame.
fn picture_in(app: &App, cx: &mut TestAppContext, pk: u64) -> Option<f32> {
    app.in_window(cx, |_, _| ());
    app.shell.read_with(cx, |shell, _| {
        shell.pictures_shown().get(&Pk::new(pk)).copied()
    })
}

#[gpui_kit::test]
async fn a_picture_kept_is_drawn_at_once_and_a_new_one_comes_in(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("42.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 40, 120, 255]))
        .save(&file)
        .unwrap();
    let mut me = account(42, "me", true);
    me.picture = Some(file.clone());
    me.picture_kept_at = Some(UNIX_EPOCH + Duration::from_secs(1));
    let backend = Scripted::with(vec![me.clone()]);
    let app = open(cx, backend.clone());
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    cx.run_until_parked();
    assert_eq!(
        picture_in(&app, cx, 42),
        Some(1.0),
        "from the cache, at once"
    );

    // The day's check kept a new picture: the files say it was written again.
    image::RgbaImage::from_pixel(8, 8, image::Rgba([20, 140, 220, 255]))
        .save(&file)
        .unwrap();
    me.picture_kept_at = Some(UNIX_EPOCH + Duration::from_secs(2));
    backend.script(|script| {
        script.accounts(vec![me.clone()]);
    });
    app.system.say(woke());
    cx.run_until_parked();
    let coming = picture_in(&app, cx, 42).unwrap();
    assert!(coming < 1.0, "it comes in: {coming}");

    cx.executor().advance_clock(PICTURE_ARRIVES);
    cx.run_until_parked();
    assert_eq!(picture_in(&app, cx, 42), Some(1.0));
}

/// What the palette lists, by name, and which row is chosen.
fn listed(app: &App, cx: &mut TestAppContext) -> (Vec<String>, Option<usize>) {
    app.in_window(cx, |window, _| {
        let mut names = Vec::new();
        let mut chosen = None;
        while let Some(row) = window.try_find(("palette-command", names.len())) {
            assert_eq!(row.role(), Some(Role::ListBoxOption));
            if row.selected() == Some(true) {
                chosen = Some(names.len());
            }
            names.push(row.label().unwrap_or_default().to_string());
        }
        (names, chosen)
    })
}

#[gpui_kit::test]
async fn the_palette_lists_what_the_screen_can_do_and_finds_it_by_typing(cx: &mut TestAppContext) {
    let app = at_home(cx, two_accounts()).await;
    assert!(!shown(&app, cx, "palette"));

    app.press(cx, "ctrl-k");
    app.in_window(cx, |window, _| {
        let palette = window.find("palette");
        assert_eq!(palette.role(), Some(Role::Dialog));
        assert_eq!(palette.label(), Some("Command palette"));
        assert_eq!(window.find("palette-commands").role(), Some(Role::ListBox));
    });
    assert_eq!(
        listed(&app, cx),
        (
            [
                "Search",
                "Automations",
                "Settings",
                "Add account",
                "Switch to @someone",
                "Sign out",
                "Open the logs folder",
                "Copy the diagnostic report",
            ]
            .map(str::to_string)
            .to_vec(),
            Some(0)
        )
    );

    // What is typed goes to its field without a click, and narrows the list.
    app.in_window(cx, |window, cx| window.input("LOGS open", cx));
    assert_eq!(
        listed(&app, cx),
        (vec!["Open the logs folder".to_string()], Some(0))
    );
    app.press(cx, "enter");
    assert!(!shown(&app, cx, "palette"), "doing a command closes it");
    assert_eq!(app.system.opened().len(), 1, "the logs folder was opened");

    app.press(cx, "ctrl-k");
    app.in_window(cx, |window, cx| window.input("nothing like this", cx));
    assert_eq!(listed(&app, cx), (Vec::new(), None));
    assert!(shown(&app, cx, "palette-empty"));
    app.press(cx, "enter");
    assert!(shown(&app, cx, "palette"), "there is nothing to do");
}

#[gpui_kit::test]
async fn the_arrows_choose_in_the_palette_and_escape_hands_focus_back(cx: &mut TestAppContext) {
    let app = at_home(cx, Scripted::with(vec![account(42, "me", true)])).await;
    app.press(cx, "ctrl-k");
    let (names, chosen) = listed(&app, cx);
    assert_eq!(chosen, Some(0));

    app.press(cx, "down");
    assert_eq!(listed(&app, cx).1, Some(1));
    app.press(cx, "up");
    app.press(cx, "up");
    assert_eq!(listed(&app, cx).1, Some(names.len() - 1), "around the end");
    app.press(cx, "down");
    assert_eq!(listed(&app, cx).1, Some(0));

    app.press(cx, "escape");
    assert!(!shown(&app, cx, "palette"));
    assert!(focused(&app, cx, "title"), "back where it was");

    // The second row is the one Enter does.
    app.press(cx, "ctrl-k");
    app.press(cx, "down");
    app.press(cx, "enter");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;
    assert!(!shown(&app, cx, "palette"));
}

#[gpui_kit::test]
async fn the_palette_offers_the_two_ways_in_and_nothing_on_the_way(cx: &mut TestAppContext) {
    let backend = Scripted::with(Vec::new());
    backend.hold();
    let app = open(cx, backend.clone());
    app.press(cx, "ctrl-k");
    assert!(
        !shown(&app, cx, "palette"),
        "nothing to do under the splash"
    );
    backend.release();
    app.until_screen(cx, |screen| matches!(screen, Screen::Start { .. }))
        .await;

    app.press(cx, "ctrl-k");
    assert_eq!(
        listed(&app, cx).0,
        ["Sign in to Instagram", "Use your session ID (advanced)"]
    );
    app.press(cx, "down");
    app.press(cx, "enter");
    app.until_screen(cx, |screen| *screen == Screen::Paste)
        .await;
    app.press(cx, "ctrl-k");
    assert!(!shown(&app, cx, "palette"));
}

#[gpui_kit::test]
async fn the_arrows_walk_the_menu_around_its_ends(cx: &mut TestAppContext) {
    let app = at_home(cx, Scripted::with(vec![account(42, "me", true)])).await;
    app.click(cx, "account");
    let items = ["menu-add", "menu-settings", "menu-sign-out"];
    let on = |app: &App, cx: &mut TestAppContext| {
        items
            .iter()
            .position(|id| focused(app, cx, id))
            .map(|at| items[at])
    };

    app.press(cx, "down");
    assert_eq!(on(&app, cx), Some("menu-add"));
    app.press(cx, "down");
    assert_eq!(on(&app, cx), Some("menu-settings"));
    app.press(cx, "down");
    app.press(cx, "down");
    assert_eq!(on(&app, cx), Some("menu-add"), "around the end");
    app.press(cx, "up");
    assert_eq!(on(&app, cx), Some("menu-sign-out"), "and back around it");
}

#[gpui_kit::test]
async fn the_rail_goes_to_each_section_and_says_which_one_the_app_is_in(cx: &mut TestAppContext) {
    let app = at_home(cx, Scripted::with(vec![account(42, "me", true)])).await;
    let chosen = |app: &App, cx: &mut TestAppContext| {
        app.in_window(cx, |window, _| {
            ["rail-home", "rail-search", "rail-automations"]
                .into_iter()
                .filter(|id| window.find(*id).selected() == Some(true))
                .collect::<Vec<_>>()
        })
    };
    app.in_window(cx, |window, _| {
        for (id, name) in [
            ("rail-home", "Home"),
            ("rail-search", "Search"),
            ("rail-automations", "Automations"),
        ] {
            let button = window.find(id);
            assert_eq!(button.role(), Some(Role::Button));
            assert_eq!(button.label(), Some(name));
            let size = button.bounds().size;
            assert!(size.width >= px(24.) && size.height >= px(24.));
        }
    });
    assert_eq!(chosen(&app, cx), ["rail-home"]);

    app.click(cx, "rail-search");
    app.until_screen(cx, |screen| *screen == Screen::Search)
        .await;
    assert_eq!(app.title(cx), ("Search".to_string(), true));
    assert_eq!(chosen(&app, cx), ["rail-search"]);

    // And with the keyboard: Tab comes to the rail, which the arrows do not
    // need to be walked.
    let mut presses = 0;
    while !focused(&app, cx, "rail-automations") {
        app.press(cx, "tab");
        presses += 1;
        assert!(presses < 8, "Tab never comes to the rail");
    }
    app.press(cx, "enter");
    app.until_screen(cx, |screen| *screen == Screen::Automations)
        .await;

    // The settings are none of them.
    app.press(cx, snob_desktop::ui::SETTINGS_KEYS);
    app.until_screen(cx, |screen| *screen == Screen::Settings)
        .await;
    assert!(chosen(&app, cx).is_empty());
    app.click(cx, "rail-home");
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
}

/// The window left for another and looked at again.
fn look_away_and_back(app: &App, cx: &mut TestAppContext) {
    VisualTestContext::from_window(app.window, cx).deactivate_window();
    app.in_window(cx, |window, _| window.activate_window());
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn what_the_command_line_did_meanwhile_is_found_when_the_window_is_looked_at_again(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(Vec::new());
    let app = open(cx, backend.clone());
    app.until_screen(cx, |screen| matches!(screen, Screen::Start { .. }))
        .await;

    // Somebody signs in with `snob login`.
    backend.script(|script| {
        script.accounts(vec![account(42, "me", true)]);
    });
    look_away_and_back(&app, cx);
    app.until_screen(cx, |screen| matches!(screen, Screen::Welcome(_)))
        .await;
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    assert_eq!(app.label_of(cx, "account-photo"), "Profile picture of @me");

    // A second account is signed in there, and becomes the one in use.
    backend.script(|script| {
        script.accounts(vec![
            account(9001, "someone", true),
            account(42, "me", false),
        ]);
    });
    look_away_and_back(&app, cx);
    assert_eq!(
        app.label_of(cx, "account-photo"),
        "Profile picture of @someone"
    );
    assert_eq!(app.label_of(cx, "announcements"), "Now using @someone");
    assert_eq!(
        app.shell.read_with(cx, |shell, _| shell.shown().clone()),
        Screen::Home
    );

    // And everybody is signed out.
    backend.script(|script| {
        script.accounts(Vec::new());
    });
    look_away_and_back(&app, cx);
    app.until_screen(cx, |screen| matches!(screen, Screen::Start { .. }))
        .await;
    assert!(!shown(&app, cx, "account"));
}

#[gpui_kit::test]
async fn looking_again_changes_nothing_while_an_account_is_being_added(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    let app = at_home(cx, backend.clone()).await;
    app.click(cx, "account");
    app.click(cx, "menu-add");
    app.until_screen(cx, |screen| matches!(screen, Screen::Start { .. }))
        .await;
    let before = backend.asked().len();

    look_away_and_back(&app, cx);
    assert_eq!(backend.asked().len(), before, "nothing is read again");
    assert!(matches!(
        app.shell.read_with(cx, |shell, _| shell.shown().clone()),
        Screen::Start { back: true, .. }
    ));
}
