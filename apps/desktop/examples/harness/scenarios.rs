//! The scenarios: each is a person doing one thing with the real app,
//! against the fake Instagram, and what they must find. **A scenario reads
//! the window as a screen reader does and acts on controls by name**, so
//! what passes here is what a person without sight, or an agent, can do.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use anyhow::{anyhow, bail};
use snob_desktop::control::{Answer, Pointing, Said};
use snob_fake_ig::world::{self, World};

use crate::drive::{Driven, Start, expect, expect_eq, label, node};

/// What every scenario is run with.
pub struct Lab {
    /// The test build of the app.
    pub exe: PathBuf,
    pub world: World,
    /// Where the fake Instagram is served.
    pub instagram: String,
}

/// One scenario's own corner: a data directory nobody else uses, and the
/// app running in it.
pub struct Stage<'a> {
    pub lab: &'a Lab,
    pub root: PathBuf,
    pub app: Option<Driven>,
    /// How the app is started, unless a scenario says otherwise.
    pub start: Start,
    /// Where a picture is kept after every step.
    pub film: Option<PathBuf>,
}

impl Stage<'_> {
    /// Starts the app.
    pub async fn open(&mut self) -> anyhow::Result<&mut Driven> {
        let start = self.start.clone();
        self.open_with(start).await
    }

    pub async fn open_with(&mut self, start: Start) -> anyhow::Result<&mut Driven> {
        self.close().await?;
        let mut driven = Driven::start(&self.lab.exe, &self.root, &start).await?;
        if let Some(folder) = &self.film {
            std::fs::create_dir_all(folder)?;
            let frames = std::fs::read_dir(folder).map(Iterator::count).unwrap_or(0);
            driven.film = Some((folder.clone(), frames));
        }
        Ok(self.app.insert(driven))
    }

    pub fn app(&mut self) -> anyhow::Result<&mut Driven> {
        self.app
            .as_mut()
            .ok_or_else(|| anyhow!("the app is not running"))
    }

    /// Closes the app, when it runs.
    pub async fn close(&mut self) -> anyhow::Result<()> {
        match self.app.take() {
            Some(app) => app.quit().await,
            None => Ok(()),
        }
    }

    /// Signs `pk` in with a pasted session ID, from the start screen to the
    /// home screen.
    pub async fn sign_in(&mut self, pk: u64, tag: &str) -> anyhow::Result<Answer> {
        let app = self.app()?;
        app.until_screen("start").await?;
        app.click("start-paste").await?;
        app.until_screen("paste").await?;
        app.copy(&session_id(pk, tag)).await?;
        app.click("paste-paste").await?;
        app.click("paste-submit").await?;
        app.until_screen("home").await
    }
}

/// A `sessionid` of `pk`, told apart from every other scenario's by `tag`.
pub fn session_id(pk: u64, tag: &str) -> String {
    format!("{pk}%3A{tag}AbCdEfGh%3A20")
}

type Run =
    for<'a, 'b> fn(&'a mut Stage<'b>) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>>;

/// A scenario: its name, what it is about, and what runs it.
pub struct Scenario {
    pub name: &'static str,
    /// Which sections of the design it covers.
    pub covers: &'static str,
    pub run: Run,
}

macro_rules! scenario {
    ($name:literal, $covers:literal, $run:ident) => {
        Scenario {
            name: $name,
            covers: $covers,
            run: |stage| Box::pin($run(stage)),
        }
    };
}

/// Every scenario, in the order they run.
pub fn all() -> Vec<Scenario> {
    vec![
        scenario!("first start with nobody signed in", "4", first_start),
        scenario!("sign in with a pasted session ID", "4", paste_signs_in),
        scenario!(
            "a session that is already here goes home",
            "4",
            session_already_here
        ),
        scenario!(
            "an expired session asks its account back",
            "4",
            expired_session
        ),
        scenario!(
            "what is typed wrong is said under the field",
            "4",
            typed_wrong
        ),
        scenario!("an ID Instagram refuses is an error", "4", refused_id),
        scenario!(
            "the account menu and the settings",
            "3 9",
            menu_and_settings
        ),
        scenario!(
            "the frame opens on the sections and closes to add an account",
            "2 3",
            frame_and_sections
        ),
        scenario!(
            "a theme and a language chosen are kept",
            "1 9",
            theme_and_language
        ),
        scenario!("the settings are searched", "9", settings_search),
        scenario!(
            "clearing the cache keeps the session",
            "9 10",
            clearing_the_cache
        ),
        scenario!(
            "deleting history asks first and keeps the session",
            "9 10",
            forgetting_history
        ),
        scenario!(
            "the outline shows while the scrollbar is dragged",
            "9",
            outline_while_dragging
        ),
        scenario!("switching between two accounts", "3", switch_accounts),
        scenario!("signing out asks first", "3", signing_out),
        scenario!("the network goes and comes back", "3 5", offline),
        scenario!("the system sleeps and wakes", "5", sleep_and_wake),
        scenario!(
            "a second start hands over to the first",
            "5",
            second_instance
        ),
        scenario!(
            "the theme and the text size follow the system",
            "1 8",
            follows_the_system
        ),
        scenario!(
            "no secret reaches the log or the report",
            "9 10",
            no_secret_is_written
        ),
        scenario!("the keyboard alone signs in", "8", keyboard_alone),
        scenario!(
            "the automations are introduced once an account",
            "2 3",
            automations_introduced_once
        ),
        scenario!(
            "a profile, its lists and the consent another's asks",
            "3 8",
            profile_and_lists
        ),
    ]
}

