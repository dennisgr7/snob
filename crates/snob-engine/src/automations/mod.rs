//! Automations, as every front reads, keeps and runs them: the command line's
//! `snob watch` and the desktop app's automations are one thing.
//!
//! - **What one is** is `snob_core::automation`; `watch.toml` is one more,
//!   with the id `watch` (`snob_store::automations::from_watch`), kept in
//!   that file and written back to it ([`save`]).
//! - **What one reads** is [`read`]: the monitor's tick for lists and posts
//!   (`watch`, through [`watched`]), and [`read::read`] for the
//!   rest, every list through `engine::list` with its consent, pace, budget
//!   and cooldown. **Nothing here writes to Instagram.**
//! - **When one runs**: on its schedules, through `snob watch` (the scheduled
//!   mode, or a timer running `snob watch once --when-due`), or at once
//!   ([`run_now`], which starts `snob watch once --automation <id>`), the
//!   only way one with a manual trigger alone runs ([`Next::ByHand`]). A run
//!   counts from its start, written down before its work
//!   (`Shared::record_automation_run`).

pub mod read;

use std::ffi::OsString;

use anyhow::{Context, Result};
use snob_core::automation::{Automation, Planned, Problem, Read, WATCH_TOML};
use snob_core::watch::schedule::{self, Due};
use snob_core::{Epoch, Pk};
use snob_store::automations as files;
use snob_store::config;
use snob_store::paths::AppPaths;
use snob_store::registry::Registry;
use snob_store::secrets::{Kind, SecretStore};
use snob_store::store::shared::{AutomationRun, Shared};

use crate::watch::{Consent, PostsWatch, Watched};

pub use snob_core::automation;

/// What [`all`] found, with what could not be read said apart.
#[derive(Debug, Default)]
pub struct Kept {
    /// `watch.toml`'s first, when there is one, then the files by id.
    pub automations: Vec<Automation>,
    /// A sentence for each file that could not be read.
    pub unreadable: Vec<String>,
}

/// Every automation: `watch.toml`'s and the ones kept as files.
pub fn all(paths: &AppPaths) -> Kept {
    let mut kept = Kept::default();
    match config::load(paths) {
        Ok(Some(config)) => kept.automations.push(files::from_watch(&config)),
        Ok(None) => {}
        Err(e) => kept.unreadable.push(e.to_string()),
    }
    let (found, failed) = files::list(paths);
    kept.automations.extend(found);
    kept.unreadable
        .extend(failed.into_iter().map(|e| e.to_string()));
    kept
}

/// The automation `id`, if there is one.
pub fn find(paths: &AppPaths, id: &str) -> Result<Option<Automation>> {
    if id == WATCH_TOML {
        return Ok(config::load(paths)?.map(|config| files::from_watch(&config)));
    }
    Ok(files::load(paths, id)?)
}

/// What keeps `automation` from running among the others: its own problems,
/// then each list or grid another automation already reads.
pub fn problems(paths: &AppPaths, automation: &Automation) -> Vec<Problem> {
    let mut problems = automation.problems();
    if automation.id == WATCH_TOML
        && let Err(more) = files::to_watch(automation)
    {
        problems.extend(more);
    }
    for other in all(paths)
        .automations
        .iter()
        .filter(|other| other.id != automation.id)
    {
        problems.extend(automation.clashes_with(other));
    }
    problems
}

