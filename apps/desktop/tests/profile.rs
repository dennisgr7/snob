//! A profile page and the lists it opens, driven in process on the scripted
//! backend: what a person or a screen reader finds, and **what was asked of
//! the backend, and when**: another account's list only after a yes, and
//! nothing of a story ever.

#![cfg(feature = "testing")]

mod common;

use std::time::{Duration, Instant};

use common::{App, disabled, focused, open};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, PlatformInput, Role, ScrollDelta, ScrollEvent, TestAppContext,
    VisualTestContext, point, px, size,
};
use snob_core::{Epoch, Pk};
use snob_desktop::backend::scripted::{Asked, Scripted, account};
use snob_desktop::state::flow::Screen;
use snob_desktop::state::profile::{self, Side, Tab};
use snob_desktop::state::rest::QUIET_FOR;
use snob_desktop::system::{Event, Settings};
use snob_desktop::theme::tokens::frame;

/// The app at home, signed in as `me` (42).
async fn at_home(cx: &mut TestAppContext, backend: Scripted) -> App {
    let app = open(cx, backend);
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app
}

/// The profile of the account in use, opened from its menu.
async fn own_profile(cx: &mut TestAppContext, backend: Scripted) -> App {
    let app = at_home(cx, backend).await;
    app.click(cx, "account");
    app.click(cx, "menu-profile");
    app.until_screen(cx, |screen| matches!(screen, Screen::Profile(_)))
        .await;
    app.until(cx, |window, _| {
        window.try_find(("profile-tile", 0usize)).is_some()
    })
    .await;
    app
}

fn shown(app: &App, cx: &mut TestAppContext, id: &'static str) -> bool {
    app.in_window(cx, |window, _| {
        window.try_find(id).is_some_and(|found| found.visible())
    })
}

fn me() -> Scripted {
    Scripted::with(vec![account(42, "me", true)])
}

#[gpui_kit::test]
async fn the_account_s_own_profile_opens_from_its_menu_with_its_names_counters_and_posts(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, me()).await;

    assert_eq!(app.title(cx), ("Invented me (@me)".to_string(), true));
    app.in_window(cx, |window, _| {
        let picture = window.find("profile-picture");
        assert_eq!(picture.role(), Some(Role::Button));
        assert_eq!(picture.label(), Some("Open the profile picture of @me"));
        assert_eq!(
            window.find("profile-followers").label(),
            Some("1,234 followers, open the list")
        );
        assert_eq!(
            window.find("profile-following").label(),
            Some("567 following, open the list")
        );
        let posts = window.find(("profile-tab", 0usize));
        assert_eq!(posts.role(), Some(Role::Tab));
        assert_eq!(posts.label(), Some("Posts · 48"));
        assert_eq!(posts.selected(), Some(true));
        assert_eq!(
            window.find(("profile-tab", 2usize)).label(),
            Some("Mentioned in")
        );
        assert_eq!(
            window.find(("profile-highlight", 0usize)).role(),
            Some(Role::Image)
        );
        assert_eq!(
            window.find(("profile-tile", 0usize)).label(),
            Some("Photo, pinned: an invented caption, number 0")
        );
        assert_eq!(window.find("profile-options").label(), Some("More options"));
        assert_eq!(window.find("band-back").label(), Some("Back"));
    });
    // The profile and the first page of the tab shown: nothing else.
    let asked: Vec<Asked> = app
        .backend
        .asked()
        .into_iter()
        .filter(|asked| matches!(asked, Asked::Profile(_) | Asked::Tiles(..)))
        .collect();
    assert_eq!(
        asked,
        [
            Asked::Profile(Pk::new(42)),
            Asked::Tiles(Pk::new(42), Tab::Posts)
        ]
    );

    app.click(cx, "band-back");
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
}

#[gpui_kit::test]
async fn a_tab_is_read_once_when_it_is_first_shown(cx: &mut TestAppContext) {
    let app = own_profile(cx, me()).await;
    app.in_window(cx, |window, cx| window.click(("profile-tab", 1usize), cx));
    app.until(cx, |window, _| {
        window
            .try_find(("profile-tile", 0usize))
            .and_then(|tile| tile.label().map(|label| label.contains("@user0")))
            .unwrap_or(false)
    })
    .await;
    app.in_window(cx, |window, _| {
        assert_eq!(window.find(("profile-tab", 1usize)).selected(), Some(true));
        assert_eq!(window.find(("profile-tab", 0usize)).selected(), Some(false));
    });
    app.in_window(cx, |window, cx| window.click(("profile-tab", 2usize), cx));
    app.until(cx, |window, _| window.try_find("profile-empty").is_some())
        .await;
    app.in_window(cx, |window, cx| window.click(("profile-tab", 1usize), cx));
    let reposts = app
        .backend
        .asked()
        .into_iter()
        .filter(|asked| *asked == Asked::Tiles(Pk::new(42), Tab::Reposts))
        .count();
    assert_eq!(reposts, 1, "a tab shown again is not read again");
}

#[gpui_kit::test]
async fn a_profile_seen_before_shows_what_was_kept_at_once_while_the_band_says_it_loads(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, me()).await;
    assert_eq!(app.label_of(cx, "status"), "", "read: nothing loads");
    app.click(cx, "band-back");
    app.until_screen(cx, |screen| *screen == Screen::Home).await;

    app.backend.hold();
    app.click(cx, "account");
    app.click(cx, "menu-profile");
    app.until_screen(cx, |screen| matches!(screen, Screen::Profile(_)))
        .await;
    app.until(cx, |window, _| {
        window.try_find(("profile-tile", 0usize)).is_some()
    })
    .await;
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find(("profile-tile", 0usize)).label(),
            Some("Photo, pinned: an invented caption, number 0")
        );
        assert!(window.try_find(("profile-highlight", 0usize)).is_some());
        assert!(window.try_find("profile-reading").is_none(), "no ring");
    });
    assert_eq!(app.label_of(cx, "status"), "Loading the profile\u{2026}");

    // The reads it makes anyway are asked all the same, and answer.
    app.backend.release();
    app.until(cx, |window, _| {
        window
            .try_find("status")
            .is_some_and(|status| status.label().unwrap_or_default().is_empty())
    })
    .await;
    let tiles = app
        .backend
        .asked()
        .into_iter()
        .filter(|asked| *asked == Asked::Tiles(Pk::new(42), Tab::Posts))
        .count();
    assert_eq!(tiles, 2, "read again on the second visit");
}

#[gpui_kit::test]
async fn the_layout_and_the_way_posts_are_shown_change_from_their_buttons(cx: &mut TestAppContext) {
    let app = own_profile(cx, me()).await;
    assert_eq!(
        app.label_of(cx, "profile-layout"),
        "Put the card beside the posts"
    );
    assert!(!shown(&app, cx, "profile-posts"), "the page scrolls whole");
    app.click(cx, "profile-layout");
    assert_eq!(
        app.label_of(cx, "profile-layout"),
        "Put the card above the posts"
    );
    assert!(shown(&app, cx, "profile-posts"), "the posts scroll alone");

    app.in_window(cx, |window, _| {
        assert_eq!(window.find("profile-grid").checked(), Some(true));
        assert!(window.try_find("profile-grid-posts").is_some());
    });
    app.click(cx, "profile-list");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("profile-list").checked(), Some(true));
        assert!(window.try_find("profile-list-posts").is_some());
        assert!(window.try_find("profile-grid-posts").is_none());
    });
}

#[gpui_kit::test]
async fn the_own_list_opens_at_once_and_its_rows_come_as_they_are_read(cx: &mut TestAppContext) {
    let backend = me();
    backend.script(|script| {
        script.list_size(40);
    });
    let app = own_profile(cx, backend.clone()).await;
    told_it_is(SAME_DAY_AS_THE_SCAN, cx);
    backend.hold_the_work();
    app.click(cx, "profile-followers");
    app.until_screen(cx, |screen| matches!(screen, Screen::People { .. }))
        .await;
    assert!(
        !shown(&app, cx, "list-question"),
        "the own list asks nothing"
    );
    assert_eq!(app.title(cx), ("Followers of @me".to_string(), true));

    // The first page is there while the rest is read.
    app.until(cx, |window, _| {
        window.try_find(("person", 0usize)).is_some()
    })
    .await;
    assert_eq!(app.label_of(cx, "people-progress"), "Reading… 12 of 40");
    app.in_window(cx, |window, _| {
        let row = window.find(("person", 0usize));
        assert_eq!(row.role(), Some(Role::Button));
        assert_eq!(row.label(), Some("Open the profile of @user0"));
        let options = window.find("people-options");
        assert_eq!(options.label(), Some("Options"));
        assert_eq!(options.expanded(), Some(false));
    });
    assert!(
        backend
            .asked()
            .contains(&Asked::List(Pk::new(42), Side::Followers))
    );

    backend.release();
    app.until(cx, |window, _| {
        window.try_find("people-progress").is_some_and(|progress| {
            progress
                .label()
                .is_some_and(|said| said.starts_with("40 accounts."))
        })
    })
    .await;
    assert_eq!(
        app.label_of(cx, "people-progress"),
        "40 accounts. Last scan at 14:13."
    );

    // A row opens its account's profile.
    app.in_window(cx, |window, cx| window.click(("person", 3usize), cx));
    app.until_screen(
        cx,
        |screen| matches!(screen, Screen::Profile(who) if who.pk == Pk::new(1003)),
    )
    .await;
}