/// Whether a profile's first read has come: its first tab's posts, or why
/// there are none.
pub fn profile_read(seen: &Answer) -> bool {
    seen.screen == "profile"
        && seen.nodes.iter().any(|node| {
            [
                "profile-tile#0",
                "profile-empty",
                "profile-hidden",
                "profile-tab-failed",
            ]
            .contains(&node.id.as_str())
        })
}

/// Whether the list shown was read to its end.
pub fn list_read(seen: &Answer) -> bool {
    seen.screen == "people"
        && seen.nodes.iter().any(|node| {
            node.id == "people-progress"
                && node
                    .label
                    .as_deref()
                    .is_some_and(|said| said.ends_with("accounts"))
        })
}

/// The account's own profile from its menu, its story's way in reading
/// nothing, its own list read without a question, and another account's
/// list asked about first: **a no sends nothing**, a yes reads it.
async fn profile_and_lists(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    let world = stage.lab.world.clone();
    stage.open().await?;
    stage.sign_in(world::SOMEONE, "profile").await?;
    let app = stage.app()?;
    app.click("account").await?;
    app.click("menu-profile").await?;
    let seen = app.until("the own profile read", profile_read).await?;
    let title = label(&seen, "title")?;
    expect(title.contains("@someone"), || {
        format!("the page's title is {title:?}")
    })?;
    // The fake Instagram's REST profile, which a build with no browser
    // reads, says nothing of stories: the ring is shown only where the
    // profile query is (`tests/profile.rs` has it on the script). Either
    // way the picture opens, and no story is read.
    let story = label(&seen, "profile-picture")?.ends_with("with a story");
    let seen = app.click("profile-picture").await?;
    if story {
        expect(
            seen.nodes
                .iter()
                .any(|node| node.id == "profile-open-story"),
            || "a story up offers the story".to_string(),
        )?;
        app.click("profile-open-story").await?;
        let seen = app.look().await?;
        expect(seen.announced.contains("Nothing was read"), || {
            format!("the story's way in said {:?}", seen.announced)
        })?;
    } else {
        let viewer = node(&seen, "photo-viewer")?;
        expect_eq(viewer.role.as_deref(), Some("Dialog"), "the picture large")?;
        app.press("escape").await?;
    }

    app.click("profile-followers").await?;
    let seen = app.until("the own followers read", list_read).await?;
    expect(
        !seen.nodes.iter().any(|node| node.id == "list-question"),
        || "the own list asked a question".to_string(),
    )?;
    expect(
        label(&seen, "person#0")?.starts_with("Open the profile of @"),
        || "a row opens its profile".to_string(),
    )?;

    app.click("person#0").await?;
    app.until("their profile read", profile_read).await?;
    let before = world.search_surfaces().len();
    let seen = app.click("profile-followers").await?;
    let asked = node(&seen, "list-question")?;
    expect_eq(asked.role.as_deref(), Some("AlertDialog"), "the question")?;
    app.press("escape").await?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let seen = app.look().await?;
    expect_eq(seen.screen.as_str(), "profile", "a no stays on the profile")?;
    expect_eq(
        world.search_surfaces().len(),
        before,
        "followers pages asked after a no",
    )?;

    app.click("profile-followers").await?;
    app.click("list-question-confirm").await?;
    app.until("their followers read", list_read).await?;
    expect(world.search_surfaces().len() > before, || {
        "a yes read nothing".to_string()
    })
}

async fn first_start(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    let app = stage.open().await?;
    let seen = app.until_screen("start").await?;
    expect_eq(
        label(&seen, "title")?.as_str(),
        "Let's get started.",
        "the title",
    )?;
    expect_eq(
        node(&seen, "title")?.focused,
        Some(true),
        "focus on the title",
    )?;
    expect_eq(
        label(&seen, "start-browser")?.as_str(),
        "Sign in to Instagram",
        "the main button",
    )?;
    expect_eq(
        label(&seen, "start-paste")?.as_str(),
        "Use your session ID (advanced)",
        "the second button",
    )?;
    expect_eq(
        label(&seen, "test-badge")?.as_str(),
        "TEST",
        "the test badge",
    )?;
    expect(node(&seen, "account").is_err(), || {
        "an account is shown with nobody signed in".to_string()
    })
}