/// Keeps `automation`, finished or not: `watch.toml`'s back in that file,
/// **only when the file can hold it** and it is finished, any other in its
/// own file. `secrets` says whether the file is to say its webhook's body is
/// signed (`config::template`).
pub fn save(paths: &AppPaths, secrets: &SecretStore, automation: &Automation) -> Result<()> {
    if automation.id != WATCH_TOML {
        files::save(paths, automation)?;
        return Ok(());
    }
    let config = files::to_watch(automation).map_err(|problems| {
        anyhow::anyhow!(
            "watch.toml cannot hold this: {}",
            problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    let signed = secrets
        .load_secret(Kind::WatchSigningKey)
        .is_ok_and(|stored| stored.found().is_some());
    let registry = Registry::load(paths).unwrap_or_default();
    let text = config::template(&config, signed, |pk| {
        crate::account::username(&registry, pk).map(str::to_string)
    });
    config::write(paths, &text)?;
    Ok(())
}

/// Removes the automation `id` and its history. `watch.toml` is never
/// removed here: it is the person's file.
pub fn remove(paths: &AppPaths, id: &str) -> Result<bool> {
    if id == WATCH_TOML {
        anyhow::bail!("watch.toml is removed by hand, not from here");
    }
    let removed = files::remove(paths, id)?;
    if let Ok(shared) = Shared::open(paths) {
        shared.forget_automation_runs(id)?;
    }
    Ok(removed)
}

/// An id no automation has: eight hexadecimal digits.
pub fn new_id(paths: &AppPaths) -> String {
    loop {
        let id = format!("{:08x}", fastrand::u32(..));
        if !snob_store::automations::possible_id(&id) {
            continue;
        }
        if files::load(paths, &id).ok().flatten().is_none() {
            return id;
        }
    }
}

/// The newest `limit` runs of the automation `id`, the newest first.
pub fn runs(paths: &AppPaths, id: &str, limit: usize) -> Result<Vec<AutomationRun>> {
    match Shared::read_existing(paths)? {
        Some(shared) => Ok(shared.automation_runs(id, limit)?),
        None => Ok(Vec::new()),
    }
}

/// When a finished automation runs next: due now, at a moment, only when
/// somebody runs it (a manual trigger and no schedule), or never: not
/// finished, or no schedule of it comes round again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Now,
    At(Epoch),
    ByHand,
    Never,
}

/// When `automation` is next due at `now`, from its last start and the
/// moment a timer first asked (`Shared::ask_since`): due when any one of its
/// schedules is. The moment it was due for comes with `Now`.
pub fn next_run(
    paths: &AppPaths,
    automation: &Automation,
    now: Epoch,
) -> Result<(Next, Option<Epoch>)> {
    let Ok(program) = automation.program() else {
        return Ok((Next::Never, None));
    };
    if program.schedules.is_empty() {
        // Finished with no schedule: every trigger it has is manual.
        return Ok((Next::ByHand, None));
    }
    let shared = Shared::open(paths)?;
    let last_run = shared.automation_last_started(&automation.id)?;
    let asked_since = shared.asked_since()?.unwrap_or(now);
    let mut soonest: Option<Epoch> = None;
    for spec in &program.schedules {
        let Ok(schedule) = spec.build() else {
            continue;
        };
        match schedule::due_asked_since(&schedule, last_run, asked_since, now, &chrono::Local) {
            Due::Now { .. } => {
                let due_at = schedule::due_moment(
                    &schedule,
                    last_run,
                    Some(asked_since),
                    now,
                    &chrono::Local,
                );
                return Ok((Next::Now, due_at));
            }
            Due::At(at) => soonest = Some(soonest.map_or(at, |soonest| soonest.min(at))),
            Due::Never => {}
        }
    }
    Ok((soonest.map_or(Next::Never, Next::At), None))
}

/// The monitor's entries for the lists and posts `reads` asks for, one per
/// account and viewer as `watch.toml` has them, the account in use standing
/// for a viewer not named: what a tick reads ([`crate::watch::tick`]).
pub fn watched(reads: &[Planned], in_use: Option<Pk>) -> Vec<Watched> {
    let mut entries: Vec<(Option<Pk>, Option<String>, Watched)> = Vec::new();
    for read in reads {
        let viewer = read.account.viewer.or(in_use);
        let name = read
            .account
            .name()
            .map(|name| crate::engine::target::clean(name).to_string());
        let (lists, posts) = match &read.what {
            Read::Lists => (true, None),
            Read::Posts { save_to, quality } => (
                false,
                Some(PostsWatch {
                    save_to: save_to.clone(),
                    quality: *quality,
                }),
            ),
            _ => continue,
        };
        let same = |(v, n, _): &&mut (Option<Pk>, Option<String>, Watched)| {
            *v == viewer
                && match (n, &name) {
                    (Some(one), Some(two)) => one.eq_ignore_ascii_case(two),
                    (None, None) => true,
                    _ => false,
                }
        };
        if let Some((_, _, entry)) = entries.iter_mut().find(|e| same(e)) {
            let lists = lists || entry.lists();
            let posts = posts.or_else(|| entry.posts().cloned());
            *entry = entry.clone().watching(lists, posts);
            continue;
        }
        let entry = match &name {
            None => Watched::own(),
            Some(name) if read.account.consent.is_some() => {
                Watched::consented(name.clone(), Consent)
            }
            Some(name) => Watched::asking(name.clone()),
        };
        entries.push((viewer, name, entry.watching(lists, posts).read_as(viewer)));
    }
    entries.into_iter().map(|(_, _, entry)| entry).collect()
}

/// Starts a run of the automation `id` now, apart from the program asking:
/// `snob watch once --automation <id>` from the binary the engine starts
/// (`exe::start_as`), with `globals` before it, its errors to the
/// automations' log.
pub fn run_now(paths: &AppPaths, id: &str, globals: &[OsString]) -> Result<()> {
    // Unnamed, the program is the one asking, which may be an app that has
    // no `watch` to run.
    if !crate::system::exe::was_named() && !cfg!(test) {
        let running = std::env::current_exe().unwrap_or_default();
        let is_snob = running
            .file_stem()
            .is_some_and(|stem| stem.eq_ignore_ascii_case("snob"));
        anyhow::ensure!(is_snob, "there is no snob beside this program to run it");
    }
    let program = crate::system::exe::program().context("could not find snob to start it")?;
    let dir = paths.data_dir().join("logs");
    snob_store::paths::create_private_dir(&dir)?;
    let mut args = globals.to_vec();
    args.extend(["watch", "once", "--automation", id].map(OsString::from));
    crate::system::detach::spawn(&program, &args, None, &dir.join("automations.log"))
        .context("could not start the run")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::automation::{Account, Step};

    fn reading(what: Read, name: Option<&str>, viewer: u64) -> Planned {
        Planned {
            node: 1,
            account: Account {
                username: name.map(str::to_string),
                viewer: Some(Pk::new(viewer)),
                consent: name.map(|_| Epoch::new(1)),
            },
            what,
            only: automation::Filters::default(),
            delivered: false,
        }
    }

    #[test]
    fn the_lists_and_posts_of_one_account_are_one_entry_of_the_monitor() {
        let posts = Read::Posts {
            save_to: None,
            quality: None,
        };
        let entries = watched(
            &[
                reading(Read::Lists, Some("@Friend"), 1),
                reading(posts.clone(), Some("friend"), 1),
                reading(posts, Some("friend"), 2),
                reading(Read::Fans, None, 1),
                reading(Read::Lists, None, 1),
            ],
            None,
        );
        assert_eq!(entries.len(), 3, "the fans are not the monitor's");
        assert!(entries[0].lists() && entries[0].posts().is_some());
        assert_eq!(entries[0].name(), Some("Friend"));
        assert!(entries[0].may_run_unattended());
        assert!(!entries[1].lists() && entries[1].viewer() == Some(Pk::new(2)));
        assert_eq!(entries[2].name(), None);
    }

    #[test]
    fn an_automation_is_kept_and_found_again_and_watch_toml_is_never_removed() {
        let root = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(root.path());
        let secrets =
            SecretStore::new(paths.clone(), true).with_service("snob-ig-test-automations");
        let id = new_id(&paths);
        let mut automation = Automation::new(id.clone(), "Mine");
        automation.insert(Step::new_read(Read::Fans), (0, 0));
        save(&paths, &secrets, &automation).unwrap();
        assert_eq!(find(&paths, &id).unwrap(), Some(automation.clone()));
        assert_eq!(all(&paths).automations, [automation]);
        assert_eq!(
            next_run(&paths, &find(&paths, &id).unwrap().unwrap(), Epoch::new(0))
                .unwrap()
                .0,
            Next::Never,
            "unfinished"
        );
        assert!(remove(&paths, &id).unwrap());
        assert!(remove(&paths, WATCH_TOML).is_err());
    }

    #[test]
    fn a_finished_automation_with_a_manual_trigger_alone_runs_only_by_hand() {
        let root = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(root.path());
        let mut automation = Automation::new("ab12cd34", "By hand");
        let manual = automation.insert(Step::Manual {}, (0, 0));
        let lists = automation.insert(
            Step::Read {
                account: Some(Account::own(None)),
                what: Read::Lists,
            },
            (200, 0),
        );
        automation.connect(manual, lists).unwrap();
        assert_eq!(
            next_run(&paths, &automation, Epoch::new(1_700_000_000)).unwrap(),
            (Next::ByHand, None)
        );
    }
}
