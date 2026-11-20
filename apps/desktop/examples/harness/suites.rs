//! The suites: what each runs, and what it found, one [`Outcome`] per
//! thing checked.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use snob_desktop::control::{Answer, Said};
use snob_fake_ig::world::{self, World};

use crate::drive::{Driven, Start, expect, label, node};
use crate::scenarios::{self, Lab, Scenario, Stage};
use crate::{Asked, app_dir, report};

/// What one check found.
pub struct Outcome {
    pub suite: &'static str,
    pub name: String,
    pub passed: bool,
    pub took: Duration,
    /// Why it failed, or what is worth knowing about a pass.
    pub detail: String,
    /// Pictures to show beside it: what it is, and where it is.
    pub pictures: Vec<(String, PathBuf)>,
    /// The app's log of a failed run.
    pub log: Option<PathBuf>,
    since: Instant,
}

impl Outcome {
    pub fn of(suite: &'static str, name: impl Into<String>, since: Instant) -> Self {
        Self {
            suite,
            name: name.into(),
            passed: true,
            took: since.elapsed(),
            detail: String::new(),
            pictures: Vec::new(),
            log: None,
            since,
        }
    }

    /// It ended now: how long it took is counted up to here.
    pub fn ended(mut self) -> Self {
        self.took = self.since.elapsed();
        self
    }

    pub fn failed(mut self, why: impl Into<String>) -> Self {
        self.passed = false;
        self.detail = why.into();
        self.ended()
    }

    fn noted(mut self, what: impl Into<String>) -> Self {
        self.detail = what.into();
        self.ended()
    }
}

/// A name as part of a file's.
fn slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// The app's test build, built now: beside this program.
fn test_build() -> anyhow::Result<PathBuf> {
    println!("building the test build of the app...");
    let built = Command::new(env!("CARGO"))
        .args(["build", "--features", "testing", "--bin", "snob-desktop"])
        .current_dir(app_dir())
        .status()
        .context("running cargo")?;
    if !built.success() {
        bail!("the test build of the app does not build");
    }
    let here = std::env::current_exe()?;
    let profile = here
        .parent()
        .and_then(Path::parent)
        .context("this program is not in a target directory")?;
    let exe = profile.join(format!("snob-desktop{}", std::env::consts::EXE_SUFFIX));
    if !exe.is_file() {
        bail!("no app at {}", exe.display());
    }
    Ok(exe)
}

/// A scenario's own corner of `out`.
fn stage<'a>(lab: &'a Lab, out: &Path, name: &str, start: Start) -> anyhow::Result<Stage<'a>> {
    let root = out.join("roots").join(slug(name));
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    std::fs::create_dir_all(&root)?;
    Ok(Stage {
        lab,
        root,
        app: None,
        start,
        film: None,
    })
}

/// Runs `scenario`, and keeps a picture and the log when it fails.
async fn run_scenario(suite: &'static str, lab: &Lab, out: &Path, scenario: &Scenario) -> Outcome {
    let since = Instant::now();
    let start = Start {
        instagram: Some(lab.instagram.clone()),
        reduce_motion: true,
        ..Start::default()
    };
    let mut stage = match stage(lab, out, scenario.name, start) {
        Ok(stage) => stage,
        Err(e) => return Outcome::of(suite, scenario.name, since).failed(format!("{e:#}")),
    };
    let film = out.join("film").join(slug(scenario.name));
    let _ = std::fs::remove_dir_all(&film);
    stage.film = Some(film.clone());
    let ran = (scenario.run)(&mut stage).await;
    let mut outcome = Outcome::of(suite, scenario.name, since)
        .noted(format!("sections {} of the design", scenario.covers));
    if ran.is_ok() {
        let _ = std::fs::remove_dir_all(&film);
    }
    if let Err(e) = ran {
        outcome = outcome.failed(format!("{e:#}"));
        // What led to it, step by step: the last frames of its film.
        let mut frames: Vec<PathBuf> = std::fs::read_dir(&film)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        frames.sort();
        let from = frames.len().saturating_sub(8);
        for (at, frame) in frames.into_iter().enumerate().skip(from) {
            outcome.pictures.push((format!("step {}", at + 1), frame));
        }
        let failures = out.join("failures");
        if let Some(app) = stage.app.as_mut() {
            let picture = failures.join(format!("{}.png", slug(scenario.name)));
            if app.capture(&picture).await.is_ok() {
                outcome
                    .pictures
                    .push(("when it failed".to_string(), picture));
            }
            let log = failures.join(format!("{}.log", slug(scenario.name)));
            if std::fs::create_dir_all(&failures).is_ok() && std::fs::write(&log, app.log()).is_ok()
            {
                outcome.log = Some(log);
            }
        }
    }
    let _ = stage.close().await;
    outcome
}

