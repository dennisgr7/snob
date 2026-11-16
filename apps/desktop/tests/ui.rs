//! The window, driven in process: the real views on a backend that answers
//! what the test told it to and a system the test speaks for. No engine, no
//! Instagram and no display: what is checked is what a person or a screen
//! reader would find, by name and by role.

#![cfg(feature = "testing")]

mod common;

use std::time::Duration;

use common::{App, disabled, open, open_with};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Role, TestAppContext, Window, px, size};
use snob_desktop::backend::SignIn;
use snob_desktop::backend::scripted::{Asked, Scripted, account};
use snob_desktop::state::flow::{Method, Screen, Update, Updating};
use snob_desktop::state::status::Status;
use snob_desktop::system::{Event, Settings};
use snob_desktop::theme::tokens::frame::{RAIL, SIDE};
use snob_desktop::theme::tokens::{Palette, Scheme};
use snob_desktop::ui::look::Look;
use snob_engine::login::Failure;

fn start() -> impl Fn(&Screen) -> bool {
    |screen| matches!(screen, Screen::Start { .. })
}

#[gpui_kit::test]
async fn the_splash_gives_way_to_the_start_screen_when_nobody_is_signed_in(
    cx: &mut TestAppContext,
) {
    let app = open(cx, Scripted::with(Vec::new()));
    assert_eq!(
        app.shell.read_with(cx, |shell, _| shell.shown().clone()),
        Screen::Splash
    );
    assert_eq!(app.label_of(cx, "splash-brand"), "snob.");

    app.until_screen(cx, start()).await;

    assert_eq!(app.title(cx), ("Let's get started.".to_string(), true));
    app.in_window(cx, |window, _| {
        for (id, label) in [
            ("start-browser", "Sign in to Instagram"),
            ("start-paste", "Use your session ID (advanced)"),
        ] {
            let button = window.find(id);
            assert_eq!(button.role(), Some(Role::Button));
            assert_eq!(button.label(), Some(label));
            assert!(button.visible());
        }
        assert!(
            window.try_find("back").is_none(),
            "there is nowhere to go back to"
        );
        assert!(window.try_find("account").is_none(), "nobody is signed in");
    });
    // Nothing but reading what is here and the update check was asked for.
    let asked: Vec<Asked> = app
        .backend
        .asked()
        .into_iter()
        .filter(|asked| *asked != Asked::RememberWindow)
        .collect();
    assert_eq!(asked, [Asked::Load, Asked::CheckForUpdate]);
}

#[gpui_kit::test]
async fn an_account_signed_in_already_goes_straight_home_with_its_picture_in_the_band(
    cx: &mut TestAppContext,
) {
    let app = open(cx, Scripted::with(vec![account(42, "me", true)]));
    app.until_screen(cx, |screen| *screen == Screen::Home).await;

    assert_eq!(app.title(cx), ("Home".to_string(), true));
    app.in_window(cx, |window, _| {
        let picture = window.find("account-photo");
        assert_eq!(picture.label(), Some("Profile picture of @me"));
        assert!(picture.visible());
    });
}

#[gpui_kit::test]
async fn the_frame_holds_the_panel_on_four_sides_on_the_way_in_and_says_it_is_a_test_build(
    cx: &mut TestAppContext,
) {
    let app = open(cx, Scripted::with(Vec::new()));
    let framed = |window: &mut Window| {
        let band = window.find("band");
        let panel = window.find("panel").bounds();
        assert_eq!(band.role(), Some(Role::Banner));
        assert_eq!(window.find("panel").role(), Some(Role::Main));
        assert_eq!(band.bounds().size.height, px(42.));
        assert_eq!(panel.origin.y, px(42.), "the panel starts under the band");
        let viewport = window.viewport_size();
        assert_eq!(panel.origin.x, px(SIDE));
        assert_eq!(panel.right(), viewport.width - px(SIDE));
        assert_eq!(panel.bottom(), viewport.height - px(SIDE));
        assert!(window.try_find("rail").is_none(), "no sections yet");
        assert_eq!(window.find("brand").label(), Some("snob."));
        assert_eq!(window.find("test-badge").label(), Some("TEST"));
    };
    app.in_window(cx, |window, _| framed(window));
    app.until_screen(cx, start()).await;
    app.in_window(cx, |window, _| framed(window));
}

