//! A profile page and the lists it opens, driven in process on the scripted
//! backend: what a person or a screen reader finds, and **what was asked of
//! the backend, and when**: another account's list only after a yes, and
//! nothing of a story ever.

#![cfg(feature = "testing")]

mod common;

use common::{App, focused, open};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Role, TestAppContext, px};
use snob_core::Pk;
use snob_desktop::backend::scripted::{Asked, Scripted, account};
use snob_desktop::state::flow::Screen;
use snob_desktop::state::profile::{Side, Tab};

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
        assert_eq!(
            window.find("people-detach").label(),
            Some("Open in a new window")
        );
    });
    assert!(
        backend
            .asked()
            .contains(&Asked::List(Pk::new(42), Side::Followers))
    );

    backend.release();
    app.until(cx, |window, _| {
        window
            .try_find("people-progress")
            .is_some_and(|progress| progress.label() == Some("40 accounts"))
    })
    .await;

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

#[gpui_kit::test]
async fn the_options_copy_what_is_known_of_the_account(cx: &mut TestAppContext) {
    let app = own_profile(cx, me()).await;
    app.click(cx, "profile-options");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("profile-menu").label(), Some("Options"));
        assert_eq!(window.find("profile-options").expanded(), Some(true));
    });
    app.click(cx, "profile-copy-link");
    assert_eq!(
        app.system.clipboard().as_deref(),
        Some("https://www.instagram.com/me/")
    );
    assert!(!shown(&app, cx, "profile-menu"));
    assert!(focused(&app, cx, "profile-options"), "focus is back");
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
            _ => assert!(bar.bottom() <= picture.top(), "{bar:?} over {picture:?}"),
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