/// The app's unit tests and its tests of the window in process.
///
/// **A run that fails no test is run once more**, and the report keeps what
/// cargo said the first time: on Windows a test binary just linked can be
/// held open a moment (by an antivirus scan, or a process a test left), which
/// fails the build, and a real failure fails twice.
fn logic() -> Vec<Outcome> {
    let since = Instant::now();
    let outcome = Outcome::of("logic", "cargo test --features testing", since);
    let first = match cargo_test() {
        Ok(ran) => ran,
        Err(e) => return vec![outcome.failed(format!("cargo did not run: {e}"))],
    };
    let (ran, retried) = match &first {
        Tested::Failed { failed, .. } if failed.is_empty() => match cargo_test() {
            Ok(again) => (again, Some(first)),
            Err(e) => return vec![outcome.failed(format!("cargo did not run: {e}"))],
        },
        _ => (first, None),
    };
    let before = retried.map(|first| match first {
        Tested::Failed { stderr, .. } => format!(
            "\nThe first run failed no test, and cargo said:\n{}",
            last_lines(&stderr)
        ),
        Tested::Passed(_) => String::new(),
    });
    match ran {
        Tested::Passed(passed) => vec![outcome.noted(format!(
            "{passed} tests passed{}",
            before.unwrap_or_default()
        ))],
        Tested::Failed { failed, stderr } if failed.is_empty() => vec![outcome.failed(format!(
            "{}{}",
            last_lines(&stderr),
            before.unwrap_or_default()
        ))],
        Tested::Failed { failed, .. } => vec![outcome.failed(failed.join("\n"))],
    }
}

/// What one `cargo test` of the app came to.
enum Tested {
    Passed(usize),
    /// The tests that failed, none when the build or a test binary did, and
    /// what cargo said.
    Failed {
        failed: Vec<String>,
        stderr: String,
    },
}

fn cargo_test() -> std::io::Result<Tested> {
    let ran = Command::new(env!("CARGO"))
        .args(["test", "--features", "testing", "--lib", "--tests"])
        .current_dir(app_dir())
        .output()?;
    let said = String::from_utf8_lossy(&ran.stdout);
    if ran.status.success() {
        let passed = said
            .lines()
            .filter(|line| line.starts_with("test result:"))
            .filter_map(|line| line.split_whitespace().nth(3)?.parse::<usize>().ok())
            .sum();
        return Ok(Tested::Passed(passed));
    }
    Ok(Tested::Failed {
        failed: said
            .lines()
            .filter(|line| line.ends_with("FAILED") || line.starts_with("---- "))
            .map(str::to_string)
            .collect(),
        stderr: String::from_utf8_lossy(&ran.stderr).into_owned(),
    })
}

/// The last [`STDERR_LINES`] of what cargo said.
fn last_lines(said: &str) -> String {
    let lines: Vec<&str> = said.lines().collect();
    lines[lines.len().saturating_sub(STDERR_LINES)..].join("\n")
}

/// How many of cargo's last lines a failure without a failed test shows.
const STDERR_LINES: usize = 40;

/// The screens a picture is taken of and an audit made of, and how each is
/// reached from the screen before it.
const SCREENS: [&str; 38] = [
    "start",
    "paste",
    "failed",
    "home",
    "menu",
    "menu-accounts",
    "sign-out",
    "settings",
    "settings-languages",
    "palette",
    "settings-search",
    "settings-storage",
    "settings-accounts",
    "settings-forget",
    "search",
    "automations",
    "automations-list",
    "automations-editor",
    "automations-listed",
    "automation-page",
    "automation-flow",
    "automation-search",
    "automation-menu",
    "automation-schedule",
    "automation-schedule-month",
    "automation-schedule-advanced",
    "automation-leave",
    "profile",
    "profile-split",
    "profile-options",
    "profile-about",
    "viewer",
    "people",
    "people-options",
    "people-selecting",
    "list-question",
    "automation-suggestions",
    "paste-browsers",
];

/// The settings' keys.
const SETTINGS_KEYS: &str = if cfg!(target_os = "macos") {
    "cmd-,"
} else {
    "ctrl-,"
};