#[gpui_kit::test]
async fn a_start_already_signed_in_draws_the_frame_open_from_its_first_home_frame(
    cx: &mut TestAppContext,
) {
    let app = open(cx, Scripted::with(vec![account(42, "me", true)]));
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    // The first frame home is drawn in, with no time let pass.
    app.in_window(cx, |window, _| {
        let panel = window.find("panel").bounds();
        let viewport = window.viewport_size();
        assert_eq!(panel.origin.x, px(RAIL), "the rail at its left");
        assert_eq!(panel.right(), viewport.width, "against the window's right");
        assert_eq!(panel.bottom(), viewport.height, "and its bottom");
        let rail = window.find("rail");
        assert_eq!(rail.role(), Some(Role::Navigation));
        assert_eq!(rail.label(), Some("Sections"));
    });
}
/// Where the panel's left edge is now.
fn panel_left(app: &App, cx: &mut TestAppContext) -> f32 {
    app.in_window(cx, |window, _| {
        f32::from(window.find("panel").bounds().origin.x)
    })
}

#[gpui_kit::test]
async fn the_frame_opens_once_somebody_is_welcomed_and_closes_to_add_an_account(
    cx: &mut TestAppContext,
) {
    let app = open(cx, Scripted::with(Vec::new()));
    app.until_screen(cx, start()).await;
    app.click(cx, "start-browser");
    app.until_screen(cx, |screen| matches!(screen, Screen::Welcome(_)))
        .await;
    assert_eq!(panel_left(&app, cx), SIDE, "closed for the welcome");

    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.until(cx, |window, _| {
        window.find("panel").bounds().origin.x == px(RAIL)
    })
    .await;
    app.in_window(cx, |window, _| {
        assert!(window.find("rail-home").visible());
    });

    app.click(cx, "account");
    app.click(cx, "menu-add");
    app.until_screen(cx, start()).await;
    app.until(cx, |window, _| {
        window.find("panel").bounds().origin.x == px(SIDE) && window.try_find("rail").is_none()
    })
    .await;
}

#[gpui_kit::test]
async fn the_brand_stays_in_the_middle_whatever_the_band_says(cx: &mut TestAppContext) {
    let app = open(cx, Scripted::with(vec![account(42, "me", true)]));
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    let middle_of_the_brand = |app: &App, cx: &mut TestAppContext| {
        app.in_window(cx, |window, _| {
            let brand = window.find("brand").bounds();
            let off = brand.center().x - window.viewport_size().width / 2.;
            (f32::from(off).abs(), brand)
        })
    };
    let (off, _) = middle_of_the_brand(&app, cx);
    assert!(off <= 1.0, "{off} px off the middle with nothing said");

    // The longest status, in the smallest window the app is used in.
    app.backend.script(|script| {
        script.paused_until(Some(snob_core::Epoch::new(
            snob_core::clock::now().get() + 3600,
        )));
    });
    app.system.say(Event::Woke {
        slept: Duration::from_secs(60),
    });
    let shell = app.shell.clone();
    app.until(cx, move |_, cx| {
        matches!(shell.read(cx).status(), Some(Status::Paused { .. }))
    })
    .await;
    cx.simulate_window_resize(app.window, size(px(800.), px(600.)));

    let (off, brand) = middle_of_the_brand(&app, cx);
    assert!(off <= 1.0, "{off} px off the middle under a long status");
    app.in_window(cx, |window, _| {
        let status = window.find("status").bounds();
        assert!(
            status.right() <= brand.left(),
            "the status ends before the brand begins"
        );
        assert!(status.left() >= px(0.), "and is cut short, not pushed out");
        let close = window.find("window-close");
        if close.visible() {
            assert!(window.find("account").bounds().right() <= close.bounds().left());
        }
    });
}

#[gpui_kit::test]
async fn the_status_starts_at_the_left_edge_of_the_band_away_from_the_brand(
    cx: &mut TestAppContext,
) {
    let app = open(cx, Scripted::with(Vec::new()));
    app.until_screen(cx, start()).await;
    cx.simulate_window_resize(app.window, size(px(1100.), px(760.)));
    app.system.say(Event::Online(false));
    let shell = app.shell.clone();
    app.until(cx, move |_, cx| {
        shell.read(cx).status() == Some(&Status::Offline)
    })
    .await;

    app.in_window(cx, |window, _| {
        let band = window.find("band").bounds();
        let status = window.find("status").bounds();
        let brand = window.find("brand").bounds();
        let edge = if cfg!(target_os = "macos") {
            78.
        } else {
            snob_desktop::theme::tokens::frame::SIDE
        };
        let from_the_edge = f32::from(status.left() - band.left());
        assert!(
            (from_the_edge - edge).abs() <= 1.0,
            "the status starts {from_the_edge} px from the band's left edge"
        );
        assert!(
            status.right() + px(100.) < brand.left(),
            "a short status is not drawn against the brand"
        );
    });
}