async fn paste_signs_in(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    let app = stage.app()?;
    app.until_screen("start").await?;
    app.click("start-paste").await?;
    let seen = app.until_screen("paste").await?;
    expect_eq(
        node(&seen, "paste-submit")?.disabled,
        Some(true),
        "the button, empty",
    )?;

    app.copy(&session_id(world::ME, "paste")).await?;
    let seen = app.click("paste-paste").await?;
    expect(node(&seen, "paste-submit")?.disabled != Some(true), || {
        "the button stayed off with a session ID pasted".to_string()
    })?;
    app.click("paste-submit").await?;

    let seen = app.until_screen("welcome").await?;
    expect_eq(
        label(&seen, "title")?.as_str(),
        "Welcome, @me.",
        "the welcome",
    )?;
    expect_eq(
        seen.announced.as_str(),
        "Welcome, @me.",
        "said to a screen reader",
    )?;

    let seen = app.until_screen("home").await?;
    expect_eq(
        label(&seen, "account-photo")?.as_str(),
        "Profile picture of @me",
        "the account in the band",
    )?;
    let picture = app.paths.account(world::ME.into()).viewer_picture();
    expect(picture.is_file(), || {
        format!("the picture was not kept at {}", picture.display())
    })
}

async fn session_already_here(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "kept").await?;
    // The next start: no start screen, no question.
    let app = stage.open().await?;
    let seen = app.until_screen("home").await?;
    expect_eq(label(&seen, "title")?.as_str(), "Home", "the title")?;
    expect_eq(
        label(&seen, "account-photo")?.as_str(),
        "Profile picture of @me",
        "the account in the band",
    )
}

async fn expired_session(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "expires").await?;
    stage.close().await?;
    // What the app writes down when it finds a session dead.
    let prefs = stage.root.join("data").join("desktop.json");
    let mut kept: serde_json::Value = match std::fs::read_to_string(&prefs) {
        Ok(text) => serde_json::from_str(&text)?,
        Err(_) => serde_json::json!({}),
    };
    kept["expired"] = serde_json::json!(world::ME);
    std::fs::write(&prefs, serde_json::to_vec(&kept)?)?;

    let app = stage.open().await?;
    let seen = app.until_screen("start").await?;
    expect_eq(
        label(&seen, "expired-photo")?.as_str(),
        "Profile picture of @me",
        "whose session expired",
    )?;

    // Signing in again as it is what clears it.
    stage.sign_in(world::ME, "renewed").await?;
    let app = stage.open().await?;
    app.until_screen("home").await.map(drop)
}

async fn typed_wrong(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    let app = stage.open().await?;
    app.until_screen("start").await?;
    app.click("start-paste").await?;
    app.until_screen("paste").await?;
    app.click("session-id").await?;
    app.type_in("sessionid").await?;
    let hint = "Copy only the value of the sessionid cookie.";
    let seen = app
        .until("the hint under the field", |seen| seen.announced == hint)
        .await?;
    expect_eq(
        node(&seen, "paste-submit")?.disabled,
        Some(true),
        "the button",
    )?;
    app.click("back").await?;
    app.until_screen("start").await.map(drop)
}

async fn refused_id(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    let dead = session_id(world::SOMEONE, "refused");
    stage.lab.world.refuse(&dead);
    let app = stage.open().await?;
    app.until_screen("start").await?;
    app.click("start-paste").await?;
    app.until_screen("paste").await?;
    app.copy(&dead).await?;
    app.click("paste-paste").await?;
    app.click("paste-submit").await?;

    let seen = app.until_screen("failed").await?;
    let why = "That session ID no longer works. Get a new one, or sign in with the browser.";
    expect_eq(label(&seen, "error-why")?.as_str(), why, "the reason")?;
    expect_eq(seen.announced.as_str(), why, "said to a screen reader")?;
    expect_eq(
        node(&seen, "error-details")?.expanded,
        Some(false),
        "the details",
    )?;

    // The code the failure was logged under is in the log.
    app.click("error-details").await?;
    let log = app.log();
    let logged = log.lines().filter(|line| line.contains("\"id\"")).count();
    expect(logged > 0, || {
        "the failure is in no line of the log".to_string()
    })?;

    app.click("back").await?;
    app.until_screen("start").await.map(drop)
}