/// Takes the app from one of [`SCREENS`] to the next.
async fn reach(stage: &mut Stage<'_>, screen: &str) -> anyhow::Result<()> {
    match screen {
        "start" => {
            stage.open().await?.until_screen("start").await?;
        }
        "paste" => {
            let app = stage.app()?;
            app.click("start-paste").await?;
            app.until_screen("paste").await?;
        }
        "failed" => {
            let dead = scenarios::session_id(world::GHOST, "screens");
            stage.lab.world.refuse(&dead);
            let app = stage.app()?;
            app.copy(&dead).await?;
            app.click("paste-paste").await?;
            app.click("paste-submit").await?;
            app.until_screen("failed").await?;
        }
        "home" => {
            stage.app()?.click("back").await?;
            stage.sign_in(world::ME, "screens").await?;
            let app = stage.app()?;
            app.click("account").await?;
            app.click("menu-add").await?;
            stage.sign_in(world::SOMEONE, "screens").await?;
        }
        "menu" => {
            stage.app()?.click("account").await?;
        }
        "menu-accounts" => {
            stage.app()?.click("menu-switch").await?;
        }
        "sign-out" => {
            stage.app()?.click("menu-sign-out").await?;
        }
        "settings" => {
            let app = stage.app()?;
            app.press("escape").await?;
            app.press(SETTINGS_KEYS).await?;
            app.until_screen("settings").await?;
        }
        "settings-languages" => {
            stage.app()?.click("settings-language").await?;
        }
        "settings-search" => {
            let app = stage.app()?;
            app.press("escape").await?;
            app.click("settings-search").await?;
            app.type_in("cach").await?;
        }
        "settings-storage" => {
            // Come again, nothing typed, and turned down to what is kept.
            let app = stage.app()?;
            app.click("rail-home").await?;
            app.until_screen("home").await?;
            app.press(SETTINGS_KEYS).await?;
            let seen = app.until_screen("settings").await?;
            let (left, top, width, height) = crate::scenarios::bounds(&seen, "settings-list")?;
            app.point(
                left + width / 2.0,
                top + height / 2.0,
                snob_desktop::control::Pointing::Wheel(330.0),
            )
            .await?;
            // The scrollbar shown by the turn has gone again: a picture
            // taken while it fades would never be the same twice.
            tokio::time::sleep(Duration::from_millis(2300)).await;
        }
        "settings-accounts" => {
            // Turned down to the end: the accounts and what they hold.
            let app = stage.app()?;
            let seen = app.look().await?;
            let (left, top, width, height) = crate::scenarios::bounds(&seen, "settings-list")?;
            app.point(
                left + width / 2.0,
                top + height / 2.0,
                snob_desktop::control::Pointing::Wheel(2000.0),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(2300)).await;
        }
        "settings-forget" => {
            stage.app()?.click("settings-forget-all").await?;
        }
        "search" => {
            let app = stage.app()?;
            app.press("escape").await?;
            app.click("rail-search").await?;
            app.until_screen("search").await?;
        }
        "automations" => {
            let app = stage.app()?;
            app.click("rail-automations").await?;
            app.until_screen("automations").await?;
        }
        "automations-list" => {
            let app = stage.app()?;
            app.click("automations-begin").await?;
            app.until("the list of automations", |seen| {
                scenarios::bounds(seen, "automations-create").is_ok()
            })
            .await?;
        }
        "automations-editor" => {
            let app = stage.app()?;
            app.click("automations-create").await?;
            app.until("the editor", |seen| {
                scenarios::bounds(seen, "editor-canvas").is_ok()
            })
            .await?;
        }
        "automations-listed" => {
            // Back from the empty editor, two automations kept, and the
            // section entered again so that it reads them.
            let app = stage.app()?;
            app.press("escape").await?;
            app.until("the list", |seen| {
                scenarios::bounds(seen, "automations-create").is_ok()
            })
            .await?;
            scenarios::seed_automations(&stage.root)?;
            let app = stage.app()?;
            app.click("rail-home").await?;
            app.until_screen("home").await?;
            app.click("rail-automations").await?;
            app.until_screen("automations").await?;
            app.until("the automations", |seen| {
                scenarios::bounds(seen, "automation#0").is_ok()
            })
            .await?;
        }
        "automation-page" => {
            let app = stage.app()?;
            app.click("automation#0").await?;
            app.until("the history", |seen| {
                scenarios::bounds(seen, "automation-run-row#0").is_ok()
            })
            .await?;
        }
        "automation-flow" => {
            let app = stage.app()?;
            app.click("automation-edit").await?;
            app.until("the flow", |seen| scenarios::bounds(seen, "node#4").is_ok())
                .await?;
        }
        "automation-search" => {
            // The actions, and the search open over their name, a word
            // typed in it.
            let app = stage.app()?;
            app.click("editor-actions").await?;
            app.click("editor-shelf-search").await?;
            app.until("the search open", |seen| {
                node(seen, "editor-shelf-field").is_ok()
            })
            .await?;
            app.type_in("fol").await?;
        }
        "automation-menu" => {
            // A step is chosen and its menu opened with the keyboard.
            let app = stage.app()?;
            app.click("node#2").await?;
            app.press("enter").await?;
            app.until("the menu", |seen| {
                scenarios::bounds(seen, "node-menu").is_ok()
            })
            .await?;
        }
        "automation-schedule" => {
            let app = stage.app()?;
            app.press("escape").await?;
            app.click("node#1").await?;
            app.press("enter").await?;
            app.click("node-menu-configure").await?;
            app.until("the dialog", |seen| {
                scenarios::bounds(seen, "step-dialog").is_ok()
            })
            .await?;
        }
        "automation-schedule-month" => {
            // Every month, on one weekday of it.
            let app = stage.app()?;
            app.click("schedule-frequency-month").await?;
            app.click("schedule-by-weekday").await?;
            app.until("a weekday of the month", |seen| {
                node(seen, "schedule-nth#0").is_ok()
            })
            .await?;
        }
        "automation-schedule-advanced" => {
            // The same schedule, as `watch.toml` writes it.
            let app = stage.app()?;
            app.click("schedule-frequency-advanced").await?;
            app.until("the fields of watch.toml", |seen| {
                node(seen, "schedule-frequency-advanced").is_ok_and(|one| one.checked == Some(true))
            })
            .await?;
        }
        "automation-leave" => {
            // A step taken off leaves the action without a trigger.
            let app = stage.app()?;
            app.press("escape").await?;
            app.until("the dialog closed", |seen| {
                node(seen, "step-dialog").is_err()
            })
            .await?;
            app.click("node#1").await?;
            app.press("enter").await?;
            app.click("node-menu-remove").await?;
            app.click("editor-back").await?;
            app.until("the question", |seen| {
                scenarios::bounds(seen, "leave-question").is_ok()
            })
            .await?;
        }
        "profile" => {
            // The account in use is `someone`. What the editor asked is
            // answered first. **One page of posts**: a test build has no
            // browser, and the engine reads a profile's posts past their
            // first page from the browser alone, so what is said while it
            // reads on, rests or has read them all is not pictured here
            // (`tests/profile.rs` has each, on the script).
            let app = stage.app()?;
            let seen = app.look().await?;
            if scenarios::bounds(&seen, "leave-discard").is_ok() {
                app.click("leave-discard").await?;
            }
            app.click("account").await?;
            app.click("menu-profile").await?;
            app.until_screen("profile").await?;
            app.until("the profile read", scenarios::profile_read)
                .await?;
            pictures_settled(stage).await;
        }
        "profile-split" => {
            stage.app()?.click("profile-layout").await?;
        }
        "profile-options" => {
            // Under its button, in the split layout.
            stage.app()?.click("profile-options").await?;
        }
        "profile-about" => {
            // From the options open on the screen before.
            let app = stage.app()?;
            app.click("profile-about-open").await?;
            app.until("about this account", |seen| {
                node(seen, "profile-about").is_ok()
            })
            .await?;
        }
        "viewer" => {
            // What the screen before opened closed first; with a story up,
            // the picture is chosen in the small menu.
            let app = stage.app()?;
            app.press("escape").await?;
            let seen = app.click("profile-picture").await?;
            if seen
                .nodes
                .iter()
                .any(|node| node.id == "profile-open-picture")
            {
                app.click("profile-open-picture").await?;
            }
        }
        "people" => {
            let app = stage.app()?;
            app.press("escape").await?;
            app.click("profile-followers").await?;
            app.until_screen("people").await?;
            app.until("the list read", scenarios::list_read).await?;
            pictures_settled(stage).await;
        }
        "people-options" => {
            let app = stage.app()?;
            app.click("people-options").await?;
            app.until("the list's options", |seen| {
                node(seen, "people-menu").is_ok()
            })
            .await?;
        }
        "people-selecting" => {
            // Three rows chosen, and the bar over the list.
            let app = stage.app()?;
            app.click("people-select").await?;
            app.until("rows to choose", |seen| {
                node(seen, "people-selection").is_ok()
            })
            .await?;
            for row in ["person#1", "person#2", "person#4"] {
                app.click(row).await?;
            }
            app.until("three rows chosen", |seen| {
                label(seen, "people-selected").is_ok_and(|said| said.starts_with('3'))
            })
            .await?;
        }
        "list-question" => {
            // No row chosen any more; another account's profile, and its
            // list asked about.
            let app = stage.app()?;
            app.click("people-cancel").await?;
            app.until("no row chosen", |seen| {
                node(seen, "people-selection").is_err()
            })
            .await?;
            app.click("person#0").await?;
            app.until_screen("profile").await?;
            app.until("the profile read", scenarios::profile_read)
                .await?;
            app.click("profile-followers").await?;
        }
        "automation-suggestions" => {
            // No to the question; back in the editor of the first
            // automation, its action read of another account, and a name
            // begun that the accounts the list read go on.
            let app = stage.app()?;
            app.press("escape").await?;
            app.until("the question gone", |seen| {
                node(seen, "list-question").is_err()
            })
            .await?;
            app.click("rail-automations").await?;
            app.until_screen("automations").await?;
            let seen = app
                .until("the automations or the first one", |seen| {
                    node(seen, "automation#0").is_ok() || node(seen, "automation-edit").is_ok()
                })
                .await?;
            if node(&seen, "automation#0").is_ok() {
                app.click("automation#0").await?;
            }
            app.until("its page", |seen| node(seen, "automation-edit").is_ok())
                .await?;
            app.click("automation-edit").await?;
            app.until("the flow", |seen| node(seen, "node#2").is_ok())
                .await?;
            app.click("node#2").await?;
            app.press("enter").await?;
            app.click("node-menu-configure").await?;
            app.until("the dialog", |seen| node(seen, "step-dialog").is_ok())
                .await?;
            app.click("action-other").await?;
            app.click("action-username").await?;
            app.type_in("user").await?;
            // The accounts known, and last the name as typed: no account
            // is called that.
            app.until("the accounts known suggested", |seen| {
                node(seen, "action-suggestion#1").is_ok()
            })
            .await?;
            pictures_settled(stage).await;
        }
        "palette" => {
            let app = stage.app()?;
            // The languages' list, open on the screen before, closes first.
            app.press("escape").await?;
            app.press(if cfg!(target_os = "macos") {
                "cmd-k"
            } else {
                "ctrl-k"
            })
            .await?;
        }
        "paste-browsers" => {
            // The same computer with three browsers listed, and the list of
            // them open under the field.
            let start = Start {
                browsers: 3,
                ..stage.start.clone()
            };
            let app = stage.open_with(start).await?;
            app.until_screen("home").await?;
            app.click("account").await?;
            app.click("menu-switch").await?;
            app.click("menu-add").await?;
            app.until_screen("start").await?;
            app.click("start-paste").await?;
            app.until_screen("paste").await?;
            app.click("paste-browser").await?;
        }
        other => bail!("no screen is called {other}"),
    }
    // Whatever moves has come to rest.
    tokio::time::sleep(Duration::from_millis(350)).await;
    Ok(())
}