#[gpui_kit::test]
async fn what_the_band_says_is_said_to_a_screen_reader_too(cx: &mut TestAppContext) {
    let app = open(cx, Scripted::with(Vec::new()));
    app.until_screen(cx, start()).await;
    assert_eq!(app.label_of(cx, "status"), "");

    app.system.say(Event::Online(false));
    let shell = app.shell.clone();
    app.until(cx, move |_, cx| {
        shell.read(cx).status() == Some(&Status::Offline)
    })
    .await;
    assert_eq!(app.label_of(cx, "status"), "No internet connection");
    assert_eq!(app.label_of(cx, "announcements"), "No internet connection");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("status").role(), Some(Role::Status));
    });

    app.system.say(Event::Online(true));
    let shell = app.shell.clone();
    app.until(cx, move |_, cx| shell.read(cx).status().is_none())
        .await;
    assert_eq!(app.label_of(cx, "status"), "");
}

#[gpui_kit::test]
async fn the_theme_and_the_language_are_the_system_s_and_follow_it_at_once(
    cx: &mut TestAppContext,
) {
    let app = open_with(
        cx,
        Scripted::with(Vec::new()),
        Settings {
            locale: "es-ES".into(),
            ..Settings::default()
        },
    );
    app.until_screen(cx, start()).await;
    assert_eq!(app.title(cx).0, "Empecemos."); // i18n-allow
    assert_eq!(app.label_of(cx, "test-badge"), "PRUEBA"); // i18n-allow
    let palette = |cx: &mut TestAppContext| cx.update(|cx| Look::now(cx).palette);
    assert_eq!(palette(cx), Palette::of(Scheme::Light, false));

    app.system.change(|settings| settings.dark = true);
    app.until(cx, |_, cx| {
        Look::now(cx).palette == Palette::of(Scheme::Dark, false)
    })
    .await;

    app.system.change(|settings| {
        settings.more_contrast = true;
        settings.text_scale = 2.0;
    });
    app.until(cx, |window, cx| {
        Look::now(cx).palette == Palette::of(Scheme::Dark, true) && window.rem_size() == px(32.)
    })
    .await;
    // Nothing went anywhere: the screen is the one that was shown.
    assert_eq!(app.title(cx).0, "Empecemos."); // i18n-allow
}

#[gpui_kit::test]
async fn the_tab_key_walks_the_screen_in_the_order_it_is_drawn(cx: &mut TestAppContext) {
    let app = open(cx, Scripted::with(Vec::new()));
    app.until_screen(cx, start()).await;
    let focused = common::focused;

    app.press(cx, "tab");
    assert!(focused(&app, cx, "start-browser"));
    app.press(cx, "tab");
    assert!(focused(&app, cx, "start-paste"));
    app.press(cx, "shift-tab");
    assert!(focused(&app, cx, "start-browser"));

    // And a button is pressed with the keyboard alone.
    app.press(cx, "tab");
    app.press(cx, "enter");
    app.until_screen(cx, |screen| *screen == Screen::Paste)
        .await;
    assert_eq!(app.title(cx), ("Use your session ID".to_string(), true));
}

#[gpui_kit::test]
async fn a_method_chosen_can_be_backed_out_of(cx: &mut TestAppContext) {
    let app = open(cx, Scripted::with(Vec::new()));
    app.until_screen(cx, start()).await;

    app.click(cx, "start-paste");
    app.until_screen(cx, |screen| *screen == Screen::Paste)
        .await;
    app.click(cx, "back");
    app.until_screen(cx, start()).await;
    assert_eq!(app.title(cx), ("Let's get started.".to_string(), true));
}