async fn menu_and_settings(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "menu").await?;
    let app = stage.app()?;
    let seen = app.click("account").await?;
    expect_eq(
        node(&seen, "account-menu")?.role.as_deref(),
        Some("Menu"),
        "the menu's role",
    )?;
    expect_eq(
        label(&seen, "menu-add")?.as_str(),
        "Add account",
        "one account",
    )?;

    app.press("escape").await?;
    let seen = app.look().await?;
    expect(node(&seen, "account-menu").is_err(), || {
        "Escape left the menu open".to_string()
    })?;
    expect_eq(
        node(&seen, "account")?.focused,
        Some(true),
        "focus back on the picture",
    )?;

    app.click("account").await?;
    app.click("menu-settings").await?;
    let seen = app.until_screen("settings").await?;
    expect_eq(
        node(&seen, "settings-detailed")?.checked,
        Some(false),
        "the switch",
    )?;
    let seen = app.click("settings-detailed").await?;
    expect_eq(
        node(&seen, "settings-detailed")?.checked,
        Some(true),
        "the switch, on",
    )?;
    app.click("settings-detailed").await?;

    let seen = app.click("settings-open-logs").await?;
    let logs = snob_engine::log::dir(&app.paths);
    expect_eq(seen.opened.last(), Some(&logs), "the folder opened")?;

    app.click("settings-copy-report").await?;
    let seen = app
        .until("the report on the clipboard", |seen| {
            seen.clipboard.is_some()
        })
        .await?;
    let report = seen.clipboard.unwrap_or_default();
    expect(
        report.starts_with("snob desktop: diagnostic report"),
        || "what was copied is no diagnostic report".to_string(),
    )?;
    expect(!report.contains("@me"), || {
        "the report names the account".to_string()
    })?;
    app.click("rail-home").await?;
    app.until_screen("home").await.map(drop)
}

/// Opens the settings by their keys.
async fn to_the_settings(app: &mut Driven) -> anyhow::Result<Answer> {
    app.press(if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    })
    .await?;
    app.until_screen("settings").await
}

async fn frame_and_sections(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    let app = stage.open().await?;
    let seen = app.until_screen("start").await?;
    expect(node(&seen, "rail").is_err(), || {
        "a rail before anybody is signed in".to_string()
    })?;
    stage.sign_in(world::ME, "frame").await?;
    let app = stage.app()?;
    let seen = app
        .until("the frame open", |seen| {
            bounds(seen, "panel").is_ok_and(|(left, _, width, _)| {
                left >= 63.0 && (left + width - seen.window[0]).abs() < 1.0
            })
        })
        .await?;
    expect_eq(
        node(&seen, "rail")?.role.as_deref(),
        Some("Navigation"),
        "the rail's role",
    )?;
    expect_eq(
        label(&seen, "rail-home")?.as_str(),
        "Home",
        "the first section's name",
    )?;

    // By the pointer, then by the keyboard alone.
    app.click("rail-search").await?;
    app.until_screen("search").await?;
    let mut pressed = 0;
    loop {
        let seen = app.press("tab").await?;
        if node(&seen, "rail-automations")?.focused == Some(true) {
            break;
        }
        pressed += 1;
        if pressed > 12 {
            bail!("Tab never reached the rail");
        }
    }
    app.press("enter").await?;
    app.until_screen("automations").await?;

    // Adding an account closes the frame around the start screen.
    app.click("account").await?;
    app.click("menu-add").await?;
    app.until_screen("start").await?;
    app.until("the rail gone", |seen| node(seen, "rail").is_err())
        .await?;
    app.click("back").await?;
    app.until_screen("home").await?;
    app.until("the rail back", |seen| node(seen, "rail").is_ok())
        .await
        .map(drop)
}

async fn theme_and_language(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "theme").await?;
    let app = stage.app()?;
    let seen = to_the_settings(app).await?;
    expect_eq(seen.dark, false, "the system's light")?;
    expect_eq(
        node(&seen, "theme-system")?.checked,
        Some(true),
        "the system's theme chosen",
    )?;

    let seen = app.click("theme-dark").await?;
    expect_eq(seen.dark, true, "dark once chosen")?;
    app.say(Said::Dark(false)).await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let seen = app.look().await?;
    expect_eq(seen.dark, true, "the system's light does not move it")?;

    let seen = app.click("settings-language").await?;
    expect_eq(
        node(&seen, "settings-language")?.expanded,
        Some(true),
        "the languages listed",
    )?;
    app.click("settings-language-option#2").await?;
    let seen = app
        .until("the words in Spanish", |seen| seen.language == "es")
        .await?;
    expect_eq(
        label(&seen, "title")?.as_str(),
        "Configuraci\u{f3}n",
        "the title in Spanish",
    )?;

    // Kept for the next start, whatever the system says then.
    let app = stage.open().await?;
    let seen = app.until_screen("home").await?;
    expect_eq(seen.dark, true, "dark after a restart")?;
    expect_eq(seen.language.as_str(), "es", "Spanish after a restart")
}