/// Waits until nothing has come into the run's caches for a while: the
/// pictures a screen asks for are all there before it is pictured. It
/// gives up waiting, without failing, after [`SETTLED_WITHIN`].
async fn pictures_settled(stage: &Stage<'_>) {
    let since = Instant::now();
    let mut kept = cached(&stage.root);
    let mut still = Instant::now();
    while since.elapsed() < SETTLED_WITHIN && still.elapsed() < Duration::from_millis(1200) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = cached(&stage.root);
        if now != kept {
            kept = now;
            still = Instant::now();
        }
    }
}

const SETTLED_WITHIN: Duration = Duration::from_secs(20);

/// How many files every `cache` directory under `root` holds, and their
/// bytes.
fn cached(root: &Path) -> (usize, u64) {
    let mut found = (0, 0);
    let mut folders = vec![(root.to_path_buf(), false)];
    while let Some((folder, inside)) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                let cache = inside || path.file_name().is_some_and(|name| name == "cache");
                folders.push((path, cache));
            } else if inside {
                found.0 += 1;
                found.1 += entry.metadata().map_or(0, |file| file.len());
            }
        }
    }
    found
}

/// A screen not reached, with a picture of where the app was instead and
/// its log.
async fn not_reached(
    stage: &mut Stage<'_>,
    out: &Path,
    suite: &str,
    screen: &str,
    mut outcome: Outcome,
) -> Outcome {
    let failures = out.join("failures");
    if let Ok(app) = stage.app() {
        let picture = failures.join(format!("{suite}-{screen}.png"));
        if app.capture(&picture).await.is_ok() {
            outcome
                .pictures
                .push(("where it was instead".to_string(), picture));
        }
        let log = failures.join(format!("{suite}-{screen}.log"));
        if std::fs::create_dir_all(&failures).is_ok() && std::fs::write(&log, app.log()).is_ok() {
            outcome.log = Some(log);
        }
    }
    outcome
}

