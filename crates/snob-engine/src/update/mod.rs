//! Keeping snob up to date: finding how it was installed ([`how`]), asking
//! GitHub once a day whether a newer release is out ([`github`]), fetching it
//! ahead of time, and applying it when snob next starts with no other snob
//! running ([`running`]), or when the person runs `snob update`.
//!
//! **Nothing here talks to Instagram or reads an account.**
//!
//! **Every command a person or a program runs takes part** ([`at_start`]): it
//! applies a version waiting to be applied when it is alone, says once what was
//! applied or is waiting, and starts the day's check, detached (`detach`), when
//! one is due, so no command waits on GitHub.
//!
//! **Never here**: a developer's build (`how::a_build`), a sandbox, a build
//! whose own version is a pre-release, a machine snob has no release for, `CI`
//! set, or `SNOB_NO_UPDATE_CHECK` set ([`enabled`]).
//!
//! **One integrity check: the release's `SHA256SUMS`**, the same one the
//! install scripts make, over HTTPS to GitHub's hosts only.
//!
//! **An install is updated as the edition it is** ([`edition`]): the app
//! beside `snob` comes from the same archive and is replaced with it.

pub mod apply;
pub mod edition;
pub mod github;
pub mod how;
pub(crate) mod running;
pub mod state;
pub mod version;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use snob_store::paths::AppPaths;

use self::edition::Edition;
use self::how::{Applies, Method};
use self::state::{Auto, Files, State};
use self::version::Version;

/// The repository releases come from.
pub(crate) const REPO: &str = "dennisgr7/snob";

/// The hidden command: `snob __update fetch|apply`.
pub const COMMAND: &str = "__update";

/// How long after a check the next one is due.
const CHECK_EVERY_SECS: i64 = 24 * 60 * 60;

/// The release target this build is, whose archive the command line updates
/// from; the desktop edition's is `edition::APP_TARGET`.
pub(crate) const TARGET: Option<&str> = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("x86_64-unknown-linux-musl")
} else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
    Some("aarch64-unknown-linux-musl")
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    Some("aarch64-apple-darwin")
} else if cfg!(all(windows, target_arch = "x86_64")) {
    Some("x86_64-pc-windows-msvc")
} else if cfg!(all(windows, target_arch = "aarch64")) {
    Some("aarch64-pc-windows-msvc")
} else {
    None
};

/// Whether this process may check for and apply updates at all.
pub fn enabled(sandboxed: bool) -> bool {
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if set("SNOB_NO_UPDATE_CHECK") || set("CI") {
        return false;
    }
    if Version::current().is_none() || TARGET.is_none() {
        return false;
    }
    if test_origin().is_some() {
        return true;
    }
    !sandboxed && !how::this_is_a_build()
}

/// Whether `snob update` may run here, which a person asking for it settles
/// for everything [`enabled`] leaves to the automatic update (`CI`,
/// `SNOB_NO_UPDATE_CHECK`) but not for a build snob cannot replace.
pub fn may_update_by_hand(sandboxed: bool) -> anyhow::Result<()> {
    use crate::exit::{ExitCode, ExitError};
    let refused = |why: &str| -> anyhow::Result<()> {
        Err(ExitError::new(ExitCode::Error, why.to_string()).into())
    };
    if Version::current().is_none() {
        return refused("this snob is a pre-release build, which never updates");
    }
    if TARGET.is_none() {
        return refused("snob has no release for this machine; build it from source instead");
    }
    if test_origin().is_some() {
        return Ok(());
    }
    if sandboxed {
        return refused("a sandbox never updates");
    }
    if how::this_is_a_build() {
        return Err(ExitError::new(
            ExitCode::Error,
            "this snob is a build from source, which never replaces itself",
        )
        .with_hint("update it with git pull and cargo build")
        .into());
    }
    Ok(())
}

/// A fake GitHub a sandboxed test points the updater at, and the one way a
/// build in `target` or a sandbox updates.
#[cfg(feature = "testing")]
fn test_origin() -> Option<github::Origin> {
    let base = std::env::var("SNOB_TEST_UPDATE_ORIGIN").ok()?;
    url::Url::parse(&base)
        .ok()
        .map(|base| github::Origin::at(&base))
}

#[cfg(not(feature = "testing"))]
fn test_origin() -> Option<github::Origin> {
    None
}