async fn settings_search(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "search").await?;
    let app = stage.app()?;
    to_the_settings(app).await?;
    app.click("settings-search").await?;
    let seen = app.type_in("cach").await?;
    expect(node(&seen, "settings-clear-cache").is_ok(), || {
        "the cache is not found by its name".to_string()
    })?;
    expect(node(&seen, "settings-theme").is_err(), || {
        "the theme is shown for \"cach\"".to_string()
    })?;
    let seen = app.type_in("zzz").await?;
    expect_eq(
        label(&seen, "settings-none")?.as_str(),
        "No setting matches \u{201c}cachzzz\u{201d}",
        "what is said when nothing matches",
    )
}

async fn clearing_the_cache(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "cache").await?;
    let app = stage.app()?;
    let account = app.paths.account(snob_core::Pk::new(world::ME));
    let cache = account.cache_dir();
    expect(cache.exists(), || {
        "the account's picture was kept nowhere".to_string()
    })?;
    to_the_settings(app).await?;
    // Found by its name, so its line is in view whatever the window's size.
    app.click("settings-search").await?;
    app.type_in("cache").await?;
    app.until("what is kept measured", |seen| {
        node(seen, "settings-clear-cache").is_ok_and(|button| button.disabled != Some(true))
    })
    .await?;
    app.click("settings-clear-cache").await?;
    let seen = app
        .until("the cache cleared", |seen| {
            label(seen, "settings-accounts-note").is_ok_and(|note| note.ends_with("freed"))
        })
        .await?;
    expect(
        std::fs::read_dir(&cache).map_or(true, |mut left| left.next().is_none()),
        || "files are left in the cache".to_string(),
    )?;
    expect_eq(seen.screen.as_str(), "settings", "still in the settings")?;
    expect(account.dir().exists(), || {
        "the account's directory went with the cache".to_string()
    })?;
    // The session stays: a start goes home.
    let app = stage.open().await?;
    app.until_screen("home").await.map(drop)
}

async fn forgetting_history(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "forget").await?;
    let app = stage.app()?;
    to_the_settings(app).await?;
    app.click("settings-search").await?;
    app.type_in("history").await?;
    app.until("what is kept measured", |seen| {
        node(seen, "storage-account-0-forget").is_ok_and(|button| button.disabled != Some(true))
    })
    .await?;

    // A no deletes nothing.
    app.click("storage-account-0-forget").await?;
    let seen = app
        .until("the question asked", |seen| {
            node(seen, "forget-title").is_ok()
        })
        .await?;
    expect(
        label(&seen, "forget-body")?.contains("cannot be undone"),
        || "the question does not say it cannot be undone".to_string(),
    )?;
    app.press("escape").await?;
    let seen = app
        .until("the question gone", |seen| {
            node(seen, "forget-title").is_err()
        })
        .await?;
    expect(
        !label(&seen, "storage-account-0-note")?.ends_with("freed"),
        || "a no deleted something".to_string(),
    )?;

    app.click("storage-account-0-forget").await?;
    app.until("the question asked again", |seen| {
        node(seen, "forget-title").is_ok()
    })
    .await?;
    app.click("forget-confirm").await?;
    let seen = app
        .until("the history deleted", |seen| {
            label(seen, "storage-account-0-note").is_ok_and(|note| note.ends_with("freed"))
        })
        .await?;
    expect_eq(seen.screen.as_str(), "settings", "still in the settings")?;
    // The session stays: a start goes home.
    let app = stage.open().await?;
    app.until_screen("home").await.map(drop)
}

async fn outline_while_dragging(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "outline").await?;
    let app = stage.app()?;
    app.resize(900.0, 560.0).await?;
    let seen = to_the_settings(app).await?;
    let (left, top, width, height) = bounds(&seen, "settings-list")?;
    // The thumb, under the top of a list scrolled a little.
    let (x, y) = (left + width - 13.0, top + height * 0.3);
    for _ in 0..2 {
        app.point(left + width / 2.0, y, Pointing::Wheel(30.0))
            .await?;
    }
    let seen = app.point(x, y, Pointing::Move).await?;
    expect(node(&seen, "settings-outline").is_err(), || {
        "the outline shows without a drag".to_string()
    })?;
    app.point(x, y, Pointing::Down).await?;
    let seen = app
        .until("the outline shown", |seen| {
            node(seen, "settings-outline").is_ok()
        })
        .await?;
    expect_eq(
        node(&seen, "settings-outline")?.role.as_deref(),
        Some("Group"),
        "the outline's role",
    )?;
    app.point(x, y + 40.0, Pointing::Drag).await?;
    app.point(x, y + 40.0, Pointing::Up).await?;
    app.until("the outline gone", |seen| {
        node(seen, "settings-outline").is_err()
    })
    .await
    .map(drop)
}

