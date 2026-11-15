//! The window, driven in process: the real views on a backend that answers
//! what the test told it to and a system the test speaks for. No engine, no
//! Instagram and no display: what is checked is what a person or a screen
//! reader would find, by name and by role.

#![cfg(feature = "testing")]

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, Role, TestAppContext, Window, px, size};
use snob_desktop::backend::SignIn;
use snob_desktop::backend::scripted::{Asked, Scripted, account};
use snob_desktop::state::flow::{Method, Screen, Update, Updating};
use snob_desktop::state::status::Status;
use snob_desktop::system::simulated::Simulated;
use snob_desktop::system::{Event, Settings};
use snob_desktop::theme::tokens::{Palette, Scheme};
use snob_desktop::ui;
use snob_desktop::ui::look::Look;
use snob_desktop::ui::shell::Shell;
use snob_engine::login::Failure;

/// How long a test waits for the window to get somewhere, on the test's own
/// clock: longer than every wait the app makes by itself.
const SOON: Duration = Duration::from_secs(20);

struct App {
    window: AnyWindowHandle,
    shell: Entity<Shell>,
    backend: Scripted,
    system: Simulated,
}

fn open_with(cx: &mut TestAppContext, backend: Scripted, settings: Settings) -> App {
    let system = Simulated::new(settings);
    let (window, shell) = cx.update(|cx| {
        ui::window::init(&system, true, cx);
        ui::window::open(Arc::new(backend.clone()), Rc::new(system.clone()), None, cx)
            .expect("the window opens")
    });
    App {
        window,
        shell,
        backend,
        system,
    }
}

fn open(cx: &mut TestAppContext, backend: Scripted) -> App {
    open_with(cx, backend, Settings::default())
}

impl App {
    /// Waits until the screen shown is one `wanted` takes.
    async fn until_screen(&self, cx: &mut TestAppContext, wanted: impl Fn(&Screen) -> bool) {
        let shell = self.shell.clone();
        cx.wait_for(self.window, SOON, move |_, cx| {
            wanted(shell.read(cx).shown())
        })
        .await;
    }

    /// Waits until `wanted` holds of the window.
    async fn until(
        &self,
        cx: &mut TestAppContext,
        wanted: impl FnMut(&mut Window, &mut gpui_kit::App) -> bool,
    ) {
        cx.wait_for(self.window, SOON, wanted).await;
    }

    fn in_window<R>(
        &self,
        cx: &mut TestAppContext,
        act: impl FnOnce(&mut Window, &mut gpui_kit::App) -> R,
    ) -> R {
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            act(window, cx)
        })
        .expect("the window is open")
    }

    fn click(&self, cx: &mut TestAppContext, id: &'static str) {
        self.in_window(cx, |window, cx| window.click(id, cx));
    }

    fn press(&self, cx: &mut TestAppContext, key: &'static str) {
        self.in_window(cx, |window, cx| window.press(key, cx));
    }

    /// The title of the screen shown, as a screen reader is told it, and
    /// whether focus is on it.
    fn title(&self, cx: &mut TestAppContext) -> (String, bool) {
        self.in_window(cx, |window, _| {
            let title = window.find("title");
            assert_eq!(title.role(), Some(Role::Heading));
            (
                title.label().unwrap_or_default().to_string(),
                title.focused() == Some(true),
            )
        })
    }

    fn label_of(&self, cx: &mut TestAppContext, id: &'static str) -> String {
        self.in_window(cx, |window, _| {
            window.find(id).label().unwrap_or_default().to_string()
        })
    }
}

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
        let picture = window.find("account");
        assert_eq!(picture.label(), Some("Profile picture of @me"));
        assert!(picture.visible());
    });
}

#[gpui_kit::test]
async fn the_frame_is_the_same_on_every_screen_and_says_it_is_a_test_build(
    cx: &mut TestAppContext,
) {
    let app = open(cx, Scripted::with(Vec::new()));
    let framed = |window: &mut Window| {
        let band = window.find("band");
        let panel = window.find("panel");
        assert_eq!(band.role(), Some(Role::Banner));
        assert_eq!(panel.role(), Some(Role::Main));
        assert_eq!(band.bounds().size.height, px(42.));
        assert_eq!(
            panel.bounds().origin.y,
            px(42.),
            "the panel starts under the band"
        );
        assert_eq!(window.find("brand").label(), Some("snob."));
        assert_eq!(window.find("test-badge").label(), Some("TEST"));
    };
    app.in_window(cx, |window, _| framed(window));
    app.until_screen(cx, start()).await;
    app.in_window(cx, |window, _| framed(window));
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
    let focused = |app: &App, cx: &mut TestAppContext, id: &'static str| {
        app.in_window(cx, |window, _| window.find(id).focused() == Some(true))
    };

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
    assert_eq!(app.label_of(cx, "account"), "Profile picture of @me");
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