pub fn origin() -> github::Origin {
    test_origin().unwrap_or_else(github::Origin::github)
}

/// The binary that is running, every link followed: the file a script
/// install replaces.
pub fn this_exe() -> std::io::Result<PathBuf> {
    how::real_exe()
}

/// How a command takes part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A command: applies, tells, checks, and counts as running.
    Command,
    /// Counts as running and nothing more: a job's supervisor, and the app
    /// for as long as it is open.
    Counted,
}

/// What a command holds while it runs: its place in `running.lock`.
pub struct Taking(#[allow(dead_code)] Option<std::fs::File>);

/// What `main` does at the start of every command that takes part (all but
/// `snob mcp`, the owner of the browsers, the plumbing of a job, `snob update`
/// and this module's own processes).
pub fn at_start(paths: &AppPaths, sandboxed: bool, role: Role, globals: &[OsString]) -> Taking {
    if !enabled(sandboxed) {
        return Taking(None);
    }
    let files = Files::of(paths);
    if snob_store::paths::create_private_dir(paths.data_dir()).is_err() {
        return Taking(None);
    }
    if role == Role::Counted {
        return Taking(running::count_me(&files.running_lock()));
    }
    if let Ok(exe) = this_exe() {
        apply::sweep_old(&exe);
        apply::sweep_old(&edition::app_beside(&exe));
    }
    let state = state::load(&files);
    if state.auto == Auto::Apply
        && state.staged.is_some()
        && let Some(alone) = running::alone(&files.running_lock())
    {
        apply_when_alone(&files, &state, globals);
        drop(alone);
    }
    let taking = Taking(running::count_me(&files.running_lock()));
    tell(&files);
    let state = state::load(&files);
    if state.auto != Auto::Off && due(&state) {
        start_detached(&files, globals, &["fetch".into()]);
    }
    taking
}

/// Whether the day's check is due.
fn due(state: &State) -> bool {
    state
        .last_check
        .is_none_or(|at| state::now().saturating_sub(at) >= CHECK_EVERY_SECS || at > state::now())
}

/// Starts `snob __update <what...>` detached, its errors appended to the
/// log.
fn start_detached(files: &Files, globals: &[OsString], what: &[OsString]) {
    let applying = what.first().is_some_and(|first| first == "apply");
    let started = (|| -> anyhow::Result<()> {
        std::fs::create_dir_all(files.dir())?;
        let program = applier(files, applying)?;
        let mut args = globals.to_vec();
        args.push(COMMAND.into());
        args.extend(what.iter().cloned());
        crate::system::detach::spawn(&program, &args, None, &files.log())?;
        Ok(())
    })();
    if let Err(e) = started {
        tracing::debug!(error = %e, ?what, "the updater could not be started");
    }
}

/// The program `snob __update <what>` runs from: this binary, but for a
/// package manager's upgrade on Windows, which refuses while the
/// `snob.exe` it would replace runs, and so runs from a copy in the data
/// directory.
fn applier(files: &Files, applying: bool) -> anyhow::Result<PathBuf> {
    let exe = crate::system::exe::program()?;
    if !(cfg!(windows) && applying) {
        return Ok(exe);
    }
    let copy = files.dir().join(format!("applier-{}", apply::EXE));
    std::fs::copy(&exe, &copy)?;
    Ok(copy)
}

/// Applies the version waiting, this process being the only snob running.
fn apply_when_alone(files: &Files, state: &State, globals: &[OsString]) {
    let Some(staged) = &state.staged else {
        return;
    };
    let method = how::this_binary();
    match method.applies() {
        Applies::Itself => {
            let Ok(exe) = this_exe() else {
                return;
            };
            match put_in_place(&exe, staged) {
                Ok(()) => {
                    note_applied(files, &staged.version);
                    #[cfg(unix)]
                    {
                        let failed = apply::run_again(&exe);
                        note_failure(files, &format!("snob could not start again: {failed}"));
                    }
                }
                Err(e) => {
                    let _ = state::change(files, |s| s.staged = None);
                    note_failure(files, &format!("{e:#}"));
                }
            }
        }
        // The binary it is for is named, since on Windows the upgrade runs
        // from a copy whose own path says nothing of how snob was installed.
        Applies::Manager => {
            if let Ok(exe) = this_exe() {
                start_detached(
                    files,
                    globals,
                    &["apply".into(), "--for".into(), exe.into()],
                );
            }
        }
        Applies::ByHand | Applies::Never => {}
    }
}

/// Checks the staged binary is the one downloaded, and puts it at `exe`,
/// with the app beside it when the install is the desktop edition's.
pub fn put_in_place(exe: &Path, staged: &state::Staged) -> anyhow::Result<()> {
    let (Some(file), Some(sha256)) = (&staged.file, &staged.sha256) else {
        anyhow::bail!("the downloaded version has no file to apply");
    };
    let unchanged = |file: &Path, sha256: &str| -> anyhow::Result<()> {
        anyhow::ensure!(
            github::sha256_of(file)? == sha256,
            "the downloaded binary changed since it was checked; it is downloaded again"
        );
        Ok(())
    };
    unchanged(file, sha256)?;
    let mut swaps = Vec::new();
    match (Edition::of(exe), &staged.app) {
        (Edition::Desktop, Some(app)) => {
            unchanged(&app.file, &app.sha256)?;
            swaps.push(apply::Swap {
                exe: edition::app_beside(exe),
                new: app.file.clone(),
            });
        }
        (Edition::Desktop, None) => anyhow::bail!(
            "the downloaded version is the command line alone, and the app is now beside \
             it; it is downloaded again"
        ),
        // An app taken away since: the command line is updated alone.
        (Edition::Cli, _) => {}
    }
    // The command line last: a swap cut short between the two leaves it
    // the older version, which the next check sees and puts right.
    swaps.push(apply::Swap {
        exe: exe.to_path_buf(),
        new: file.clone(),
    });
    apply::replace_all(&swaps)?;
    Ok(())
}

pub fn note_applied(files: &Files, to: &str) {
    let from = env!("CARGO_PKG_VERSION").to_string();
    let to = to.to_string();
    let _ = state::change(files, |s| {
        s.staged = None;
        s.error = None;
        s.applied = Some(state::Applied {
            from,
            to,
            at: state::now(),
            told: false,
        });
    });
    let _ = std::fs::remove_dir_all(files.staged_dir());
}

pub fn note_failure(files: &Files, message: &str) {
    let message = message.to_string();
    let _ = state::change(files, |s| {
        s.error = Some(state::Failure {
            at: state::now(),
            message,
        });
    });
}

/// Says, once each and on a terminal only, what was applied and what is
/// waiting. Standard error, never standard output, which carries data.
fn tell(files: &Files) {
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() {
        return;
    }
    let state = state::load(files);
    let current = Version::current();
    let mut said = Vec::new();
    let mut applied_told = false;
    if let Some(applied) = &state.applied
        && !applied.told
        && Version::parse(&applied.to) == current
    {
        said.push(format!(
            "snob was updated from {} to {}.",
            applied.from, applied.to
        ));
        applied_told = true;
    }
    let waiting = state
        .latest
        .as_deref()
        .and_then(Version::parse)
        .filter(|latest| Some(*latest) > current);
    let mut told = None;
    if let Some(latest) = waiting
        && state.told.as_deref() != Some(&latest.to_string())
    {
        let method = how::this_binary();
        let staged = state
            .staged
            .as_ref()
            .is_some_and(|s| s.version == latest.to_string());
        let line = match (method.applies(), staged, state.auto) {
            (Applies::Itself | Applies::Manager, true, Auto::Apply) => Some(format!(
                "snob {latest} is downloaded, and is applied the next time snob starts \
                 with no other snob running (or after a restart); \"snob update\" \
                 applies it now."
            )),
            (Applies::Itself | Applies::Manager, true, _) => Some(format!(
                "snob {latest} is downloaded; \"snob update\" applies it."
            )),
            (Applies::Never, ..) => None,
            _ => method
                .by_hand(state.staged.as_ref().and_then(|s| s.file.as_deref()))
                .map(|command| format!("snob {latest} is out. Update with: {command}")),
        };
        if let Some(line) = line {
            said.push(line);
            told = Some(latest.to_string());
        }
    }
    if said.is_empty() {
        return;
    }
    for line in &said {
        crate::host::info(line);
    }
    let _ = state::change(files, |s| {
        if applied_told && let Some(applied) = &mut s.applied {
            applied.told = true;
        }
        if told.is_some() {
            s.told = told;
        }
    });
}

/// `snob __update fetch`: the day's check, and the download when a newer
/// version is out and the setting allows it.
pub async fn fetch(paths: &AppPaths) -> anyhow::Result<()> {
    let files = Files::of(paths);
    std::fs::create_dir_all(files.dir())?;
    let Some(_alone) = snob_store::paths::try_lock_file(&files.fetch_lock())? else {
        return Ok(());
    };
    let state = state::load(&files);
    if state.auto == Auto::Off {
        return Ok(());
    }
    let checked = check(&files, &origin()).await;
    match checked {
        Ok(Some(latest)) if state.auto != Auto::Notify => {
            if let Err(e) = stage(&files, &origin(), latest, &how::this_binary()).await {
                note_failure(&files, &format!("{e:#}"));
            }
        }
        Ok(_) => {}
        Err(e) => note_failure(&files, &format!("{e:#}")),
    }
    Ok(())
}

/// Asks GitHub, writes down what it said, and returns the newest version
/// when it is newer than this one.
pub async fn check(files: &Files, origin: &github::Origin) -> anyhow::Result<Option<Version>> {
    let (etag, latest) = state::change(files, |s| {
        s.last_check = Some(state::now());
        (s.etag.clone(), s.latest.clone())
    })?
    .0;
    let named = match github::latest(origin, etag.as_deref()).await? {
        github::Latest::Unchanged => latest.as_deref().and_then(Version::parse),
        github::Latest::Release { version, etag } => {
            state::change(files, |s| {
                // Never moved back: an older release named later is no
                // version to move to.
                let seen = s.latest.as_deref().and_then(Version::parse);
                if seen.is_none_or(|seen| version > seen) {
                    s.latest = Some(version.to_string());
                }
                s.etag = etag;
                s.error = None;
            })?;
            let seen = state::load(files).latest;
            seen.as_deref().and_then(Version::parse)
        }
    };
    Ok(named.filter(|named| Some(*named) > Version::current()))
}

/// Fetches `version` ahead of applying it, as `method` allows, and writes it
/// down as staged.
pub async fn stage(
    files: &Files,
    origin: &github::Origin,
    version: Version,
    method: &Method,
) -> anyhow::Result<()> {
    stage_telling(files, origin, version, method, &|_| {}).await
}

/// [`stage`], telling `told` how far the download has come, where there is
/// one: a package manager's own fetch says nothing.
pub async fn stage_telling(
    files: &Files,
    origin: &github::Origin,
    version: Version,
    method: &Method,
    told: github::Told<'_>,
) -> anyhow::Result<()> {
    let state = state::load(files);
    if state
        .staged
        .as_ref()
        .is_some_and(|s| s.version == version.to_string())
    {
        return Ok(());
    }
    let target = TARGET.ok_or_else(|| anyhow::anyhow!("snob has no release for this machine"))?;
    // Earlier downloads go: only one version is ever staged.
    let dir = files.staged_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mut app = None;
    let (file, sha256) = match method.applies() {
        Applies::Itself => {
            let installed = Edition::of(&this_exe()?);
            let (top, name) = installed.archive(version)?;
            let sums = github::sums(origin, version).await?;
            let expected = github::listed(&sums, &name)
                .ok_or_else(|| anyhow::anyhow!("{name} is not listed in SHA256SUMS"))?;
            let archive = dir.join(&name);
            github::download(origin, version, &name, &expected, &archive, told).await?;
            let held = apply::unpack(&archive, &dir.join("x"), &top)?;
            // Out of the unpacked directory under the name it is kept as,
            // with its SHA-256 as it now is.
            let keep = |name: &str| -> anyhow::Result<(PathBuf, String)> {
                let staged = dir.join(format!("{name}.new"));
                std::fs::rename(apply::file_in(&held, name)?, &staged)?;
                let sha256 = github::sha256_of(&staged)?;
                Ok((staged, sha256))
            };
            let (staged, sha256) = keep(apply::EXE)?;
            if installed == Edition::Desktop {
                let (file, sha256) = keep(edition::APP)?;
                app = Some(state::StagedApp { file, sha256 });
            }
            let _ = std::fs::remove_dir_all(dir.join("x"));
            let _ = std::fs::remove_file(&archive);
            (Some(staged), Some(sha256))
        }
        Applies::ByHand if *method == Method::Deb => {
            let name = format!("snob-v{version}-{target}.deb");
            let sums = github::sums(origin, version).await?;
            let expected = github::listed(&sums, &name)
                .ok_or_else(|| anyhow::anyhow!("{name} is not listed in SHA256SUMS"))?;
            let deb = dir.join(&name);
            github::download(origin, version, &name, &expected, &deb, told).await?;
            (Some(deb), Some(expected))
        }
        Applies::Manager => {
            let steps = method.prefetch();
            let log = files.log();
            tokio::task::spawn_blocking(move || {
                apply::run_steps(&steps, &apply::Output::Log(&log))
            })
            .await??;
            (None, None)
        }
        Applies::ByHand | Applies::Never => return Ok(()),
    };
    state::change(files, |s| {
        s.staged = Some(state::Staged {
            version: version.to_string(),
            file,
            sha256,
            app,
            at: state::now(),
        });
        s.error = None;
    })?;
    Ok(())
}

/// `snob __update apply`: a package manager's upgrade to the staged version,
/// once no snob runs.
pub fn apply_detached(paths: &AppPaths, exe: &Path) -> anyhow::Result<()> {
    let files = Files::of(paths);
    let Some(_once) = snob_store::paths::try_lock_file(&files.dir().join("apply.lock"))? else {
        return Ok(());
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30 * 60);
    let alone = loop {
        if let Some(alone) = running::alone(&files.running_lock()) {
            break alone;
        }
        if std::time::Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    };
    let state = state::load(&files);
    let Some(staged) = state.staged else {
        return Ok(());
    };
    let method = how::detect(&std::fs::canonicalize(exe)?, &how::System);
    let steps = method.apply(&staged.version);
    let applied = apply::run_steps(&steps, &apply::Output::Log(&files.log()));
    drop(alone);
    match applied {
        Ok(()) => note_applied(&files, &staged.version),
        Err(e) => {
            let _ = state::change(&files, |s| s.staged = None);
            note_failure(&files, &format!("{e:#} (see {})", files.log().display()));
        }
    }
    Ok(())
}

/// The state, for `snob status --json`, `snob update --status --json` and
/// anything that shows it: stable tokens and numbers, moments in seconds with
/// an ISO twin, as the monitor's reports give them.
pub fn report(paths: &AppPaths, sandboxed: bool) -> Value {
    report_as(paths, sandboxed, Version::current())
}

/// [`report`], `current` being the version installed: this process's, or the
/// one `snob update` has just put in its place.
pub fn report_as(paths: &AppPaths, sandboxed: bool, current: Option<Version>) -> Value {
    let files = Files::of(paths);
    let state = state::load(&files);
    let method = how::this_binary();
    let latest = state.latest.as_deref().and_then(Version::parse);
    let available = latest.filter(|latest| Some(*latest) > current);
    let deb = state.staged.as_ref().and_then(|s| s.file.as_deref());
    let at = |seconds: i64| {
        json!({
            "at": seconds,
            "at_iso": chrono::DateTime::from_timestamp(seconds, 0)
                .map(|moment| moment.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        })
    };
    let staged = state.staged.as_ref().map(|s| {
        let mut staged = at(s.at);
        staged["version"] = json!(s.version);
        staged
    });
    let applies = match (method.applies(), state.auto) {
        (_, _) if !enabled(sandboxed) => "never",
        (Applies::Itself | Applies::Manager, Auto::Apply) => "next_cold_start",
        (Applies::Itself | Applies::Manager, _) => "update_command",
        (Applies::ByHand, _) => "by_hand",
        (Applies::Never, _) => "never",
    };
    let installed = this_exe().map_or(Edition::Cli, |exe| Edition::of(&exe));
    json!({
        "current": current.map(|v| v.to_string()),
        "method": method.as_str(),
        "edition": installed.as_str(),
        "enabled": enabled(sandboxed),
        "auto": state.auto.as_str(),
        "last_check": state.last_check.map(&at),
        "latest": state.latest,
        "available": available.map(|v| v.to_string()),
        "staged": staged,
        "applies": applies,
        "by_hand": available.and(method.by_hand(deb)),
        "applied": state.applied.as_ref().map(|a| {
            let mut applied = at(a.at);
            applied["from"] = json!(a.from);
            applied["to"] = json!(a.to);
            applied
        }),
        "error": state.error.as_ref().map(|e| {
            let mut error = at(e.at);
            error["message"] = json!(e.message);
            error
        }),
    })
}