/// How the system is set up for one picture of a screen.
#[derive(Clone, Copy)]
struct Looks {
    name: &'static str,
    dark: bool,
    contrast: bool,
    text_scale: f32,
    window: (f32, f32),
}

const USUAL: Looks = Looks {
    name: "light",
    dark: false,
    contrast: false,
    text_scale: 1.0,
    window: (1100.0, 760.0),
};

const LOOKS: [Looks; 6] = [
    USUAL,
    Looks {
        name: "dark",
        dark: true,
        ..USUAL
    },
    Looks {
        name: "light-contrast",
        contrast: true,
        ..USUAL
    },
    Looks {
        name: "dark-contrast",
        dark: true,
        contrast: true,
        ..USUAL
    },
    Looks {
        name: "text-200",
        text_scale: 2.0,
        ..USUAL
    },
    Looks {
        name: "small-window",
        window: (800.0, 600.0),
        ..USUAL
    },
];

async fn look_like(app: &mut Driven, looks: Looks) -> anyhow::Result<Answer> {
    app.say(Said::Dark(looks.dark)).await?;
    app.say(Said::MoreContrast(looks.contrast)).await?;
    app.say(Said::TextScale(looks.text_scale)).await?;
    let (width, height) = looks.window;
    app.resize(width, height).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    app.look().await
}

/// A list turned to a part of it, turned again in each look: its scroll is
/// kept in pixels, which the larger text or the smaller window would leave
/// on another part. Nothing for any other screen.
async fn turned_for(app: &mut Driven, screen: &str, looks: Looks) -> anyhow::Result<()> {
    let by = match screen {
        "settings-storage" => 330.0 * looks.text_scale,
        "settings-accounts" => 100_000.0,
        _ => return Ok(()),
    };
    let seen = app.look().await?;
    let (left, top, width, height) = crate::scenarios::bounds(&seen, "settings-list")?;
    let (x, y) = (left + width / 2.0, top + height / 2.0);
    app.point(x, y, snob_desktop::control::Pointing::Wheel(-100_000.0))
        .await?;
    app.point(x, y, snob_desktop::control::Pointing::Wheel(by))
        .await?;
    // The scrollbar shown by the turn has gone again.
    tokio::time::sleep(Duration::from_millis(2300)).await;
    Ok(())
}

/// Where the approved pictures of this system are kept.
fn approved_dir() -> PathBuf {
    app_dir()
        .join("tests")
        .join("approved")
        .join(std::env::consts::OS)
}