#[gpui_kit::test]
async fn signing_in_in_the_browser_window_is_welcomed_and_then_home(cx: &mut TestAppContext) {
    let backend = Scripted::with(Vec::new());
    let app = open(cx, backend.clone());
    app.until_screen(cx, start()).await;

    backend.hold();
    app.click(cx, "start-browser");
    app.until_screen(cx, |screen| *screen == Screen::Browser)
        .await;
    assert_eq!(app.title(cx), ("Sign in to Instagram".to_string(), true));
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("cancel").label(), Some("Cancel"));
    });
    assert_eq!(
        backend.asked().last(),
        Some(&Asked::SignInInAWindow { renewing: None })
    );

    backend.release();
    app.until_screen(cx, |screen| matches!(screen, Screen::Welcome(_)))
        .await;
    assert_eq!(app.title(cx), ("Welcome, @me.".to_string(), true));
    assert_eq!(app.label_of(cx, "announcements"), "Welcome, @me.");
    assert_eq!(app.label_of(cx, "welcome-photo"), "Profile picture of @me");

    // It moves on by itself.
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    assert_eq!(app.label_of(cx, "account-photo"), "Profile picture of @me");
}

#[gpui_kit::test]
async fn canceling_the_browser_login_closes_its_window_and_is_no_error(cx: &mut TestAppContext) {
    let backend = Scripted::with(Vec::new());
    let app = open(cx, backend.clone());
    app.until_screen(cx, start()).await;
    backend.hold();
    app.click(cx, "start-browser");
    app.until_screen(cx, |screen| *screen == Screen::Browser)
        .await;

    app.click(cx, "cancel");
    app.until_screen(cx, start()).await;
    assert!(backend.window_was_stopped());

    // The answer that comes after it changes nothing.
    backend.release();
    cx.run_until_parked();
    assert_eq!(app.title(cx).0, "Let's get started.");
}

#[gpui_kit::test]
async fn a_failure_says_why_in_words_and_keeps_its_code_folded_away(cx: &mut TestAppContext) {
    let backend = Scripted::with(Vec::new());
    backend.script(|script| {
        script.sign_in(SignIn::Failed {
            why: Failure::Challenge,
            id: "7K2Q-9FMX".into(),
            until: None,
        });
    });
    let app = open(cx, backend.clone());
    app.until_screen(cx, start()).await;
    app.click(cx, "start-browser");
    app.until_screen(cx, |screen| matches!(screen, Screen::Failed { .. }))
        .await;

    assert_eq!(app.title(cx), ("That did not work".to_string(), true));
    let why = "Instagram wants to confirm it is you. Sign in with the browser to complete it.";
    app.in_window(cx, |window, _| {
        let said = window.find("error-why");
        assert_eq!(said.role(), Some(Role::Alert));
        assert_eq!(said.label(), Some(why));
        assert_eq!(window.find("error-details").expanded(), Some(false));
        assert!(window.find_all_by_label("Code 7K2Q-9FMX").is_empty());
    });
    assert_eq!(app.label_of(cx, "announcements"), why);

    app.click(cx, "error-details");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("error-details").expanded(), Some(true));
    });

    // The same method again, which now works.
    backend.script(|script| {
        script.sign_in(SignIn::Done(account(42, "me", true)));
    });
    app.click(cx, "error-retry");
    app.until_screen(cx, |screen| matches!(screen, Screen::Welcome(_)))
        .await;
}

#[gpui_kit::test]
async fn an_expired_session_asks_that_account_back_by_name(cx: &mut TestAppContext) {
    let backend = Scripted::with(vec![account(42, "me", true)]);
    backend.script(|script| {
        script.expired(account(42, "me", true).named);
    });
    let app = open(cx, backend.clone());
    app.until_screen(cx, start()).await;

    assert_eq!(app.label_of(cx, "expired-photo"), "Profile picture of @me");
    backend.hold();
    app.click(cx, "start-browser");
    app.until_screen(cx, |screen| *screen == Screen::Browser)
        .await;
    assert_eq!(
        backend.asked().last(),
        Some(&Asked::SignInInAWindow {
            renewing: Some(42.into())
        })
    );
}

#[gpui_kit::test]
async fn an_update_is_fetched_with_its_progress_and_the_app_restarts_into_the_method(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(Vec::new());
    backend.script(|script| {
        script.update(Update::Ready).fetch(vec![Some(63)], true);
    });
    let app = open(cx, backend.clone());
    app.until_screen(cx, start()).await;

    backend.hold();
    app.click(cx, "start-paste");
    app.until_screen(cx, |screen| {
        matches!(
            screen,
            Screen::Updating {
                at: Updating::Downloading(_),
                ..
            }
        )
    })
    .await;
    assert_eq!(app.title(cx), ("Downloading update".to_string(), true));
    assert_eq!(app.label_of(cx, "status"), "Downloading update");
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find("update-progress").role(),
            Some(Role::ProgressIndicator)
        );
    });
    assert!(
        backend
            .asked()
            .contains(&Asked::RememberMethod(Some(Method::Paste))),
        "the method is remembered for after the restart"
    );

    backend.release();
    let system = app.system.clone();
    app.until(cx, move |_, _| system.restarts() == 1).await;
    assert_eq!(app.title(cx).0, "Restarting…");
}

