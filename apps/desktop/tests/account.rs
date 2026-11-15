//! The account's menu, the settings and the report of a crash, driven in
//! process: what a person or a screen reader finds in them, and what the
//! backend and the system are asked.

#![cfg(feature = "testing")]

mod common;

use common::{App, focused, open};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Role, TestAppContext};
use snob_desktop::backend::scripted::{Asked, Scripted, account};
use snob_desktop::state::flow::Screen;

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