#[gpui_kit::test]
async fn another_account_s_list_is_asked_about_first_and_a_no_reads_nothing(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, me()).await;
    app.click(cx, "profile-following");
    app.until(cx, |window, _| {
        window.try_find(("person", 0usize)).is_some()
    })
    .await;
    app.in_window(cx, |window, cx| window.click(("person", 0usize), cx));
    app.until_screen(
        cx,
        |screen| matches!(screen, Screen::Profile(who) if who.pk == Pk::new(1000)),
    )
    .await;
    app.until(cx, |window, _| {
        window.try_find(("profile-tile", 0usize)).is_some()
    })
    .await;
    let theirs = |app: &App| {
        app.backend
            .asked()
            .into_iter()
            .any(|asked| matches!(asked, Asked::List(pk, _) if pk == Pk::new(1000)))
    };

    app.click(cx, "profile-followers");
    app.in_window(cx, |window, _| {
        let question = window.find("list-question");
        assert_eq!(question.role(), Some(Role::AlertDialog));
        assert_eq!(question.label(), Some("Read the followers of @user0?"));
        assert!(question.description().is_some());
    });
    assert!(focused(&app, cx, "list-question-cancel"), "focus is on no");
    assert!(!theirs(&app), "nothing is read before the answer");

    app.press(cx, "escape");
    assert!(!shown(&app, cx, "list-question"));
    assert!(!theirs(&app), "a no reads nothing");
    assert!(matches!(
        app.shell.read_with(cx, |shell, _| shell.shown().clone()),
        Screen::Profile(_)
    ));

    app.click(cx, "profile-followers");
    app.click(cx, "list-question-confirm");
    app.until_screen(cx, |screen| {
        matches!(screen, Screen::People { owner, side: Side::Followers } if owner.pk == Pk::new(1000))
    })
    .await;
    // Read once what was stored of it is shown, which here is nothing.
    app.until(cx, |window, _| {
        window.try_find(("person", 0usize)).is_some()
    })
    .await;
    assert!(theirs(&app), "a yes reads it");

    // Back the way it came: the profile, the list, the own profile.
    app.click(cx, "band-back");
    app.until_screen(
        cx,
        |screen| matches!(screen, Screen::Profile(who) if who.pk == Pk::new(1000)),
    )
    .await;
}

#[gpui_kit::test]
async fn the_picture_opens_large_is_copied_and_closes_on_escape(cx: &mut TestAppContext) {
    let app = own_profile(cx, me()).await;
    app.click(cx, "profile-picture");
    // Its buttons are there once the picture has arrived.
    app.until(cx, |window, _| window.try_find("viewer-close").is_some())
        .await;
    app.in_window(cx, |window, _| {
        let viewer = window.find("photo-viewer");
        assert_eq!(viewer.role(), Some(Role::Dialog));
        assert_eq!(viewer.label(), Some("Profile picture of @me, large"));
        for (id, label) in [
            ("viewer-download", "Save to Downloads"),
            ("viewer-copy", "Copy picture"),
            ("viewer-close", "Close the picture"),
        ] {
            assert_eq!(window.find(id).label(), Some(label));
        }
    });
    assert!(
        app.backend
            .asked()
            .contains(&Asked::FullPicture(Pk::new(42)))
    );

    app.click(cx, "viewer-copy");
    let system = app.system.clone();
    app.until(cx, move |_, _| system.copied_image().is_some())
        .await;

    app.press(cx, "escape");
    assert!(!shown(&app, cx, "photo-viewer"));
    assert!(focused(&app, cx, "profile-picture"), "focus is back");
}

/// Where the picture fits in its viewer in `window`, as left, top, width
/// and height.
fn fit_in(window: &gpui_kit::Window) -> (f32, f32, f32, f32) {
    let size = window.viewport_size();
    let place = profile::viewer_place(f32::from(size.width), f32::from(size.height), frame::BAND);
    (place.x, place.y, place.w, place.h)
}

fn edges(bounds: gpui_kit::Bounds<gpui_kit::Pixels>) -> (f32, f32, f32, f32) {
    (
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
    )
}

#[gpui_kit::test]
async fn the_picture_grows_from_the_page_into_its_viewer_and_goes_back_with_focus_where_it_was(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, me()).await;
    let me = Pk::new(42);
    let page = app.in_window(cx, |window, _| {
        edges(window.find("profile-picture").bounds())
    });
    let clicked = Instant::now();
    app.click(cx, "profile-picture");
    let (first, fit, buttons, away) = app.in_window(cx, |window, cx| {
        (
            edges(window.find("viewer-picture").bounds()),
            fit_in(window),
            window.try_find("viewer-close").is_some(),
            app.shell.read(cx).picture_is_away(me),
        )
    });
    // On its way: every edge between the page's and the viewer's, and the
    // page's own picture left out meanwhile.
    let ends = [
        (first.0, page.0, fit.0),
        (first.1, page.1, fit.1),
        (first.2, page.2 - 12.0, fit.2),
        (first.3, page.3 - 12.0, fit.3),
    ];
    for (now, from, to) in ends {
        assert!(
            from.min(to) - 1.0 <= now && now <= from.max(to) + 1.0,
            "{first:?} between {page:?} and {fit:?}"
        );
    }
    assert!(away, "the page's own picture is not drawn twice");
    // Not there yet, and nothing to press, unless this machine took most
    // of the move to draw one frame.
    if clicked.elapsed() < Duration::from_millis(150) {
        assert!(first.2 < fit.2 - 1.0, "{first:?} has not arrived");
        assert!(!buttons, "no button before it has arrived");
    }
    assert!(
        !focused(&app, cx, "profile-picture"),
        "focus is in the viewer"
    );

    // It arrives where it fits, exactly, and its buttons come.
    app.until(cx, |window, _| {
        let now = edges(window.find("viewer-picture").bounds());
        let fit = fit_in(window);
        [now.0 - fit.0, now.1 - fit.1, now.2 - fit.2, now.3 - fit.3]
            .iter()
            .all(|off| off.abs() < 0.5)
    })
    .await;
    app.until(cx, |window, _| window.try_find("viewer-close").is_some())
        .await;

    // Closed, the viewer is gone at once and focus is back; the picture
    // alone goes back to the page, and then nothing is left over it.
    let pressed = Instant::now();
    app.press(cx, "escape");
    let (viewer, leaving, away) = app.in_window(cx, |window, cx| {
        (
            window
                .try_find("photo-viewer")
                .is_some_and(|found| found.visible()),
            window
                .try_find("viewer-leaving")
                .is_some_and(|found| found.visible()),
            app.shell.read(cx).picture_is_away(me),
        )
    });
    assert!(!viewer, "the viewer is gone at once");
    assert!(leaving, "on its way back");
    if pressed.elapsed() < Duration::from_millis(150) {
        assert!(away, "the page's own picture is not drawn twice");
    }
    assert!(focused(&app, cx, "profile-picture"), "focus is back");
    app.until(cx, |window, _| {
        window
            .try_find("viewer-leaving")
            .is_none_or(|found| !found.visible())
    })
    .await;
    assert!(
        !app.shell
            .read_with(cx, |shell, _| shell.picture_is_away(me)),
        "the page draws its own picture again"
    );
    assert!(focused(&app, cx, "profile-picture"));
}

#[gpui_kit::test]
async fn under_less_motion_the_picture_does_not_move_and_fades_in_where_it_fits(
    cx: &mut TestAppContext,
) {
    let settings = Settings {
        reduce_motion: true,
        ..Settings::default()
    };
    let app = common::open_with(cx, me(), settings);
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    app.click(cx, "account");
    app.click(cx, "menu-profile");
    app.until(cx, |window, _| {
        window.try_find(("profile-tile", 0usize)).is_some()
    })
    .await;
    app.click(cx, "profile-picture");
    app.in_window(cx, |window, _| {
        let now = edges(window.find("viewer-picture").bounds());
        let fit = fit_in(window);
        assert!(
            [now.0 - fit.0, now.1 - fit.1, now.2 - fit.2, now.3 - fit.3]
                .iter()
                .all(|off| off.abs() < 0.5),
            "{now:?} where it fits, {fit:?}, from the first frame"
        );
    });
    app.press(cx, "escape");
    assert!(!shown(&app, cx, "photo-viewer"));
    assert!(focused(&app, cx, "profile-picture"), "focus is back");
}

#[gpui_kit::test]
async fn a_story_up_offers_the_story_or_the_picture_and_reads_nothing_of_the_story(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, Scripted::with(vec![account(9001, "someone", true)])).await;
    assert_eq!(
        app.label_of(cx, "profile-picture"),
        "Profile picture of @someone, with a story"
    );
    app.click(cx, "profile-picture");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("profile-menu").role(), Some(Role::Menu));
        assert_eq!(window.find("profile-picture").expanded(), Some(true));
        assert_eq!(
            window.find("profile-open-story").role(),
            Some(Role::MenuItem)
        );
    });
    let before = app.backend.asked().len();
    app.click(cx, "profile-open-story");
    assert!(!shown(&app, cx, "profile-menu"));
    assert!(
        app.label_of(cx, "announcements")
            .contains("Nothing was read or marked as seen")
    );
    assert_eq!(app.backend.asked().len(), before, "nothing was asked");

    app.click(cx, "profile-picture");
    app.click(cx, "profile-open-picture");
    assert!(shown(&app, cx, "photo-viewer"));
}