/// A picture of every screen in every look, against the approved ones.
async fn visual(lab: &Lab, asked: &Asked) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    let start = Start {
        instagram: Some(lab.instagram.clone()),
        reduce_motion: true,
        fixed_storage: true,
        fixed_now: Some(scenarios::FIXED_NOW),
        ..Start::default()
    };
    let mut stage = match stage(lab, &asked.out, "visual", start) {
        Ok(stage) => stage,
        Err(e) => {
            return vec![Outcome::of("visual", "setup", Instant::now()).failed(format!("{e:#}"))];
        }
    };
    let shots = asked.out.join("shots");
    for screen in SCREENS {
        let since = Instant::now();
        if let Err(e) = reach(&mut stage, screen).await {
            let failed = Outcome::of("visual", screen, since).failed(format!("{e:#}"));
            outcomes.push(not_reached(&mut stage, &asked.out, "visual", screen, failed).await);
            break;
        }
        for look in LOOKS {
            let since = Instant::now();
            let name = format!("{screen}.{}", look.name);
            let taken = shots.join(format!("{name}.png"));
            let outcome = Outcome::of("visual", &name, since);
            let pictured = async {
                let app = stage.app()?;
                look_like(app, look).await?;
                turned_for(app, screen, look).await?;
                app.capture(&taken).await
            }
            .await;
            outcomes.push(match pictured {
                Ok(()) => report::compared(outcome, &taken, &approved_dir(), &name, asked.approve),
                Err(e) if format!("{e:#}").contains("not implemented") => outcome
                    .noted("this system cannot draw the window to a picture: nothing compared"),
                Err(e) => outcome.failed(format!("{e:#}")),
            });
        }
        if let Ok(app) = stage.app() {
            let _ = look_like(app, USUAL).await;
        }
    }
    let _ = stage.close().await;
    outcomes
}

/// The roles a person acts on.
fn acted_on(role: Option<&str>) -> bool {
    matches!(
        role,
        Some(
            "Button"
                | "MenuItem"
                | "Switch"
                | "TextInput"
                | "PasswordInput"
                | "Link"
                | "ComboBox"
                | "RadioButton"
                | "CheckBox"
                | "Tab"
        )
    )
}

/// What holds the controls of something open over a screen: what Tab walks
/// while it is open, and nothing under it. A list of suggestions or a
/// select's options is no such thing: **the arrows walk them, not Tab**,
/// and Tab leaves the field they hang from, which closes them.
const OVER: [&str; 11] = [
    "account-menu",
    "sign-out",
    "palette",
    "forget",
    "photo-viewer",
    "profile-menu",
    "profile-about",
    "people-menu",
    "list-question",
    "step-dialog",
    "node-menu",
];

/// What is wrong with a screen for somebody who does not see it, does not
/// use a pointer, or reads it at twice the size.
fn audit(seen: &Answer, whole_window: bool) -> Vec<String> {
    let mut wrong = Vec::new();
    let visible: Vec<_> = seen.nodes.iter().filter(|node| node.visible).collect();

    let titles: Vec<_> = visible
        .iter()
        .filter(|node| node.id == "title" && node.role.as_deref() == Some("Heading"))
        .collect();
    match titles.as_slice() {
        [title] => {
            if title.label.as_deref().unwrap_or_default().is_empty() {
                wrong.push("the title has no name".to_string());
            }
        }
        found => wrong.push(format!("{} titles instead of one", found.len())),
    }
    for needed in ["status", "announcements", "brand", "band", "panel"] {
        if !seen.nodes.iter().any(|node| node.id == needed) {
            wrong.push(format!("no {needed}"));
        }
    }

    let controls: Vec<_> = visible
        .iter()
        .filter(|node| acted_on(node.role.as_deref()))
        .collect();
    let mut names: Vec<&str> = Vec::new();
    for control in &controls {
        let name = control.label.as_deref().unwrap_or_default();
        if name.is_empty() {
            wrong.push(format!("{} has no name", control.id));
        } else if names.contains(&name) {
            wrong.push(format!("two controls are called \"{name}\""));
        }
        names.push(name);
        // A text field is clicked on its frame: the role is on the line of
        // text inside it, which is no target of its own.
        let field = matches!(control.role.as_deref(), Some("TextInput" | "PasswordInput"));
        let frame = control
            .inside
            .iter()
            .rev()
            .filter(|_| field)
            .find_map(|above| visible.iter().find(|node| &node.id == above));
        let [left, top, width, height] = frame.map_or(control.bounds, |frame| frame.bounds);
        // WCAG 2.5.8: a target is at least 24 by 24.
        if width < 24.0 || height < 24.0 {
            wrong.push(format!(
                "{} is {width} by {height}: under 24 by 24",
                control.id
            ));
        }
        if whole_window && (left < -0.5 || top < -0.5 || left + width > seen.window[0] + 0.5) {
            wrong.push(format!("{} is cut by the window's edge", control.id));
        }
    }
    for image in visible
        .iter()
        .filter(|node| node.role.as_deref() == Some("Image"))
    {
        if image.label.as_deref().unwrap_or_default().is_empty() {
            wrong.push(format!("the picture {} has no description", image.id));
        }
    }
    wrong
}