async fn switch_accounts(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "switch").await?;
    let app = stage.app()?;
    app.click("account").await?;
    app.click("menu-add").await?;
    stage.sign_in(world::SOMEONE, "switch").await?;

    let app = stage.app()?;
    let seen = app.look().await?;
    expect_eq(
        label(&seen, "account-photo")?.as_str(),
        "Profile picture of @someone",
        "the account that signed in last",
    )?;
    app.click("account").await?;
    let seen = app.click("menu-switch").await?;
    expect_eq(
        label(&seen, "menu-account#0")?.as_str(),
        "@me",
        "the other account",
    )?;
    app.click("menu-account#0").await?;
    let seen = app
        .until("the other account in use", |seen| {
            label(seen, "account-photo").is_ok_and(|said| said == "Profile picture of @me")
        })
        .await?;
    expect_eq(
        seen.announced.as_str(),
        "Now using @me",
        "said to a screen reader",
    )?;

    // And it is the one in use at the next start.
    let app = stage.open().await?;
    let seen = app.until_screen("home").await?;
    expect_eq(
        label(&seen, "account-photo")?.as_str(),
        "Profile picture of @me",
        "the account in use after a restart",
    )
}

async fn signing_out(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "out").await?;
    let app = stage.app()?;
    app.click("account").await?;
    let seen = app.click("menu-sign-out").await?;
    expect_eq(
        label(&seen, "sign-out")?.as_str(),
        "Sign out of @me?",
        "the question",
    )?;
    expect_eq(
        node(&seen, "sign-out-cancel")?.focused,
        Some(true),
        "focus on no",
    )?;
    app.press("escape").await?;
    let seen = app.look().await?;
    expect_eq(seen.screen.as_str(), "home", "after saying no")?;

    app.click("account").await?;
    app.click("menu-sign-out").await?;
    app.click("sign-out-confirm").await?;
    app.until_screen("start").await?;
    let card = app.paths.account(world::ME.into()).viewer_card();
    expect(!card.exists(), || {
        "the account's card outlived its session".to_string()
    })?;

    let app = stage.open().await?;
    app.until_screen("start").await.map(drop)
}

async fn offline(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    let app = stage.open().await?;
    app.until_screen("start").await?;
    app.say(Said::Online(false)).await?;
    let seen = app
        .until("the band says there is no network", |seen| {
            seen.status.as_deref() == Some("No internet connection")
        })
        .await?;
    expect_eq(
        seen.announced.as_str(),
        "No internet connection",
        "said to a screen reader",
    )?;
    expect_eq(
        node(&seen, "status")?.role.as_deref(),
        Some("Status"),
        "the status's role",
    )?;
    app.say(Said::Online(true)).await?;
    app.until("the band says nothing", |seen| seen.status.is_none())
        .await
        .map(drop)
}

async fn sleep_and_wake(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "sleeps").await?;
    let app = stage.app()?;
    app.say(Said::Sleeping).await?;
    app.say(Said::Woke(2 * 60 * 60)).await?;
    // Back as it was: the same screen, and it still answers.
    let seen = app.click("account").await?;
    expect_eq(
        seen.screen.as_str(),
        "home",
        "the screen after two hours asleep",
    )?;
    expect(node(&seen, "account-menu").is_ok(), || {
        "the app does not answer after waking".to_string()
    })?;
    let log = app.log();
    expect(log.contains("\"slept_secs\":7200"), || {
        "how long it slept is not in the log".to_string()
    })
}