#[gpui_kit::test]
async fn each_menu_opens_under_what_opened_it_in_both_layouts(cx: &mut TestAppContext) {
    let app = own_profile(cx, Scripted::with(vec![account(9001, "someone", true)])).await;
    for _ in 0..2 {
        app.click(cx, "profile-options");
        app.in_window(cx, |window, _| {
            let button = window.find("profile-options").bounds();
            let menu = window.find("profile-menu").bounds();
            assert!(
                (menu.right() - button.right()).abs() <= px(1.),
                "{menu:?} ends where {button:?} does"
            );
            assert!(menu.top() >= button.bottom(), "{menu:?} under {button:?}");
        });
        app.press(cx, "escape");

        app.click(cx, "profile-picture");
        app.in_window(cx, |window, _| {
            let picture = window.find("profile-picture").bounds();
            let menu = window.find("profile-menu").bounds();
            assert!(
                (menu.left() - picture.left()).abs() <= px(1.),
                "{menu:?} starts where {picture:?} does"
            );
            assert!(menu.top() >= picture.bottom(), "{menu:?} under {picture:?}");
        });
        app.press(cx, "escape");
        app.click(cx, "profile-layout");
    }
}

/// 2026-09-21 at 17:00 and 2026-10-01 at 14:13, UTC as a test build says
/// it: the afternoon of the scripted scan (14:13 that day), and ten days on.
const SAME_DAY_AS_THE_SCAN: i64 = 1_790_010_000;
const TEN_DAYS_AFTER_THE_SCAN: i64 = 1_790_864_000;

/// From now on every screen's words are said against `at`.
fn told_it_is(at: i64, cx: &mut TestAppContext) {
    cx.update(|cx| snob_desktop::ui::now::fix_now(Epoch::new(at), cx));
}

/// The options' items, in the order they are shown and walked.
const OPTIONS: [&str; 5] = [
    "profile-open-browser",
    "profile-about-open",
    "profile-copy-username",
    "profile-copy-link",
    "profile-copy-id",
];

#[gpui_kit::test]
async fn the_options_are_the_browser_about_and_three_copies_in_that_order_in_both_languages(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, me()).await;
    for (locale, words, unread) in [
        (
            "en",
            [
                "Open in the browser",
                "About this account",
                "Copy username",
                "Copy profile link",
                "Copy account ID",
            ],
            "Not read yet",
        ),
        (
            "es",
            [
                "Abrir en el navegador",
                "Informaci\u{f3}n sobre esta cuenta", // i18n-allow
                "Copiar nombre de usuario",           // i18n-allow
                "Copiar enlace del perfil",
                "Copiar ID de la cuenta", // i18n-allow
            ],
            "A\u{fa}n no se lee",
        ),
    ] {
        app.system.say(Event::Settings(Settings {
            locale: locale.to_string(),
            ..Settings::default()
        }));
        cx.run_until_parked();
        app.click(cx, "profile-options");
        app.in_window(cx, |window, _| {
            let mut above = None;
            for (id, said) in OPTIONS.into_iter().zip(words) {
                let item = window.find(id);
                assert_eq!(item.role(), Some(Role::MenuItem), "{locale}: {id}");
                assert_eq!(item.label(), Some(said), "{locale}: {id}");
                let top = item.bounds().top();
                if let Some(above) = above {
                    assert!(top > above, "{locale}: {id} under the item before it");
                }
                above = Some(top);
            }
        });

        // The arrows walk them in that order, around the ends.
        for id in OPTIONS {
            app.press(cx, "down");
            assert!(focused(&app, cx, id), "{locale}: down to {id}");
        }
        app.press(cx, "down");
        assert!(focused(&app, cx, OPTIONS[0]), "{locale}: around the end");
        app.press(cx, "up");
        assert!(
            focused(&app, cx, OPTIONS[4]),
            "{locale}: and back around it"
        );
        // Tab walks them too, and never leaves the menu.
        app.press(cx, "tab");
        assert!(
            focused(&app, cx, OPTIONS[0]),
            "{locale}: Tab around the end"
        );
        app.press(cx, "shift-tab");
        assert!(
            focused(&app, cx, OPTIONS[4]),
            "{locale}: Shift+Tab back around it"
        );

        app.click(cx, "profile-about-open");
        for at in 0..3usize {
            assert_eq!(
                app.in_window(cx, |window, _| {
                    window
                        .find(("profile-about-value", at))
                        .label()
                        .unwrap_or_default()
                        .to_string()
                }),
                unread,
                "{locale}: row {at}"
            );
        }
        app.press(cx, "escape");
        assert!(!shown(&app, cx, "profile-about"), "{locale}");
    }
}

#[gpui_kit::test]
async fn the_options_open_the_profile_in_the_person_s_browser_asking_nothing(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, me()).await;
    let before = app.backend.asked().len();
    app.click(cx, "profile-options");
    app.click(cx, "profile-open-browser");
    assert_eq!(app.system.links(), ["https://www.instagram.com/me/"]);
    assert!(
        app.label_of(cx, "announcements")
            .contains("Opened in your browser")
    );
    assert!(!shown(&app, cx, "profile-menu"));
    assert!(focused(&app, cx, "profile-options"), "focus is back");
    assert_eq!(app.backend.asked().len(), before, "nothing was asked");
}

#[gpui_kit::test]
async fn a_name_that_cannot_be_a_username_is_neither_opened_nor_copied_as_a_link(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, Scripted::with(vec![account(42, "not a name", true)])).await;
    app.click(cx, "profile-options");
    assert!(disabled(&app, cx, "profile-open-browser"));
    assert!(disabled(&app, cx, "profile-copy-link"));
    app.click(cx, "profile-open-browser");
    app.click(cx, "profile-copy-link");
    assert_eq!(app.system.clipboard(), None);
    let said = app.label_of(cx, "announcements");
    assert!(!said.contains("Opened in your browser"), "{said}");
    assert!(!said.contains("Profile link copied"), "{said}");
}

#[gpui_kit::test]
async fn the_options_copy_the_username_the_link_and_the_id_and_say_so(cx: &mut TestAppContext) {
    let app = own_profile(cx, me()).await;
    app.click(cx, "profile-options");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("profile-menu").label(), Some("Options"));
        assert_eq!(window.find("profile-options").expanded(), Some(true));
    });
    app.press(cx, "escape");
    let before = app.backend.asked().len();
    for (id, copied, said) in [
        ("profile-copy-username", "me", "Username copied"),
        (
            "profile-copy-link",
            "https://www.instagram.com/me/",
            "Profile link copied",
        ),
        ("profile-copy-id", "42", "Account ID copied"),
    ] {
        app.click(cx, "profile-options");
        app.click(cx, id);
        assert_eq!(app.system.clipboard().as_deref(), Some(copied), "{id}");
        assert!(app.label_of(cx, "announcements").contains(said), "{id}");
        assert!(!shown(&app, cx, "profile-menu"), "{id}");
        assert!(focused(&app, cx, "profile-options"), "{id}: focus is back");
    }
    assert_eq!(app.backend.asked().len(), before, "nothing was asked");
}

#[gpui_kit::test]
async fn about_this_account_says_what_snob_does_not_read_yet_and_closes_on_escape(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, me()).await;
    let before = app.backend.asked().len();
    app.click(cx, "profile-options");
    app.click(cx, "profile-about-open");
    assert!(!shown(&app, cx, "profile-menu"));
    app.in_window(cx, |window, _| {
        let dialog = window.find("profile-about");
        assert_eq!(dialog.role(), Some(Role::Dialog));
        assert_eq!(dialog.label(), Some("About this account"));
        let heading = window.find("profile-about-title");
        assert_eq!(heading.role(), Some(Role::Heading));
        assert_eq!(heading.label(), Some("About this account"));
        assert_eq!(window.find("profile-about-username").label(), Some("@me"));
        let rows: Vec<_> = (0..3usize)
            .map(|at| {
                let label = window.find(("profile-about-label", at));
                let value = window.find(("profile-about-value", at));
                (
                    label.label().unwrap_or_default().to_string(),
                    value.label().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("Date joined", "Not read yet"),
                ("Account based in", "Not read yet"),
                ("Username changes", "Not read yet"),
            ]
            .map(|(label, value)| (label.to_string(), value.to_string()))
        );
        assert!(window.try_find(("profile-about-value", 3usize)).is_none());
    });
    // The page keeps its one title; the dialog's heading is under it.
    assert_eq!(app.title(cx).0, "Invented me (@me)");
    assert!(focused(&app, cx, "profile-about-close"), "focus is in it");
    // Tab never leaves it: its one control keeps focus.
    app.press(cx, "tab");
    assert!(focused(&app, cx, "profile-about-close"), "Tab stays in it");
    app.press(cx, "shift-tab");
    assert!(focused(&app, cx, "profile-about-close"), "Shift+Tab too");

    app.press(cx, "escape");
    assert!(!shown(&app, cx, "profile-about"));
    assert!(focused(&app, cx, "profile-options"), "focus is back");

    app.click(cx, "profile-options");
    app.click(cx, "profile-about-open");
    app.click(cx, "profile-about-close");
    assert!(!shown(&app, cx, "profile-about"));
    assert!(focused(&app, cx, "profile-options"), "focus is back");
    assert_eq!(app.backend.asked().len(), before, "nothing was asked");
}