/// Every screen audited, at the default size, at twice the text and in the
/// smallest window; and every control reached with Tab alone.
async fn accessibility(lab: &Lab, asked: &Asked) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    let start = Start {
        instagram: Some(lab.instagram.clone()),
        reduce_motion: true,
        ..Start::default()
    };
    let mut stage = match stage(lab, &asked.out, "accessibility", start) {
        Ok(stage) => stage,
        Err(e) => {
            return vec![
                Outcome::of("accessibility", "setup", Instant::now()).failed(format!("{e:#}")),
            ];
        }
    };
    for screen in SCREENS {
        let since = Instant::now();
        if let Err(e) = reach(&mut stage, screen).await {
            let failed = Outcome::of("accessibility", screen, since).failed(format!("{e:#}"));
            outcomes
                .push(not_reached(&mut stage, &asked.out, "accessibility", screen, failed).await);
            break;
        }
        for look in [USUAL, LOOKS[4], LOOKS[5]] {
            let since = Instant::now();
            let outcome = Outcome::of("accessibility", format!("{screen}, {}", look.name), since);
            let audited = async {
                let app = stage.app()?;
                let seen = look_like(app, look).await?;
                let wrong = audit(&seen, true);
                expect(wrong.is_empty(), || wrong.join("; "))
            }
            .await;
            outcomes.push(match audited {
                Ok(()) => outcome.ended(),
                Err(e) => outcome.failed(format!("{e:#}")),
            });
        }
        // The keyboard alone: Tab comes to every control of the screen.
        let since = Instant::now();
        let outcome = Outcome::of("accessibility", format!("{screen}, with Tab alone"), since);
        let walked = async {
            let app = stage.app()?;
            let seen = look_like(app, USUAL).await?;
            // By where each is, as well as its id: two text fields have the
            // toolkit's id for their line of text.
            let mut wanted: Vec<Vec<String>> = seen
                .nodes
                .iter()
                .filter(|node| {
                    node.visible
                        && acted_on(node.role.as_deref())
                        && node.disabled != Some(true)
                        && !node.id.starts_with("window-")
                })
                .map(|node| [node.inside.clone(), vec![node.id.clone()]].concat())
                .collect();
            // What is under an open menu or question is out of reach on
            // purpose.
            let over = seen.nodes.iter().any(|node| {
                node.visible
                    && matches!(
                        node.role.as_deref(),
                        Some("Menu" | "AlertDialog" | "Dialog")
                    )
            });
            if over {
                wanted.retain(|path| path.iter().any(|above| OVER.contains(&above.as_str())));
            }
            // Twice around everything Tab can stop at, a list's rows
            // included.
            let stops = seen
                .nodes
                .iter()
                .filter(|node| {
                    node.visible
                        && (acted_on(node.role.as_deref())
                            || node.role.as_deref() == Some("ListBoxOption"))
                })
                .count();
            let presses = stops * 2 + 4;
            for _ in 0..presses {
                let seen = app.press("tab").await?;
                wanted.retain(|path| {
                    !seen.nodes.iter().any(|node| {
                        node.focused == Some(true)
                            && node.id == path[path.len() - 1]
                            && node.inside == path[..path.len() - 1]
                    })
                });
                if wanted.is_empty() {
                    break;
                }
            }
            expect(wanted.is_empty(), || {
                let ids: Vec<&String> = wanted.iter().filter_map(|path| path.last()).collect();
                format!("Tab never comes to {ids:?}")
            })
        }
        .await;
        outcomes.push(match walked {
            Ok(()) => outcome.ended(),
            Err(e) => outcome.failed(format!("{e:#}")),
        });
    }
    let _ = stage.close().await;
    outcomes
}

/// A release build has no test mode in it: not in what it depends on, and
/// not in its binary.
fn no_test_mode_in_a_release() -> Outcome {
    let since = Instant::now();
    let outcome = Outcome::of("privacy", "a release build has no test mode", since);
    let cargo = |args: &[&str]| {
        Command::new(env!("CARGO"))
            .args(args)
            .current_dir(app_dir())
            .output()
    };
    // What the default build links, features included.
    let tree = match cargo(&["tree", "--edges", "normal,features", "--prefix", "none"]) {
        Ok(tree) if tree.status.success() => String::from_utf8_lossy(&tree.stdout).into_owned(),
        Ok(tree) => return outcome.failed(String::from_utf8_lossy(&tree.stderr).into_owned()),
        Err(e) => return outcome.failed(format!("cargo did not run: {e}")),
    };
    for seam in [
        "snob-fake-ig",
        "feature \"testing\"",
        "feature \"test-support\"",
    ] {
        if tree.contains(seam) {
            return outcome.failed(format!("the default build links {seam}"));
        }
    }
    // And what is in the binary of a build without the feature.
    let plain = crate::target_dir().join("plain");
    let built = cargo(&[
        "build",
        "--bin",
        "snob-desktop",
        "--target-dir",
        &plain.display().to_string(),
    ]);
    match built {
        Ok(built) if built.status.success() => {}
        Ok(built) => {
            return outcome.failed(String::from_utf8_lossy(&built.stderr).into_owned());
        }
        Err(e) => return outcome.failed(format!("cargo did not run: {e}")),
    }
    let exe = plain
        .join("debug")
        .join(format!("snob-desktop{}", std::env::consts::EXE_SUFFIX));
    let bytes = match std::fs::read(&exe) {
        Ok(bytes) => bytes,
        Err(e) => return outcome.failed(format!("{}: {e}", exe.display())),
    };
    for seam in [
        "--test-root",
        "--simulate-system",
        "--instagram",
        "--browsers",
        "--fixed-storage",
        "--fixed-now",
        "desktop-control",
    ] {
        if bytes
            .windows(seam.len())
            .any(|window| window == seam.as_bytes())
        {
            return outcome.failed(format!("the binary without the feature holds \"{seam}\""));
        }
    }
    outcome.noted("not among its dependencies, and none of its words in the binary")
}