async fn second_instance(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    let (exe, root, start) = (
        stage.lab.exe.clone(),
        stage.root.clone(),
        stage.start.clone(),
    );
    let mut second = crate::drive::spawn(&exe, &root, &start)?;
    let mut left = None;
    for _ in 0..200 {
        if let Some(status) = second.try_wait()? {
            left = Some(status);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let Some(status) = left else {
        let _ = second.kill();
        bail!("the second start stayed open");
    };
    expect(status.success(), || {
        format!("the second start left with {status}")
    })?;

    let app = stage.app()?;
    app.until_screen("start").await?;
    let since = std::time::Instant::now();
    while !app.log().contains("brought to the front by a second start") {
        expect(since.elapsed() < Duration::from_secs(10), || {
            "the first start was never told to come to the front".to_string()
        })?;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

async fn follows_the_system(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    let app = stage.open().await?;
    let seen = app.until_screen("start").await?;
    let (_, _, _, height) = bounds(&seen, "start-browser")?;

    app.say(Said::Locale("es-ES".to_string())).await?;
    let seen = app
        .until("the words in Spanish", |seen| {
            label(seen, "title").is_ok_and(|title| title == "Empecemos.") // i18n-allow
        })
        .await?;
    expect_eq(seen.screen.as_str(), "start", "the screen after the change")?;

    app.say(Said::Dark(true)).await?;
    app.say(Said::MoreContrast(true)).await?;
    app.say(Said::TextScale(2.0)).await?;
    let seen = app
        .until("text twice as large", |seen| {
            bounds(seen, "start-browser").is_ok_and(|(_, _, _, now)| now > height * 1.8)
        })
        .await?;
    // Nothing is cut or pushed out of the window at twice the size.
    for id in ["title", "start-browser", "start-paste"] {
        let (left, top, width, tall) = bounds(&seen, id)?;
        expect(
            left >= 0.0 && top >= 0.0 && left + width <= seen.window[0] && tall > 0.0,
            || format!("{id} is cut at twice the text size"),
        )?;
    }
    Ok(())
}

async fn no_secret_is_written(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    let secret = session_id(world::ME, "PrivateTokenZq");
    let dead = session_id(world::SOMEONE, "PrivateTokenZq");
    stage.lab.world.refuse(&dead);

    stage.open().await?;
    let app = stage.app()?;
    app.until_screen("start").await?;
    app.click("start-paste").await?;
    app.until_screen("paste").await?;
    app.copy(&dead).await?;
    app.click("paste-paste").await?;
    // Shown to the eye is still not read out.
    let seen = app.click("paste-reveal").await?;
    for found in &seen.nodes {
        for said in [&found.label, &found.value, &found.description]
            .into_iter()
            .flatten()
        {
            expect(!said.contains("PrivateTokenZq"), || {
                format!("{} tells the session ID", found.id)
            })?;
        }
    }
    app.click("paste-submit").await?;
    app.until_screen("failed").await?;
    app.click("back").await?;
    stage.sign_in(world::ME, "PrivateTokenZq").await?;

    let app = stage.app()?;
    app.click("account").await?;
    app.click("menu-settings").await?;
    app.until_screen("settings").await?;
    app.click("settings-detailed").await?;
    app.click("settings-copy-report").await?;
    let seen = app
        .until("the report on the clipboard", |seen| {
            seen.clipboard
                .as_deref()
                .is_some_and(|text| text.starts_with("snob desktop"))
        })
        .await?;
    let report = seen.clipboard.unwrap_or_default();
    let log = app.log();
    expect(log.lines().count() > 10, || {
        "next to nothing was logged".to_string()
    })?;
    for (what, written) in [("the log", &log), ("the diagnostic report", &report)] {
        expect(!written.contains("PrivateTokenZq"), || {
            format!("{what} holds a session ID")
        })?;
    }
    expect(
        !report.contains("@me") && !report.contains("@someone"),
        || "the diagnostic report names an account".to_string(),
    )?;
    // Everything the run wrote, wherever it wrote it.
    let leaked = files_holding(&stage.root, "PrivateTokenZq", &secret);
    expect(leaked.is_empty(), || {
        format!("the session ID is in plain files: {leaked:?}")
    })
}

async fn keyboard_alone(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    let app = stage.open().await?;
    app.until_screen("start").await?;
    // Tab to the second button, and press it.
    app.press("tab").await?;
    let seen = app.press("tab").await?;
    expect_eq(
        node(&seen, "start-paste")?.focused,
        Some(true),
        "focus after two Tabs",
    )?;
    app.press("enter").await?;
    app.until_screen("paste").await?;

    // The field is the first stop; type the ID and press Enter.
    app.press("tab").await?;
    app.type_in(&session_id(world::ME, "keys")).await?;
    app.until("the button on", |seen| {
        node(seen, "paste-submit").is_ok_and(|button| button.disabled != Some(true))
    })
    .await?;
    app.press("enter").await?;
    app.until_screen("home").await?;

    // The settings by their shortcut, and back.
    app.press(if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    })
    .await?;
    app.until_screen("settings").await.map(drop)
}

/// Two automations and a short history of the first, kept where the app
/// finds them: what a list of them and a page with a history are shown
/// with. **The same on every run**: the moments are fixed.
pub fn seed_automations(root: &Path) -> anyhow::Result<()> {
    use snob_core::Epoch;
    use snob_core::automation::{Account, Automation, Read, Step, Webhook};
    use snob_core::watch::schedule::Spec;
    use snob_store::paths::AppPaths;

    let paths = AppPaths::rooted_at(root);
    let every = |text: &str| Spec {
        every: Some(snob_core::duration::parse(text).unwrap_or_default()),
        ..Spec::default()
    };
    let mut daily = Automation::new("00000a01", "Daily followers");
    let trigger = daily.insert(Step::Schedule { when: every("6h") }, (-345, -40));
    let lists = daily.insert(
        Step::Read {
            account: Some(Account::own(None)),
            what: Read::Lists,
        },
        (-115, -40),
    );
    let changes = daily.insert(
        Step::Filter {
            only: snob_core::automation::Only::Changes,
        },
        (115, -40),
    );
    let hook = daily.insert(
        Step::Webhook(Webhook {
            url: "https://example.com/hook".to_string(),
            ..Webhook::default()
        }),
        (345, -40),
    );
    for (from, to) in [(trigger, lists), (lists, changes), (changes, hook)] {
        daily.connect(from, to).map_err(|why| anyhow!("{why}"))?;
    }
    let mut pictures = Automation::new("00000a02", "Profile pictures");
    let weekly = Spec {
        on: vec!["mon".to_string()],
        at: vec!["09:00".to_string()],
        ..Spec::default()
    };
    pictures.insert(Step::Schedule { when: weekly }, (-345, -40));
    for automation in [&daily, &pictures] {
        snob_store::automations::save(&paths, automation)
            .map_err(|e| anyhow!("an automation was not kept: {e}"))?;
    }
    let shared = snob_store::store::shared::Shared::open(&paths)
        .map_err(|e| anyhow!("the shared database did not open: {e}"))?;
    let runs = [
        (
            1_790_000_000,
            "ok",
            14,
            "Read 120 followers and 98 following.
Two came, one went.",
        ),
        (
            1_789_978_400,
            "rate_limited",
            6,
            "Instagram asked for a pause after the first list.",
        ),
        (
            1_789_956_800,
            "ok",
            14,
            "Read 119 followers and 98 following.
Nothing changed.",
        ),
    ];
    for (at, outcome, requests, summary) in runs {
        let id = shared
            .record_automation_run("00000a01", Epoch::new(at))
            .map_err(|e| anyhow!("a run was not written down: {e}"))?;
        shared
            .finish_automation_run(id, Epoch::new(at + 95), outcome, requests, summary)
            .map_err(|e| anyhow!("a run was not finished: {e}"))?;
    }
    Ok(())
}

/// Left, top, width and height of `id`.
pub fn bounds(seen: &Answer, id: &str) -> anyhow::Result<(f32, f32, f32, f32)> {
    let [left, top, width, height] = node(seen, id)?.bounds;
    Ok((left, top, width, height))
}

/// The files under `root` that hold `needle`, but for the one the session
/// is kept in, which is what holds `whole` and is where it belongs.
fn files_holding(root: &Path, needle: &str, whole: &str) -> Vec<PathBuf> {
    let mut holding = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            let is_the_session = path.file_name().is_some_and(|name| name == "session.json");
            if text.contains(needle) && !(is_the_session && text.contains(whole)) {
                holding.push(path);
            }
        }
    }
    holding
}

/// The middle of `id` across the window.
fn middle_of(seen: &Answer, id: &str) -> anyhow::Result<f32> {
    let (left, _, width, _) = bounds(seen, id)?;
    Ok(left + width / 2.0)
}

async fn automations_introduced_once(stage: &mut Stage<'_>) -> anyhow::Result<()> {
    stage.open().await?;
    stage.sign_in(world::ME, "automations").await?;
    let app = stage.app()?;
    app.click("rail-automations").await?;
    app.until_screen("automations").await?;
    let seen = app
        .until("the introduction", |seen| {
            node(seen, "automations-begin").is_ok()
        })
        .await?;
    expect_eq(label(&seen, "title")?.as_str(), "Automations", "the title")?;
    // Under the brand, on the window's middle, not the panel's.
    let (title, brand) = (middle_of(&seen, "title")?, middle_of(&seen, "brand")?);
    expect((title - brand).abs() < 1.0, || {
        format!("the title's middle is at {title}, the brand's at {brand}")
    })?;

    app.click("automations-begin").await?;
    app.until("the list", |seen| node(seen, "automations-create").is_ok())
        .await?;

    // Not again, after a restart.
    let app = stage.open().await?;
    app.until_screen("home").await?;
    app.click("rail-automations").await?;
    app.until_screen("automations").await?;
    let seen = app
        .until("the list", |seen| node(seen, "automations-create").is_ok())
        .await?;
    expect(node(&seen, "automations-begin").is_err(), || {
        "the introduction came back after a restart".to_string()
    })?;

    // The editor: the wheel zooms its canvas, Escape comes back.
    app.click("automations-create").await?;
    let seen = app
        .until("the editor", |seen| node(seen, "editor-canvas").is_ok())
        .await?;
    expect_eq(
        label(&seen, "title")?.as_str(),
        "Edit automation",
        "the editor's title",
    )?;
    let (left, top, width, height) = bounds(&seen, "editor-canvas")?;
    app.point(
        left + width / 2.0,
        top + height / 2.0,
        Pointing::Wheel(-240.0),
    )
    .await?;
    let seen = app
        .until("the canvas zoomed", |seen| {
            label(seen, "editor-zoom-reset").is_ok_and(|said| !said.starts_with("Zoom at 100 "))
        })
        .await?;
    expect(
        label(&seen, "editor-zoom-reset")?.starts_with("Zoom at 1"),
        || "turned up, the wheel did not zoom in".to_string(),
    )?;
    app.press("escape").await?;
    app.until("the list again", |seen| {
        node(seen, "automations-create").is_ok()
    })
    .await
    .map(drop)
}