/// The bounds of `id`, drawn.
fn bounds_of(
    app: &App,
    cx: &mut TestAppContext,
    id: impl Into<gpui_kit::ElementId>,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let id = id.into();
    app.in_window(cx, move |window, _| window.find(id).bounds())
}

#[gpui_kit::test]
async fn the_bar_is_one_row_of_the_view_in_words_its_buttons_and_the_options_beside_the_names(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, me()).await;
    for layout in ["stacked", "split"] {
        // One frame: the page may still be coming in.
        let (row, picture, bar, title) = app.in_window(cx, |window, _| {
            (
                [
                    "profile-showing-words",
                    "profile-grid",
                    "profile-list",
                    "profile-layout",
                    "profile-options",
                ]
                .map(|id| (id, window.find(id).bounds())),
                window.find("profile-picture").bounds(),
                window.find("profile-bar").bounds(),
                window.find("title").bounds(),
            )
        });
        let middle =
            |bounds: &gpui_kit::Bounds<gpui_kit::Pixels>| (bounds.top() + bounds.bottom()) / 2.0;
        let (first, first_bounds) = row[0];
        for pair in row.windows(2) {
            let ((left, left_bounds), (right, right_bounds)) = (pair[0], pair[1]);
            assert!(
                left_bounds.right() <= right_bounds.left(),
                "{layout}: {left} {left_bounds:?} before {right} {right_bounds:?}"
            );
            assert!(
                (middle(&right_bounds) - middle(&first_bounds)).abs() <= px(2.),
                "{layout}: {right} {right_bounds:?} on the line of {first} {first_bounds:?}"
            );
        }
        match layout {
            "stacked" => assert!(
                bar.top() <= title.bottom()
                    && bar.bottom() >= title.top()
                    && bar.left() >= title.right(),
                "the bar is on the line of the names, after them: {bar:?} beside {title:?}"
            ),
            // Over the posts alone, so the card starts at the page's top.
            _ => assert!(
                bar.left() >= picture.right() && bar.top() <= picture.top(),
                "the bar is beside the card, from its top: {bar:?} beside {picture:?}"
            ),
        }
        app.click(cx, "profile-layout");
    }
}

#[gpui_kit::test]
async fn the_words_beside_the_buttons_say_how_the_posts_are_shown(cx: &mut TestAppContext) {
    for (locale, grid, list) in [
        ("en", "Grid view", "List view"),
        ("es", "Vista en cuadrícula", "Vista en lista"), // i18n-allow
    ] {
        let app = common::open_with(
            cx,
            me(),
            snob_desktop::system::Settings {
                locale: locale.to_string(),
                ..Default::default()
            },
        );
        app.until_screen(cx, |screen| *screen == Screen::Home).await;
        app.click(cx, "account");
        app.click(cx, "menu-profile");
        app.until(cx, |window, _| {
            window.try_find(("profile-tile", 0usize)).is_some()
        })
        .await;
        let wide = bounds_of(&app, cx, "profile-showing-words").size;
        assert_eq!(app.label_of(cx, "profile-showing-words"), grid);
        app.click(cx, "profile-list");
        assert_eq!(app.label_of(cx, "profile-showing-words"), list);
        assert_eq!(
            bounds_of(&app, cx, "profile-showing-words").size,
            wide,
            "{locale}: the words hold their room as they cross"
        );
        app.click(cx, "profile-grid");
        assert_eq!(app.label_of(cx, "profile-showing-words"), grid);
    }
}

#[gpui_kit::test]
async fn a_click_on_the_username_copies_it_and_says_so(cx: &mut TestAppContext) {
    let app = own_profile(cx, me()).await;
    app.in_window(cx, |window, _| {
        let username = window.find("profile-username");
        assert_eq!(username.role(), Some(Role::Button));
        assert_eq!(username.label(), Some("Copy @me"));
    });
    let before = app.backend.asked().len();
    app.click(cx, "profile-username");
    assert_eq!(app.system.clipboard().as_deref(), Some("me"));
    assert!(
        app.label_of(cx, "announcements")
            .contains("Username copied")
    );
    assert_eq!(app.backend.asked().len(), before, "nothing was asked");
}

#[gpui_kit::test]
async fn an_account_both_follow_is_marked_friend_and_not_follows_you(cx: &mut TestAppContext) {
    let app = own_profile(cx, me()).await;
    assert!(!shown(&app, cx, "profile-friend"), "the own profile");
    assert!(!shown(&app, cx, "profile-follows-you"), "the own profile");

    // user0 follows the account in use and is followed by it.
    app.click(cx, "profile-following");
    app.until(cx, |window, _| {
        window.try_find(("person", 0usize)).is_some()
    })
    .await;
    app.in_window(cx, |window, cx| window.click(("person", 0usize), cx));
    app.until(cx, |window, _| window.try_find("profile-friend").is_some())
        .await;
    app.in_window(cx, |window, _| {
        let friend = window.find("profile-friend");
        assert_eq!(friend.role(), Some(Role::Label));
        assert_eq!(friend.label(), Some("Friend"));
        assert!(window.try_find("profile-follows-you").is_none());
    });
}

#[gpui_kit::test]
async fn a_highlight_s_cover_is_an_upright_rectangle_its_picture_fills_at_every_text_size(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cover.png");
    // Far taller than the cover: what stretched a cover before.
    image::RgbaImage::from_pixel(10, 40, image::Rgba([200, 40, 120, 255]))
        .save(&file)
        .unwrap();
    for text_scale in [1.0, 2.0] {
        let backend = me();
        backend.script(|script| {
            script.highlight_cover(file.clone());
        });
        let app = common::open_with(
            cx,
            backend,
            snob_desktop::system::Settings {
                text_scale,
                ..Default::default()
            },
        );
        app.until_screen(cx, |screen| *screen == Screen::Home).await;
        app.click(cx, "account");
        app.click(cx, "menu-profile");
        app.until(cx, |window, _| {
            window
                .try_find(("profile-highlight-picture", 0usize))
                .is_some()
        })
        .await;
        cx.run_until_parked();
        let (cover, picture) = app.in_window(cx, |window, _| {
            (
                window.find(("profile-highlight-cover", 0usize)).bounds(),
                window.find(("profile-highlight-picture", 0usize)).bounds(),
            )
        });
        assert_eq!(
            (cover.size.width, cover.size.height),
            (px(72. * text_scale), px(100. * text_scale)),
            "at {text_scale}"
        );
        assert_eq!(
            picture, cover,
            "at {text_scale}: the picture fills its cover"
        );
    }
}

#[gpui_kit::test]
async fn a_post_s_thumbnail_fills_its_square_however_tall_its_picture_at_every_text_size(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("reel.png");
    // As tall as a reel's and taller: what a square must crop, not follow.
    image::RgbaImage::from_pixel(9, 32, image::Rgba([40, 120, 200, 255]))
        .save(&file)
        .unwrap();
    for text_scale in [1.0, 2.0] {
        let backend = me();
        backend.script(|script| {
            script.thumbnail(file.clone());
        });
        let app = common::open_with(
            cx,
            backend,
            snob_desktop::system::Settings {
                text_scale,
                ..Default::default()
            },
        );
        app.until_screen(cx, |screen| *screen == Screen::Home).await;
        app.click(cx, "account");
        app.click(cx, "menu-profile");
        app.until(cx, |window, _| {
            window.try_find(("profile-tile", 0usize)).is_some()
        })
        .await;
        // The list's square is its own size, so the grid's from the frame
        // before is not the one measured.
        let listed = px(64. * text_scale);
        for (layout, sized) in [("profile-grid", None), ("profile-list", Some(listed))] {
            app.click(cx, layout);
            app.until(cx, |window, _| {
                window.try_find(("profile-thumb-picture", 0usize)).is_some()
                    && sized.is_none_or(|width| {
                        window
                            .try_find(("profile-thumb", 0usize))
                            .is_some_and(|square| {
                                (square.bounds().size.width - width).abs() <= px(0.5)
                            })
                    })
            })
            .await;
            cx.run_until_parked();
            let (square, picture) = app.in_window(cx, |window, _| {
                (
                    window.find(("profile-thumb", 0usize)).bounds(),
                    window.find(("profile-thumb-picture", 0usize)).bounds(),
                )
            });
            assert!(
                (square.size.width - square.size.height).abs() <= px(0.5),
                "in {layout} at {text_scale}: {square:?} is a square"
            );
            assert_eq!(
                picture, square,
                "in {layout} at {text_scale}: the picture fills its square"
            );
        }
    }
}

#[gpui_kit::test]
async fn every_control_of_the_page_that_shows_no_words_is_named_and_at_least_24_square(
    cx: &mut TestAppContext,
) {
    let app = own_profile(cx, me()).await;
    for layout in ["stacked", "split"] {
        app.in_window(cx, |window, _| {
            for id in [
                "profile-picture",
                "profile-grid",
                "profile-list",
                "profile-layout",
                "profile-options",
                "profile-username",
            ] {
                let control = window.find(id);
                assert!(control.role().is_some(), "{layout}: {id} has no role");
                assert!(
                    !control.label().unwrap_or_default().is_empty(),
                    "{layout}: {id} has no name"
                );
                let size = control.bounds().size;
                assert!(
                    size.width >= px(24.) && size.height >= px(24.),
                    "{layout}: {id} is {size:?}"
                );
            }
        });
        app.click(cx, "profile-layout");
    }
}