#[gpui_kit::test]
async fn a_check_that_does_not_answer_is_waited_for_and_then_given_up_on(cx: &mut TestAppContext) {
    let backend = Scripted::with(Vec::new());
    backend.script(|script| {
        script.check_never_answers();
    });
    let app = open(cx, backend);
    app.until_screen(cx, start()).await;

    app.click(cx, "start-paste");
    app.until_screen(cx, |screen| {
        matches!(
            screen,
            Screen::Updating {
                at: Updating::Checking,
                ..
            }
        )
    })
    .await;
    assert_eq!(app.title(cx), ("Checking for updates".to_string(), true));

    // Eight seconds of the test's clock later, on without it.
    app.until_screen(cx, |screen| *screen == Screen::Paste)
        .await;
}

/// The session ID the scripted backend expects.
const SESSION_ID: &str = "42%3AAbCdEfGhIjKl%3A20";

async fn on_the_paste_screen(cx: &mut TestAppContext, backend: Scripted) -> App {
    let app = open(cx, backend);
    app.until_screen(cx, start()).await;
    app.click(cx, "start-paste");
    app.until_screen(cx, |screen| *screen == Screen::Paste)
        .await;
    app
}

#[gpui_kit::test]
async fn what_is_typed_is_judged_once_the_typing_stops_and_says_what_to_copy(
    cx: &mut TestAppContext,
) {
    let app = on_the_paste_screen(cx, Scripted::with(Vec::new())).await;
    assert!(
        disabled(&app, cx, "paste-submit"),
        "nothing is in the field"
    );

    app.click(cx, "session-id");
    app.in_window(cx, |window, cx| window.input("sessionid", cx));
    // Not judged while the person may still be typing.
    assert_eq!(app.label_of(cx, "announcements"), "");

    app.until(cx, |window, _| {
        window.find("announcements").label() != Some("")
    })
    .await;
    assert!(disabled(&app, cx, "paste-submit"));
    assert_eq!(
        app.label_of(cx, "announcements"),
        "Copy only the value of the sessionid cookie."
    );
}

#[gpui_kit::test]
async fn a_pasted_id_is_judged_at_once_stays_hidden_and_signs_in(cx: &mut TestAppContext) {
    let backend = Scripted::with(Vec::new());
    let app = on_the_paste_screen(cx, backend.clone()).await;

    app.system.put_on_clipboard(&format!("  {SESSION_ID}\n"));
    app.click(cx, "paste-paste");
    assert!(!disabled(&app, cx, "paste-submit"), "judged with no wait");

    // Hidden, and never told to whoever reads the window.
    let told = |app: &App, cx: &mut TestAppContext| {
        app.in_window(cx, |window, _| {
            window.elements().iter().any(|element| {
                [element.label(), element.value(), element.description()]
                    .into_iter()
                    .flatten()
                    .any(|said| said.contains("AbCdEfGhIjKl"))
            })
        })
    };
    assert!(!told(&app, cx));
    assert_eq!(app.label_of(cx, "paste-reveal"), "Show the session ID");
    app.click(cx, "paste-reveal");
    assert_eq!(app.label_of(cx, "paste-reveal"), "Hide the session ID");
    assert!(!told(&app, cx), "shown to the eye, still not read out");

    backend.hold();
    app.click(cx, "paste-submit");
    assert_eq!(app.label_of(cx, "paste-submit"), "Checking with Instagram");
    assert!(disabled(&app, cx, "paste-submit"));
    assert_eq!(
        backend.asked().last(),
        Some(&Asked::SignInWith { expected: true })
    );

    backend.release();
    app.until_screen(cx, |screen| matches!(screen, Screen::Welcome(_)))
        .await;
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
}

#[gpui_kit::test]
async fn an_id_instagram_refuses_is_an_error_and_the_field_is_empty_on_the_way_back(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(Vec::new());
    backend.script(|script| {
        script.sign_in(SignIn::Failed {
            why: Failure::Rejected,
            id: "7K2Q-9FMX".into(),
            until: None,
        });
    });
    let app = on_the_paste_screen(cx, backend.clone()).await;
    app.system.put_on_clipboard(SESSION_ID);
    app.click(cx, "paste-paste");
    app.click(cx, "paste-submit");
    app.until_screen(cx, |screen| matches!(screen, Screen::Failed { .. }))
        .await;
    assert_eq!(
        app.label_of(cx, "error-why"),
        "That session ID no longer works. Get a new one, or sign in with the browser."
    );

    app.click(cx, "error-retry");
    app.until_screen(cx, |screen| *screen == Screen::Paste)
        .await;
    assert!(disabled(&app, cx, "paste-submit"), "nothing was kept");
}