/// Runs what was asked, prints what was found, writes the report, and
/// answers whether everything passed.
pub async fn run(asked: Asked) -> anyhow::Result<bool> {
    std::fs::create_dir_all(&asked.out)?;
    let exe = test_build()?;
    let world = if asked.demo {
        World::for_the_website()
    } else {
        World::new()
    };
    let server = world.serve_rest().await;
    scenarios::Pictures::serve(&server).await;
    let lab = Lab {
        exe,
        world,
        instagram: server.uri(),
    };

    if asked.demo {
        return demo(&lab, &asked).await.map(|()| true);
    }

    let matches = |scenario: &Scenario| {
        asked.filters.is_empty()
            || asked
                .filters
                .iter()
                .any(|filter| scenario.name.to_lowercase().contains(filter))
    };
    let mut outcomes = Vec::new();
    for suite in &asked.suites {
        println!("\n== {suite} ==");
        let found = match *suite {
            "logic" => logic(),
            "behavior" => {
                let mut found = Vec::new();
                for scenario in scenarios::all().iter().filter(|s| matches(s)) {
                    let outcome = run_scenario("behavior", &lab, &asked.out, scenario).await;
                    report::line(&outcome);
                    found.push(outcome);
                }
                found
            }
            "visual" => visual(&lab, &asked).await,
            "accessibility" => accessibility(&lab, &asked).await,
            "privacy" => {
                let mut found = Vec::new();
                for scenario in scenarios::all()
                    .iter()
                    .filter(|scenario| scenario.name.contains("secret"))
                {
                    found.push(run_scenario("privacy", &lab, &asked.out, scenario).await);
                }
                found.push(no_test_mode_in_a_release());
                found
            }
            "performance" => crate::report::measured(&lab, &asked).await,
            "endurance" => endurance(&lab, &asked).await,
            _ => Vec::new(),
        };
        if *suite != "behavior" {
            found.iter().for_each(report::line);
        }
        outcomes.extend(found);
    }

    let page = report::write(&asked.out, &outcomes)?;
    let failed = outcomes.iter().filter(|outcome| !outcome.passed).count();
    println!(
        "\n{} checked, {} failed. The report: {}",
        outcomes.len(),
        failed,
        page.display()
    );
    Ok(failed == 0)
}

/// The test build opened by hand on the world of readable names, until it
/// is closed.
async fn demo(lab: &Lab, asked: &Asked) -> anyhow::Result<()> {
    let root = asked.out.join("demo");
    std::fs::create_dir_all(&root)?;
    println!(
        "The test build, on the fake Instagram at {}.\nTo sign in, paste this as the session ID:\n\n  {}\n\nClose the window to end.",
        lab.instagram,
        scenarios::session_id(world::ME, "demo")
    );
    let mut child = std::process::Command::new(&lab.exe)
        .args(["--test-root", &root.display().to_string()])
        .args(["--instagram", &lab.instagram])
        .spawn()?;
    while child.try_wait()?.is_none() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(())
}

/// Many sleeps and many changes of account in one run of the app, which
/// must still answer and must not have grown.
async fn endurance(lab: &Lab, asked: &Asked) -> Vec<Outcome> {
    const SLEEPS: usize = 200;
    const SWITCHES: usize = 60;
    let since = Instant::now();
    let start = Start {
        instagram: Some(lab.instagram.clone()),
        reduce_motion: true,
        ..Start::default()
    };
    let outcome = Outcome::of(
        "endurance",
        format!("{SLEEPS} sleeps and {SWITCHES} changes of account"),
        since,
    );
    let ran = async {
        let mut stage = stage(lab, &asked.out, "endurance", start)?;
        stage.open().await?;
        stage.sign_in(world::ME, "endures").await?;
        let app = stage.app()?;
        app.click("account").await?;
        app.click("menu-add").await?;
        stage.sign_in(world::SOMEONE, "endures").await?;
        let app = stage.app()?;
        let before = report::memory_of(app);
        for _ in 0..SLEEPS {
            app.say(Said::Sleeping).await?;
            app.say(Said::Woke(3600)).await?;
        }
        for _ in 0..SWITCHES {
            app.click("account").await?;
            app.click("menu-switch").await?;
            app.click("menu-account#0").await?;
            app.until("the menu closed", |seen| {
                node(seen, "account-menu").is_err()
            })
            .await?;
        }
        let seen = app.look().await?;
        expect(seen.screen == "home", || {
            format!("ended on {}", seen.screen)
        })?;
        let after = report::memory_of(app);
        stage.close().await?;
        anyhow::Ok((before, after))
    }
    .await;
    vec![match ran {
        Ok((Some(before), Some(after))) => {
            let grew = after.saturating_sub(before) / (1024 * 1024);
            let said = format!(
                "memory went from {} MiB to {} MiB",
                before / (1024 * 1024),
                after / (1024 * 1024)
            );
            // A leak per cycle shows as hundreds of MiB here.
            if grew > 150 {
                outcome.failed(said)
            } else {
                outcome.noted(said)
            }
        }
        Ok(_) => outcome.noted("memory could not be read on this system"),
        Err(e) => outcome.failed(format!("{e:#}")),
    }]
}