/// The profile of the account in use in a window `width` by `height`.
async fn reading_on(cx: &mut TestAppContext, backend: Scripted, width: f32, height: f32) -> App {
    let app = at_home(cx, backend).await;
    cx.simulate_window_resize(app.window, size(px(width), px(height)));
    app.click(cx, "account");
    app.click(cx, "menu-profile");
    app.until(cx, |window, _| {
        window.try_find(("profile-tile", 0usize)).is_some()
    })
    .await;
    app
}

/// The account in use, its posts tab reading on for `pages` pages past its
/// first.
fn reading_on_for(pages: u32) -> Scripted {
    let backend = me();
    backend.script(|script| {
        script.more_pages(pages);
    });
    backend
}

/// The pages past the first asked for, in order, by their number.
fn pages_asked(backend: &Scripted) -> Vec<u32> {
    backend
        .asked()
        .into_iter()
        .filter_map(|asked| match asked {
            Asked::MoreTiles(pk, Tab::Posts, page) if pk == Pk::new(42) => Some(page),
            _ => None,
        })
        .collect()
}

/// Draws `count` frames, letting every answer in between arrive.
fn frames(app: &App, cx: &mut TestAppContext, count: usize) {
    for _ in 0..count {
        cx.run_until_parked();
        app.in_window(cx, |_, _| ());
    }
    cx.run_until_parked();
}

/// Turns the wheel far down over the window, low enough to be under the
/// highlights, which turn sideways.
fn scroll_to_the_end(app: &App, cx: &mut TestAppContext) {
    app.in_window(cx, |window, cx| {
        let seen = window.viewport_size();
        window.dispatch_event(
            PlatformInput::Scroll(ScrollEvent {
                position: point(seen.width * 0.5, seen.height * 0.9),
                delta: ScrollDelta::Pixels(point(px(0.), px(-100_000.))),
                ..ScrollEvent::default()
            }),
            cx,
        );
    });
}

fn found(app: &App, cx: &mut TestAppContext, id: &'static str) -> bool {
    app.in_window(cx, |window, _| window.try_find(id).is_some())
}

#[gpui_kit::test]
async fn posts_that_do_not_fill_the_window_read_on_a_page_at_a_time_to_the_tab_s_end(
    cx: &mut TestAppContext,
) {
    let backend = reading_on_for(3);
    let app = reading_on(cx, backend.clone(), 1000., 3000.).await;
    app.until(cx, |window, _| {
        window.try_find(("profile-tile", 47usize)).is_some()
    })
    .await;
    app.until(cx, |window, _| {
        window.try_find("profile-all-read").is_some()
    })
    .await;
    frames(&app, cx, 20);
    assert_eq!(
        pages_asked(&backend),
        [2, 3, 4],
        "in order, and none past the end"
    );
    app.in_window(cx, |window, _| {
        assert_eq!(
            window.find(("profile-tile", 12usize)).label(),
            Some("Photo: an invented caption, number 12")
        );
    });
    assert!(!found(&app, cx, "profile-more"));
}

#[gpui_kit::test]
async fn a_full_window_waits_and_scrolling_near_the_end_asks_for_one_page_in_a_list_too(
    cx: &mut TestAppContext,
) {
    let backend = reading_on_for(8);
    let app = reading_on(cx, backend.clone(), 1000., 700.).await;
    app.click(cx, "profile-list");
    frames(&app, cx, 30);
    let read = pages_asked(&backend).len();
    assert!(read < 8, "a full window reads no further: {read} pages");
    frames(&app, cx, 30);
    assert_eq!(pages_asked(&backend).len(), read, "nothing asked meanwhile");

    backend.hold_the_work();
    scroll_to_the_end(&app, cx);
    app.until(cx, |window, _| {
        window.try_find("profile-reading-more").is_some()
    })
    .await;
    for _ in 0..5 {
        scroll_to_the_end(&app, cx);
        frames(&app, cx, 5);
    }
    let asked = pages_asked(&backend);
    assert_eq!(
        asked.len(),
        read + 1,
        "one page, not one a frame: {asked:?}"
    );
    assert_eq!(
        app.label_of(cx, "profile-reading-more"),
        "Reading more posts\u{2026}"
    );

    backend.release();
    let last = (read + 2) * 12 - 1;
    app.until(cx, |window, _| {
        window.try_find(("profile-tile", last)).is_some()
            && window.try_find("profile-reading-more").is_none()
    })
    .await;
    assert!(found(&app, cx, "profile-list-posts"));
}

#[gpui_kit::test]
async fn a_page_that_failed_waits_for_its_button_which_asks_from_the_same_place(
    cx: &mut TestAppContext,
) {
    let backend = reading_on_for(3);
    backend.script(|script| {
        script.more_fails(1);
    });
    let app = reading_on(cx, backend.clone(), 1000., 3000.).await;
    app.until(cx, |window, _| {
        window.try_find("profile-more-again").is_some()
    })
    .await;
    assert_eq!(
        app.label_of(cx, "profile-more-failed"),
        "More posts could not be read."
    );
    assert_eq!(app.label_of(cx, "profile-more-again"), "Try again");
    frames(&app, cx, 30);
    assert_eq!(
        pages_asked(&backend),
        [2],
        "the app does not try again by itself"
    );

    app.click(cx, "profile-more-again");
    app.until(cx, |window, _| {
        window.try_find("profile-all-read").is_some()
    })
    .await;
    assert_eq!(pages_asked(&backend), [2, 2, 3, 4]);
    assert!(!found(&app, cx, "profile-more-failed"));
}

#[gpui_kit::test]
async fn a_rest_the_engine_owes_is_said_with_its_hour_and_nothing_else_is_asked_meanwhile(
    cx: &mut TestAppContext,
) {
    // Tomorrow at 14:35, said in UTC by a test build: a rest that has not
    // ended on the engine's clock, whatever the day the test runs.
    const DAY: i64 = 24 * 60 * 60;
    let tomorrow = (snob_core::clock::now().get() / DAY + 1) * DAY;
    let until = snob_core::Epoch::new(tomorrow + 14 * 60 * 60 + 35 * 60);
    let backend = reading_on_for(3);
    backend.script(|script| {
        script.rests_until(Some(until));
    });
    backend.hold_the_work();
    let app = reading_on(cx, backend.clone(), 1000., 3000.).await;
    app.until(cx, |window, _| window.try_find("profile-resting").is_some())
        .await;
    assert_eq!(
        app.label_of(cx, "profile-resting"),
        "snob rests until 14:35 before reading more, at a person's pace."
    );
    frames(&app, cx, 30);
    assert_eq!(pages_asked(&backend), [2], "nothing more while it rests");
    assert!(!found(&app, cx, "profile-reading-more"));

    backend.script(|script| {
        script.rests_until(None);
    });
    backend.release();
    app.until(cx, |window, _| {
        window.try_find("profile-all-read").is_some()
    })
    .await;
    assert_eq!(pages_asked(&backend), [2, 3, 4]);
    assert!(!found(&app, cx, "profile-resting"));
}

#[gpui_kit::test]
async fn leaving_the_profile_stops_the_page_on_its_way_and_nothing_more_is_asked(
    cx: &mut TestAppContext,
) {
    let backend = reading_on_for(3);
    backend.hold_the_work();
    let app = reading_on(cx, backend.clone(), 1000., 3000.).await;
    app.until(cx, |window, _| {
        window.try_find("profile-reading-more").is_some()
    })
    .await;
    assert!(!backend.more_stopped());
    app.click(cx, "band-back");
    app.until_screen(cx, |screen| *screen == Screen::Home).await;
    assert!(backend.more_stopped(), "the page on its way is stopped");
    backend.release();
    frames(&app, cx, 20);
    assert_eq!(pages_asked(&backend), [2]);
}

#[gpui_kit::test]
async fn nothing_is_read_on_while_the_window_is_not_looked_at(cx: &mut TestAppContext) {
    let backend = reading_on_for(3);
    let app = at_home(cx, backend.clone()).await;
    cx.simulate_window_resize(app.window, size(px(1000.), px(3000.)));
    VisualTestContext::from_window(app.window, cx).deactivate_window();
    app.click(cx, "account");
    app.click(cx, "menu-profile");
    app.until(cx, |window, _| {
        window.try_find(("profile-tile", 0usize)).is_some()
    })
    .await;
    frames(&app, cx, 30);
    assert_eq!(pages_asked(&backend), Vec::<u32>::new());

    app.in_window(cx, |window, _| window.activate_window());
    app.until(cx, |window, _| {
        window.try_find("profile-all-read").is_some()
    })
    .await;
    assert_eq!(pages_asked(&backend), [2, 3, 4]);
}