#[gpui_kit::test]
async fn the_paste_screen_says_where_to_find_an_id_in_four_steps(cx: &mut TestAppContext) {
    let app = on_the_paste_screen(cx, Scripted::with(Vec::new())).await;
    app.in_window(cx, |window, _| {
        let steps = window.find_all_by_role(Role::ListItem);
        assert_eq!(steps.len(), 4);
        assert!(steps.iter().all(|step| step.visible()));
        assert_eq!(steps[3].label(), Some("Copy the value of sessionid."));
    });
}

#[gpui_kit::test]
async fn one_browser_found_is_said_under_the_field_and_is_what_the_session_is_sent_as(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(Vec::new());
    let app = on_the_paste_screen(cx, backend.clone()).await;
    cx.run_until_parked();

    app.in_window(cx, |window, _| {
        let browser = window.find("paste-browser");
        assert_eq!(browser.role(), Some(Role::Label));
        assert_eq!(browser.label(), Some("Chrome detected"));
        assert!(window.try_find("paste-browsers").is_none());
    });

    app.system.put_on_clipboard(SESSION_ID);
    app.click(cx, "paste-paste");
    app.click(cx, "paste-submit");
    app.until_screen(cx, |screen| matches!(screen, Screen::Welcome(_)))
        .await;
    assert_eq!(backend.sent_as().as_deref(), Some("Chrome"));
}

#[gpui_kit::test]
async fn several_browsers_are_chosen_among_starting_from_the_one_chosen_last(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(Vec::new());
    backend.script(|script| {
        script.browsers(&["Chrome", "Edge", "Brave"], Some("Edge"));
    });
    let app = on_the_paste_screen(cx, backend.clone()).await;
    cx.run_until_parked();

    app.in_window(cx, |window, _| {
        let browser = window.find("paste-browser");
        assert_eq!(browser.role(), Some(Role::ComboBox));
        assert_eq!(
            browser.label(),
            Some("Browser this session comes from: Edge")
        );
        assert_eq!(browser.expanded(), Some(false));
        assert!(window.try_find("paste-browsers").is_none());
    });

    app.click(cx, "paste-browser");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("paste-browser").expanded(), Some(true));
        assert_eq!(window.find("paste-browsers").role(), Some(Role::ListBox));
        for (at, name) in ["Chrome", "Edge", "Brave"].into_iter().enumerate() {
            let option = window.find(("paste-browser-option", at));
            assert_eq!(option.role(), Some(Role::ListBoxOption));
            assert_eq!(option.label(), Some(name));
            assert_eq!(option.selected(), Some(name == "Edge"));
        }
    });
    // Escape closes the list and nothing else.
    app.press(cx, "escape");
    app.in_window(cx, |window, _| {
        assert!(window.try_find("paste-browsers").is_none());
    });
    assert_eq!(
        app.shell.read_with(cx, |shell, _| shell.shown().clone()),
        Screen::Paste
    );

    app.click(cx, "paste-browser");
    app.in_window(cx, |window, cx| {
        window.click(("paste-browser-option", 2usize), cx);
    });
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find("paste-browser").label(),
            Some("Browser this session comes from: Brave")
        );
        assert!(
            window.try_find("paste-browsers").is_none(),
            "chosen and closed"
        );
    });

    app.system.put_on_clipboard(SESSION_ID);
    app.click(cx, "paste-paste");
    app.click(cx, "paste-submit");
    app.until_screen(cx, |screen| matches!(screen, Screen::Welcome(_)))
        .await;
    assert_eq!(backend.sent_as().as_deref(), Some("Brave"));
}

#[gpui_kit::test]
async fn no_browser_found_is_said_under_the_field(cx: &mut TestAppContext) {
    let backend = Scripted::with(Vec::new());
    backend.script(|script| {
        script.browsers(&[], None);
    });
    let app = on_the_paste_screen(cx, backend).await;
    cx.run_until_parked();

    assert_eq!(
        app.label_of(cx, "paste-browser"),
        "No browser snob can use was found"
    );
}
