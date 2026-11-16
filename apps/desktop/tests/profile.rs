//! A profile page and the lists it opens, driven in process on the scripted
//! backend: what a person or a screen reader finds, and **what was asked of
//! the backend, and when**: another account's list only after a yes, and
//! nothing of a story ever.

#![cfg(feature = "testing")]

mod common;

use common::{App, focused, open};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Role, TestAppContext};
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
        assert_eq!(window.find("profile-back").label(), Some("Back"));
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

    app.click(cx, "profile-back");
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
    app.click(cx, "people-back");
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