#[gpui_kit::test]
async fn a_page_stopped_by_leaving_its_tab_still_comes_and_the_tab_reads_on_after_it(
    cx: &mut TestAppContext,
) {
    let backend = reading_on_for(3);
    backend.hold_the_work();
    let app = reading_on(cx, backend.clone(), 1000., 3000.).await;
    app.until(cx, |window, _| {
        window.try_find("profile-reading-more").is_some()
    })
    .await;
    app.in_window(cx, |window, cx| window.click(("profile-tab", 1usize), cx));
    frames(&app, cx, 5);
    assert!(backend.more_stopped(), "the page on its way is stopped");

    // It came anyway, its cursor spent, as the engine's does while its
    // pictures load.
    backend.release();
    frames(&app, cx, 5);
    app.in_window(cx, |window, cx| window.click(("profile-tab", 0usize), cx));
    app.until(cx, |window, _| {
        window.try_find("profile-all-read").is_some()
    })
    .await;
    assert_eq!(pages_asked(&backend), [2, 3, 4], "no page asked twice");
    assert!(!found(&app, cx, "profile-more-failed"));
}

#[gpui_kit::test]
async fn a_full_window_asks_for_nothing_on_coming_back_from_a_shorter_tab(cx: &mut TestAppContext) {
    let backend = reading_on_for(8);
    let app = reading_on(cx, backend.clone(), 1000., 700.).await;
    frames(&app, cx, 30);
    let read = pages_asked(&backend).len();
    assert!(read < 8, "a full window reads no further: {read} pages");

    for _ in 0..3 {
        app.in_window(cx, |window, cx| window.click(("profile-tab", 2usize), cx));
        frames(&app, cx, 10);
        app.in_window(cx, |window, cx| window.click(("profile-tab", 0usize), cx));
        frames(&app, cx, 10);
    }
    assert_eq!(
        pages_asked(&backend).len(),
        read,
        "the other tab's sizes are not the posts'"
    );
}

#[gpui_kit::test]
async fn switching_account_lets_go_of_the_page_on_its_way_and_of_where_the_tab_read_on(
    cx: &mut TestAppContext,
) {
    let backend = Scripted::with(vec![
        account(42, "me", true),
        account(9001, "someone", false),
    ]);
    backend.script(|script| {
        script.more_pages(3);
    });
    backend.hold_the_work();
    let app = reading_on(cx, backend.clone(), 1000., 3000.).await;
    app.until(cx, |window, _| {
        window.try_find("profile-reading-more").is_some()
    })
    .await;

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
    assert!(backend.more_stopped(), "the page on its way is stopped");

    backend.release();
    frames(&app, cx, 20);
    assert_eq!(
        pages_asked(&backend),
        [2],
        "nothing asked from a cursor let go of"
    );
    for id in [
        "profile-reading-more",
        "profile-more-failed",
        "profile-all-read",
    ] {
        assert!(!found(&app, cx, id), "{id} is not shown");
    }
    app.in_window(cx, |window, _| {
        assert!(
            window.try_find(("profile-tile", 12usize)).is_none(),
            "the page let go of is not shown"
        );
    });
}

/// The followers of the account in use, opened from its profile, once
/// their first row is drawn.
async fn own_followers(cx: &mut TestAppContext, backend: Scripted) -> App {
    let app = own_profile(cx, backend).await;
    app.click(cx, "profile-followers");
    app.until(cx, |window, _| {
        window.try_find(("person", 0usize)).is_some()
    })
    .await;
    app
}

/// Turns the wheel far down over a list's rows.
fn scroll_the_list_down(app: &App, cx: &mut TestAppContext) {
    app.in_window(cx, |window, cx| {
        let seen = window.viewport_size();
        window.dispatch_event(
            PlatformInput::Scroll(ScrollEvent {
                position: point(seen.width * 0.5, seen.height * 0.7),
                delta: ScrollDelta::Pixels(point(px(0.), px(-100_000.))),
                ..ScrollEvent::default()
            }),
            cx,
        );
    });
}

/// What the backend was asked, but the rows' pictures.
fn asked_but_pictures(backend: &Scripted) -> Vec<Asked> {
    backend
        .asked()
        .into_iter()
        .filter(|asked| !matches!(asked, Asked::Pictures(_)))
        .collect()
}

/// How many times `pk`'s `side` list was read.
fn lists_read(backend: &Scripted, pk: u64, side: Side) -> usize {
    backend
        .asked()
        .into_iter()
        .filter(|asked| *asked == Asked::List(Pk::new(pk), side))
        .count()
}

/// Waits until the line under a list's title says something `wanted`
/// takes.
async fn until_said(app: &App, cx: &mut TestAppContext, wanted: impl Fn(&str) -> bool) {
    app.until(cx, |window, _| {
        window
            .try_find("people-progress")
            .is_some_and(|progress| progress.label().is_some_and(&wanted))
    })
    .await;
}

#[gpui_kit::test]
async fn a_list_stored_before_shows_at_once_and_is_read_again_behind_it(cx: &mut TestAppContext) {
    let backend = me();
    backend.script(|script| {
        script.kept_list(42, Side::Followers, 30).list_size(40);
    });
    let app = own_profile(cx, backend.clone()).await;
    told_it_is(TEN_DAYS_AFTER_THE_SCAN, cx);
    backend.hold_the_work();
    app.click(cx, "profile-followers");
    until_said(&app, cx, |said| said.ends_with("Updating… 12 of 40")).await;
    assert_eq!(
        app.label_of(cx, "people-progress"),
        "30 accounts. Last scan on September 21 at 14:13. Updating… 12 of 40"
    );
    assert_eq!(lists_read(&backend, 42, Side::Followers), 1);
    // The rows stored stay while the read goes on, past its first page.
    scroll_the_list_down(&app, cx);
    frames(&app, cx, 2);
    app.in_window(cx, |window, _| {
        assert!(window.try_find(("person", 29usize)).is_some());
        assert!(window.try_find(("person", 30usize)).is_none());
    });

    backend.release();
    until_said(&app, cx, |said| said.starts_with("40 accounts.")).await;
    assert_eq!(
        app.label_of(cx, "people-progress"),
        "40 accounts. Last scan on September 21 at 14:13."
    );
}

#[gpui_kit::test]
async fn a_list_left_and_opened_again_shows_its_rows_at_once_and_is_read_again(
    cx: &mut TestAppContext,
) {
    let backend = me();
    let app = own_followers(cx, backend.clone()).await;
    told_it_is(TEN_DAYS_AFTER_THE_SCAN, cx);
    until_said(&app, cx, |said| said.starts_with("30 accounts.")).await;

    app.click(cx, "band-back");
    app.until_screen(cx, |screen| matches!(screen, Screen::Profile(_)))
        .await;
    backend.hold_the_work();
    app.click(cx, "profile-followers");
    app.until_screen(cx, |screen| matches!(screen, Screen::People { .. }))
        .await;
    // Before the read answers: the rows and their words, from memory, and
    // how far the new read is once its first page is in.
    let said = app.label_of(cx, "people-progress");
    assert!(
        said.starts_with("30 accounts. Last scan on September 21 at 14:13. Updating…"),
        "{said}"
    );
    app.in_window(cx, |window, _| {
        assert!(window.try_find(("person", 0usize)).is_some());
    });
    assert_eq!(lists_read(&backend, 42, Side::Followers), 2);
    backend.release();
}

#[gpui_kit::test]
async fn another_account_s_stored_list_is_read_again_only_after_its_yes(cx: &mut TestAppContext) {
    let backend = me();
    backend.script(|script| {
        script.kept_list(1000, Side::Followers, 5);
    });
    let app = own_profile(cx, backend.clone()).await;
    app.click(cx, "profile-following");
    app.until(cx, |window, _| {
        window.try_find(("person", 0usize)).is_some()
    })
    .await;
    app.in_window(cx, |window, cx| window.click(("person", 0usize), cx));
    app.until(cx, |window, _| {
        window.try_find(("profile-tile", 0usize)).is_some()
    })
    .await;

    // Its stored rows are shown at once after the yes, while it is read.
    backend.hold_the_work();
    app.click(cx, "profile-followers");
    assert_eq!(lists_read(&backend, 1000, Side::Followers), 0);
    app.click(cx, "list-question-confirm");
    until_said(&app, cx, |said| said.starts_with("5 accounts.")).await;
    assert_eq!(lists_read(&backend, 1000, Side::Followers), 1);
    backend.release();
    until_said(&app, cx, |said| said.starts_with("30 accounts.")).await;

    // Left and asked for again: nothing is read before the yes.
    app.click(cx, "band-back");
    app.until_screen(
        cx,
        |screen| matches!(screen, Screen::Profile(who) if who.pk == Pk::new(1000)),
    )
    .await;
    app.click(cx, "profile-followers");
    assert!(shown(&app, cx, "list-question"));
    app.press(cx, "escape");
    assert_eq!(
        lists_read(&backend, 1000, Side::Followers),
        1,
        "a no reads nothing"
    );
    app.click(cx, "profile-followers");
    app.click(cx, "list-question-confirm");
    until_said(&app, cx, |said| said.starts_with("30 accounts.")).await;
    assert_eq!(lists_read(&backend, 1000, Side::Followers), 2);
}

#[gpui_kit::test]
async fn the_line_under_the_title_says_how_many_and_when_they_were_scanned_in_both_languages(
    cx: &mut TestAppContext,
) {
    for (locale, one, scanned) in [
        (
            "en",
            "1 account.",
            "1 account. Last scan on September 21 at 14:13.",
        ),
        (
            "es",
            "1 cuenta.",                                                 // i18n-allow
            "1 cuenta. Último escaneo el 21 de septiembre a las 14:13.", // i18n-allow
        ),
    ] {
        let backend = me();
        backend.script(|script| {
            script.list_size(1);
        });
        let app = common::open_with(
            cx,
            backend,
            Settings {
                locale: locale.to_string(),
                ..Default::default()
            },
        );
        told_it_is(TEN_DAYS_AFTER_THE_SCAN, cx);
        app.until_screen(cx, |screen| *screen == Screen::Home).await;
        app.click(cx, "account");
        app.click(cx, "menu-profile");
        app.until(cx, |window, _| {
            window.try_find(("profile-tile", 0usize)).is_some()
        })
        .await;
        app.click(cx, "profile-followers");
        until_said(&app, cx, |said| said.starts_with(one)).await;
        assert_eq!(app.label_of(cx, "people-progress"), scanned);
    }
}

#[gpui_kit::test]
async fn the_options_hold_choosing_and_a_window_of_its_own_and_close_on_escape(
    cx: &mut TestAppContext,
) {
    let app = own_followers(cx, me()).await;
    app.click(cx, "people-options");
    app.in_window(cx, |window, _| {
        let menu = window.find("people-menu");
        assert_eq!(menu.role(), Some(Role::Menu));
        assert_eq!(menu.label(), Some("List options"));
        assert_eq!(window.find("people-options").expanded(), Some(true));
        let select = window.find("people-select");
        assert_eq!(select.role(), Some(Role::MenuItem));
        assert_eq!(select.label(), Some("Select"));
        assert_eq!(
            window.find("people-detach").label(),
            Some("Open in another window")
        );
    });
    app.press(cx, "escape");
    assert!(!shown(&app, cx, "people-menu"));
    assert!(focused(&app, cx, "people-options"), "focus is back");
    assert!(matches!(
        app.shell.read_with(cx, |shell, _| shell.shown().clone()),
        Screen::People { .. }
    ));

    // The second moves the list out, and the panel goes back.
    app.click(cx, "people-options");
    app.click(cx, "people-detach");
    app.until_screen(cx, |screen| matches!(screen, Screen::Profile(_)))
        .await;
}

#[gpui_kit::test]
async fn rows_chosen_and_all_of_them_have_their_usernames_copied_sending_nothing(
    cx: &mut TestAppContext,
) {
    let backend = me();
    backend.script(|script| {
        script.list_size(5);
    });
    let app = own_followers(cx, backend.clone()).await;
    until_said(&app, cx, |said| said.starts_with("5 accounts.")).await;
    let before = asked_but_pictures(&backend);

    app.click(cx, "people-options");
    app.click(cx, "people-select");
    assert_eq!(app.label_of(cx, "people-selected"), "0 selected");
    assert!(disabled(&app, cx, "people-copy"));
    app.in_window(cx, |window, cx| {
        let row = window.find(("person", 2usize));
        assert_eq!(row.role(), Some(Role::CheckBox));
        assert_eq!(row.label(), Some("Select @user2"));
        assert_eq!(row.checked(), Some(false));
        window.click(("person", 2usize), cx);
    });
    app.in_window(cx, |window, _| {
        assert_eq!(window.find(("person", 2usize)).checked(), Some(true));
    });
    assert_eq!(app.label_of(cx, "people-selected"), "1 selected");
    assert!(
        matches!(
            app.shell.read_with(cx, |shell, _| shell.shown().clone()),
            Screen::People { .. }
        ),
        "a row chosen opens nothing"
    );

    app.click(cx, "people-copy");
    assert_eq!(app.system.clipboard().as_deref(), Some("user2"));
    assert_eq!(app.label_of(cx, "people-selected"), "1 username copied");

    app.click(cx, "people-select-all");
    assert_eq!(app.label_of(cx, "people-selected"), "5 selected");
    assert!(disabled(&app, cx, "people-select-all"));
    app.click(cx, "people-copy");
    assert_eq!(
        app.system.clipboard().as_deref(),
        Some("user0\nuser1\nuser2\nuser3\nuser4")
    );
    assert!(
        app.label_of(cx, "announcements")
            .contains("5 usernames copied")
    );
    assert_eq!(asked_but_pictures(&backend), before, "nothing was asked");

    app.press(cx, "escape");
    assert!(!shown(&app, cx, "people-selection"));
    app.in_window(cx, |window, _| {
        assert_eq!(window.find(("person", 2usize)).role(), Some(Role::Button));
    });
}

#[gpui_kit::test]
async fn pictures_are_asked_for_the_rows_on_screen_and_a_few_around_once_each(
    cx: &mut TestAppContext,
) {
    let backend = me();
    backend.script(|script| {
        script.list_size(200);
    });
    let app = own_followers(cx, backend.clone()).await;
    until_said(&app, cx, |said| said.starts_with("200 accounts.")).await;
    cx.executor().advance_clock(Duration::from_millis(200));
    frames(&app, cx, 2);

    let asked = |backend: &Scripted| -> Vec<Vec<u64>> {
        backend
            .asked()
            .into_iter()
            .filter_map(|asked| match asked {
                Asked::Pictures(rows) => Some(rows.into_iter().map(|pk| pk.get() - 1000).collect()),
                _ => None,
            })
            .collect()
    };
    let drawn: Vec<u64> = app.in_window(cx, |window, _| {
        (0..200u64)
            .filter(|at| window.try_find(("person", *at as usize)).is_some())
            .collect()
    });
    let last = *drawn.last().expect("rows drawn");
    let batches = asked(&backend);
    let mut wanted: Vec<u64> = batches.last().expect("pictures asked for").clone();
    wanted.sort_unstable();
    assert_eq!(
        wanted,
        (0..=last + 4).collect::<Vec<_>>(),
        "the rows drawn, {drawn:?}, and four below"
    );

    // Far down: the rows there, and none asked for twice.
    scroll_the_list_down(&app, cx);
    frames(&app, cx, 2);
    cx.executor().advance_clock(Duration::from_millis(200));
    frames(&app, cx, 2);
    let batches = asked(&backend);
    let all: Vec<u64> = batches.iter().flatten().copied().collect();
    let mut once = all.clone();
    once.sort_unstable();
    once.dedup();
    assert_eq!(once.len(), all.len(), "a row's picture is asked for once");
    assert!(all.contains(&199), "the last rows, once drawn");
    assert!(!all.contains(&100), "rows never drawn are not asked for");
}

/// The rows of each batch of pictures asked for, by their place in the list.
fn pictures_asked(backend: &Scripted) -> Vec<Vec<u64>> {
    backend
        .asked()
        .into_iter()
        .filter_map(|asked| match asked {
            Asked::Pictures(rows) => Some(rows.into_iter().map(|pk| pk.get() - 1000).collect()),
            _ => None,
        })
        .collect()
}

/// Turns the wheel `by` pixels over the rows of a list in `window`: down
/// when negative.
fn wheel(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App, by: f32) {
    let seen = window.viewport_size();
    window.dispatch_event(
        PlatformInput::Scroll(ScrollEvent {
            position: point(seen.width * 0.5, seen.height * 0.7),
            delta: ScrollDelta::Pixels(point(px(0.), px(by))),
            ..ScrollEvent::default()
        }),
        cx,
    );
}

/// Lets the rows drawn stay still past the wait for their pictures.
fn settle(app: &App, cx: &mut TestAppContext) {
    frames(app, cx, 2);
    cx.executor().advance_clock(Duration::from_millis(200));
    frames(app, cx, 2);
}

/// The first row drawn, and what a screen reader is told of it.
fn top_row(app: &App, cx: &mut TestAppContext) -> (usize, String) {
    app.in_window(cx, |window, _| {
        let at = (0..400usize)
            .find(|at| window.try_find(("person", *at)).is_some())
            .expect("a row drawn");
        let row = window.find(("person", at));
        (at, row.label().unwrap_or_default().to_string())
    })
}

/// The list shown moved out to a window of its own, and that window.
async fn moved_out(app: &App, cx: &mut TestAppContext) -> gpui_kit::AnyWindowHandle {
    app.click(cx, "people-options");
    app.click(cx, "people-detach");
    app.until_screen(cx, |screen| matches!(screen, Screen::Profile(_)))
        .await;
    let main = app.window;
    cx.update(|cx| cx.windows())
        .into_iter()
        .find(|window| *window != main)
        .expect("the list's own window")
}

/// Draws `window` and acts on it.
fn in_its_window<R>(
    cx: &mut TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    act: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        act(window, cx)
    })
    .expect("the list's window is open")
}

#[gpui_kit::test]
async fn a_batch_of_pictures_a_scroll_stopped_is_asked_again_and_one_batch_goes_at_a_time(
    cx: &mut TestAppContext,
) {
    let backend = me();
    backend.script(|script| {
        script.list_size(200);
    });
    let app = own_followers(cx, backend.clone()).await;
    until_said(&app, cx, |said| said.starts_with("200 accounts.")).await;
    backend.hold_the_work();
    settle(&app, cx);
    let asked = pictures_asked(&backend);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].contains(&0));

    // Down while it is on its way: it is stopped, and the next waits for it.
    scroll_the_list_down(&app, cx);
    settle(&app, cx);
    assert_eq!(pictures_asked(&backend).len(), 1, "one batch at a time");
    backend.release();
    frames(&app, cx, 2);
    let asked = pictures_asked(&backend);
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert!(asked[1].contains(&199) && !asked[1].contains(&0));

    // Back up: what the batch stopped never brought is asked for again.
    app.in_window(cx, |window, cx| wheel(window, cx, 100_000.));
    settle(&app, cx);
    let asked = pictures_asked(&backend);
    assert_eq!(asked.len(), 3, "{asked:?}");
    assert!(asked[2].contains(&0));
}

#[gpui_kit::test]
async fn pictures_are_asked_for_only_once_the_rows_have_stayed_still(cx: &mut TestAppContext) {
    let backend = me();
    backend.script(|script| {
        script.list_size(200);
    });
    let app = own_followers(cx, backend.clone()).await;
    until_said(&app, cx, |said| said.starts_with("200 accounts.")).await;
    settle(&app, cx);
    let before = pictures_asked(&backend).len();

    scroll_the_list_down(&app, cx);
    frames(&app, cx, 2);
    cx.executor().advance_clock(Duration::from_millis(100));
    frames(&app, cx, 2);
    assert_eq!(pictures_asked(&backend).len(), before, "not while it moves");
    cx.executor().advance_clock(Duration::from_millis(100));
    frames(&app, cx, 2);
    assert_eq!(pictures_asked(&backend).len(), before + 1);
}

#[gpui_kit::test]
async fn no_picture_is_asked_for_while_the_app_rests_and_they_are_once_it_wakes(
    cx: &mut TestAppContext,
) {
    let backend = me();
    backend.script(|script| {
        script.list_size(200);
    });
    let app = own_followers(cx, backend.clone()).await;
    until_said(&app, cx, |said| said.starts_with("200 accounts.")).await;
    backend.hold_the_work();
    settle(&app, cx);
    let asked = pictures_asked(&backend);
    assert_eq!(asked.len(), 1, "{asked:?}");
    let below = asked[0].iter().max().copied().unwrap_or_default();
    // A taller window draws more rows while the first batch is on its way:
    // the next waits for it.
    cx.simulate_window_resize(app.window, size(px(900.), px(2400.)));
    settle(&app, cx);
    assert_eq!(pictures_asked(&backend).len(), 1);

    VisualTestContext::from_window(app.window, cx).deactivate_window();
    cx.executor().advance_clock(QUIET_FOR * 2);
    cx.run_until_parked();
    assert!(app.system.is_resting());
    // The batch on its way ends while it rests: the next asks nothing.
    backend.release();
    cx.run_until_parked();
    assert_eq!(pictures_asked(&backend).len(), 1, "nothing while it rests");

    cx.update_window(app.window, |_, window, _| window.activate_window())
        .expect("the window is open");
    cx.run_until_parked();
    assert!(!app.system.is_resting());
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    let asked = pictures_asked(&backend);
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert!(asked[1].iter().any(|row| *row > below), "{asked:?}");
}

#[gpui_kit::test]
async fn a_list_in_its_own_window_keeps_the_app_awake_and_its_pictures_coming(
    cx: &mut TestAppContext,
) {
    let backend = me();
    backend.script(|script| {
        script.list_size(200);
    });
    let app = own_followers(cx, backend.clone()).await;
    until_said(&app, cx, |said| said.starts_with("200 accounts.")).await;
    settle(&app, cx);
    let before = pictures_asked(&backend).len();
    let alone = moved_out(&app, cx).await;
    cx.update_window(alone, |_, window, _| window.activate_window())
        .expect("the list's window is open");
    cx.run_until_parked();

    cx.executor().advance_clock(QUIET_FOR * 2);
    cx.run_until_parked();
    assert!(
        !app.system.is_resting(),
        "the list's own window is looked at"
    );
    in_its_window(cx, alone, |_, _| ());
    in_its_window(cx, alone, |window, cx| wheel(window, cx, -100_000.));
    in_its_window(cx, alone, |_, _| ());
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    let asked = pictures_asked(&backend);
    assert_eq!(asked.len(), before + 1, "{asked:?}");
    assert!(asked[before].contains(&199));
}

#[gpui_kit::test]
async fn a_list_in_its_own_window_chooses_copies_and_stops_choosing_on_escape(
    cx: &mut TestAppContext,
) {
    let backend = me();
    backend.script(|script| {
        script.list_size(5);
    });
    let app = own_followers(cx, backend.clone()).await;
    until_said(&app, cx, |said| said.starts_with("5 accounts.")).await;
    let alone = moved_out(&app, cx).await;
    let before = asked_but_pictures(&backend);

    in_its_window(cx, alone, |window, cx| window.click("people-options", cx));
    in_its_window(cx, alone, |window, cx| window.click("people-select", cx));
    in_its_window(cx, alone, |window, cx| window.click(("person", 3usize), cx));
    in_its_window(cx, alone, |window, _| {
        assert_eq!(window.find(("person", 3usize)).checked(), Some(true));
        assert_eq!(window.find("people-selected").label(), Some("1 selected"));
    });
    in_its_window(cx, alone, |window, cx| window.click("people-copy", cx));
    assert_eq!(app.system.clipboard().as_deref(), Some("user3"));
    assert_eq!(asked_but_pictures(&backend), before, "nothing was asked");

    in_its_window(cx, alone, |window, cx| window.press("escape", cx));
    in_its_window(cx, alone, |window, _| {
        assert!(window.try_find("people-selection").is_none());
        assert_eq!(window.find(("person", 3usize)).role(), Some(Role::Button));
    });
}

#[gpui_kit::test]
async fn a_row_scrolled_away_with_focus_leaves_it_on_the_title_and_tab_goes_on(
    cx: &mut TestAppContext,
) {
    let backend = me();
    backend.script(|script| {
        script.list_size(40);
    });
    let app = own_followers(cx, backend).await;
    until_said(&app, cx, |said| said.starts_with("40 accounts.")).await;
    app.click(cx, "people-options");
    app.click(cx, "people-select");
    app.in_window(cx, |window, cx| window.click(("person", 1usize), cx));
    app.in_window(cx, |window, _| {
        let row = window.find(("person", 1usize));
        assert_eq!(row.focused(), Some(true), "the row chosen has focus");
    });

    scroll_the_list_down(&app, cx);
    cx.run_until_parked();
    app.in_window(cx, |window, _| {
        assert!(
            window.try_find(("person", 1usize)).is_none(),
            "scrolled away"
        );
    });
    assert!(focused(&app, cx, "title"), "focus is on the title");
    app.press(cx, "tab");
    assert!(!focused(&app, cx, "title"), "Tab goes on from it");
}

#[gpui_kit::test]
async fn a_list_shown_again_before_its_stored_rows_arrive_is_read_once(cx: &mut TestAppContext) {
    let backend = me();
    backend.script(|script| {
        script.kept_list(42, Side::Followers, 30).list_size(40);
    });
    let app = own_profile(cx, backend.clone()).await;
    backend.hold_the_kept_lists();
    app.click(cx, "profile-followers");
    app.until_screen(cx, |screen| matches!(screen, Screen::People { .. }))
        .await;
    cx.run_until_parked();
    assert_eq!(lists_read(&backend, 42, Side::Followers), 0);

    app.click(cx, "band-back");
    app.until_screen(cx, |screen| matches!(screen, Screen::Profile(_)))
        .await;
    app.click(cx, "profile-followers");
    app.until_screen(cx, |screen| matches!(screen, Screen::People { .. }))
        .await;
    cx.run_until_parked();
    assert_eq!(lists_read(&backend, 42, Side::Followers), 1);
    backend.release();
    frames(&app, cx, 2);
    until_said(&app, cx, |said| said.starts_with("40 accounts.")).await;
    assert_eq!(lists_read(&backend, 42, Side::Followers), 1, "read once");
}

#[gpui_kit::test]
async fn rows_chosen_stay_chosen_when_the_read_behind_them_ends(cx: &mut TestAppContext) {
    let backend = me();
    backend.script(|script| {
        script.kept_list(42, Side::Followers, 30).list_size(40);
    });
    let app = own_profile(cx, backend.clone()).await;
    backend.hold_the_work();
    app.click(cx, "profile-followers");
    until_said(&app, cx, |said| said.starts_with("30 accounts.")).await;
    app.click(cx, "people-options");
    app.click(cx, "people-select");
    app.in_window(cx, |window, cx| window.click(("person", 2usize), cx));
    assert_eq!(app.label_of(cx, "people-selected"), "1 selected");

    backend.release();
    until_said(&app, cx, |said| said.starts_with("40 accounts.")).await;
    app.in_window(cx, |window, _| {
        assert_eq!(window.find(("person", 2usize)).checked(), Some(true));
    });
    assert_eq!(app.label_of(cx, "people-selected"), "1 selected");
}

#[gpui_kit::test]
async fn the_row_at_the_top_stays_there_when_the_read_brings_rows_above_it(
    cx: &mut TestAppContext,
) {
    let backend = me();
    backend.script(|script| {
        script
            .kept_list_from(42, Side::Followers, 10, 30)
            .list_size(40);
    });
    let app = own_profile(cx, backend.clone()).await;
    backend.hold_the_work();
    app.click(cx, "profile-followers");
    until_said(&app, cx, |said| said.starts_with("30 accounts.")).await;
    app.in_window(cx, |window, cx| wheel(window, cx, -400.));
    frames(&app, cx, 2);
    let (at, said) = top_row(&app, cx);
    assert!(at > 0, "scrolled");

    backend.release();
    until_said(&app, cx, |said| said.starts_with("40 accounts.")).await;
    frames(&app, cx, 2);
    assert_eq!(top_row(&app, cx), (at + 10, said));
}
