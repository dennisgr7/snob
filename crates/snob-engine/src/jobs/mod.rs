//! A list walk that outlives the call that started it: a job.
//!
//! **Three processes, and the job's state is files.**
//!
//! **One job running per account, and nothing in one runs that was not checked
//! where it runs** (`argv.rs`): the supervisor runs itself, never a program
//! named on disk, and checks the stored command line again.
//!
//! **Stopping is a file, never a signal.**
//!
//! **Everything here is `std` and threads**, called from the async runtime
//! where waiting is involved, so jobs build without the `mcp` feature and `snob
//! jobs` and `purge` work in every build (`tokio`'s `process` is the `mcp`
//! feature's).

pub mod id;
pub mod show;
pub mod supervise;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use snob_core::Pk;
use snob_core::model::ListKind;
use snob_store::paths::{self, AccountPaths, JobId};

/// The hidden command a supervisor is started as.
pub const COMMAND: &str = "__job";

/// How long the start waits for the supervisor to take the job, before it
/// waits `--wait` for the walk (`snob_mcp::longest_call` counts both).
pub const SUPERVISOR_WITHIN: Duration = Duration::from_secs(10);

/// A job's command line, as the program that runs it reads one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    /// The command line, as it will be run.
    pub args: Vec<String>,
    /// The command, by the name the help gives it.
    pub command: &'static str,
    /// The lists it walks, in order.
    pub kinds: &'static [ListKind],
    /// Whose lists, as typed less a leading `@`; `None` is the viewer's own.
    pub target: Option<String>,
    /// Its `--max-pages`, when it has one.
    pub max_pages: Option<u32>,
}

/// Reads a stored command line as a walk a job may run, or says why not: the
/// command line's own check (`argv::check`), handed in by whoever runs jobs.
pub type Check = fn(&[String]) -> Result<Walk, String>;

/// The version of `job.json`'s shape.
pub const SCHEMA: u32 = 1;

/// How long a walk asked to stop gets before it is killed: the same as for a
/// call's child (`snob_mcp::child`), for the same reason.
pub const GRACE: Duration = Duration::from_secs(30);

pub use crate::system::detach::NEVER_INHERITED;

/// How long a job may wait for its supervisor before it counts as lost: it
/// is started at once, so a job still queued after this never will be.
pub const LOST_AFTER: Duration = Duration::from_secs(30);

/// How often a running supervisor says it is still there (`updated_at`).
pub const HEARTBEAT: Duration = Duration::from_secs(30);

/// How long a finished job's result is kept: a day, which is longer than
/// anybody waits to ask for it, and short enough that lists do not pile up.
pub const KEEP_RESULT: Duration = Duration::from_secs(24 * 3600);

/// How long a finished job's directory is kept, its record and its log.
pub const KEEP_JOB: Duration = Duration::from_secs(7 * 24 * 3600);

/// The most jobs kept for an account, however recent: the oldest finished
/// ones go first.
const MOST_KEPT: usize = 50;

/// What a job is, written once by `start`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Job {
    pub schema: u32,
    pub id: String,
    /// Milliseconds since the epoch.
    pub created_at: i64,
    pub viewer_pk: Pk,
    /// The tool call it was started for, when it was one, and its arguments,
    /// without those that only shaped how the call was answered.
    pub tool: Option<String>,
    pub arguments: Value,
    /// The command it runs, as checked (`argv::check`).
    pub argv: Vec<String>,
    /// The global flags its processes run with, **for showing only**: the
    /// supervisor takes its own from its command line (`argv::globals`).
    pub globals: Vec<String>,
    /// What the walk was estimated to cost when it was started
    /// (`commands::dry_run`), as `--dry-run`'s JSON says it.
    pub estimate: Option<Value>,
    pub detached: Option<Detached>,
    /// The client that started it, in the few characters `client_label`
    /// keeps.
    pub started_by: Option<String>,
    /// The `--max-wait` its walk runs with, in seconds.
    pub max_wait_seconds: Option<u64>,
}

/// Whether the job left the process group, or the job object, of whatever
/// started it (`detach::Started::left_job`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Detached {
    /// It goes on when its client closes.
    LeftJob,
    /// It ends when its client closes: the client's job object did not let it
    /// out (Windows), and closing the client ends the walk with it.
    InClientJob,
}

impl Detached {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LeftJob => "left_job",
            Self::InClientJob => "in_client_job",
        }
    }
}

/// How a job stands, written by its supervisor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
    /// `Running`, or how it ended.
    pub status: Status,
    pub started_at: i64,
    /// The last time the supervisor said it was there.
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    /// What the walk exited with, when it exited.
    pub exit_number: Option<i32>,
    /// Whether it was asked to stop.
    pub cancel: bool,
    /// Why it ended without running, as a token (`refused_argv`).
    pub failure: Option<String>,
    /// For showing only: nothing signals a pid read from a file.
    pub supervisor_pid: Option<u32>,
    /// The walk's, once it was started: shown, and what tells a lost job that
    /// walked from one that never did (`Seen::walked`).
    pub child_pid: Option<u32>,
}

/// How a job stands, as everything but its supervisor reads it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Made, and its supervisor not yet there.
    Queued,
    Running,
    /// Not finished, and nobody running it: its supervisor was killed, or
    /// never started.
    Lost,
    /// The walk exited 0.
    Succeeded,
    /// It exited otherwise, or did not run.
    Failed,
    /// It was asked to stop, and stopped.
    Canceled,
    /// It stopped as for Ctrl+C without being asked to, at a limit on a wait
    /// or because the system asked.
    Stopped,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Lost => "lost",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Stopped => "stopped",
        }
    }

    /// Whether it is over and will not change again.
    pub fn is_over(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }

    /// Whether the walk ran and ended.
    pub fn finished(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Canceled | Self::Stopped
        )
    }
}

/// The status a job is read as, from its state read before its lock was asked
/// about, whether the lock was held, its state read again after, when it was
/// made and the time now.
///
/// **Read twice, around the lock**, because a supervisor writes its ending and
/// then lets go of the lock, microseconds apart: a reader that read the state
/// once, before, could find it still running and the lock already free, and
/// call a job that had just succeeded `lost`, a status every waiter stops on
/// for good.
pub fn derive_around(
    before: Option<&State>,
    held: bool,
    after: Option<&State>,
    created_at: i64,
    now: i64,
) -> Status {
    if let Some(after) = after
        && after.status.is_over()
    {
        return after.status;
    }
    derive(before, held, created_at, now)
}

/// The status a job is read as, from its state, whether its lock is held, when
/// it was made and the time now.
pub fn derive(state: Option<&State>, held: bool, created_at: i64, now: i64) -> Status {
    if let Some(state) = state
        && state.status.is_over()
    {
        return state.status;
    }
    if held {
        return Status::Running;
    }
    match state {
        Some(_) => Status::Lost,
        None if now.saturating_sub(created_at) > LOST_AFTER.as_millis() as i64 => Status::Lost,
        None => Status::Queued,
    }
}

/// A job as it was read.
#[derive(Debug, Clone)]
pub struct Seen {
    pub id: JobId,
    pub dir: PathBuf,
    /// `None` when `job.json` did not read: it is then shown, and swept, and
    /// never run.
    pub job: Option<Job>,
    pub state: Option<State>,
    pub held: bool,
    pub stop_asked: bool,
    pub status: Status,
}

impl Seen {
    /// When the job was made, or when its directory was, for a record that
    /// did not read.
    pub fn created_at(&self) -> i64 {
        self.job
            .as_ref()
            .map(|job| job.created_at)
            .unwrap_or_else(|| modified_ms(&self.dir).unwrap_or(0))
    }

    /// The last moment anything of it is known to have been alive.
    pub fn last_alive(&self) -> i64 {
        let state = self.state.as_ref().map_or(0, |s| s.updated_at);
        state.max(self.created_at())
    }

    /// For a lost job: the moment its walk's claim on the list frees at the
    /// latest, which is how long another walk of that list is refused
    /// (`sentences::walked_elsewhere`). **`None` for a job that is not lost, and
    /// for a lost one that never started a walk** ([`Seen::walked`]), which
    /// holds nothing: refusing its lists for the claim's 45 minutes would
    /// refuse them over a walk that never was.
    pub fn claim_frees_at(&self) -> Option<i64> {
        (self.status == Status::Lost && self.walked()).then(|| {
            let ttl = snob_store::store::snapshots::CLAIM_TTL_SECS * 1000;
            self.last_alive()
                .saturating_add(HEARTBEAT.as_millis() as i64)
                .saturating_add(ttl)
        })
    }

    /// Whether a walk was ever started for it.
    pub fn walked(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.child_pid.is_some())
    }

    /// When it ended, or was last seen, for the retention.
    pub(crate) fn ended_at(&self) -> i64 {
        self.state
            .as_ref()
            .and_then(|s| s.finished_at)
            .unwrap_or_else(|| self.last_alive())
    }
}

/// A file of a job's directory.
pub fn file(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

/// Writes `value` as `name` in `dir`, whole or not at all
/// (`paths::replace_private`), so a reader sees the old record or the new.
pub fn write(dir: &Path, name: &str, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    paths::replace_private(&file(dir, name), &file(dir, &format!("{name}.tmp")), &bytes)
        .map_err(|(_, e)| e)
}

/// Reads `name` in `dir`: `None` when it is not there, or does not read as
/// what it should be.
pub fn read<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Option<T> {
    let bytes = std::fs::read(file(dir, name)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn modified_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

/// Reads one job of `account`'s, **without writing anything**: `None` when
/// the account has no job of that name.
pub fn look(account: &AccountPaths, id: &JobId, now: i64) -> io::Result<Option<Seen>> {
    let dir = account.job_dir(id);
    if !dir.is_dir() {
        return Ok(None);
    }
    let job: Option<Job> = read(&dir, "job.json");
    // Around the lock, not once before it (`derive_around`).
    let before: Option<State> = read(&dir, "state.json");
    let held = paths::lock_is_held(&file(&dir, "lock"))?.unwrap_or(false);
    let state: Option<State> = read(&dir, "state.json");
    let stop_asked = file(&dir, "stop").exists();
    let created_at = job
        .as_ref()
        .map(|job| job.created_at)
        .unwrap_or_else(|| modified_ms(&dir).unwrap_or(0));
    let status = derive_around(before.as_ref(), held, state.as_ref(), created_at, now);
    Ok(Some(Seen {
        id: id.clone(),
        dir,
        job,
        state,
        held,
        stop_asked,
        status,
    }))
}

/// Every job of `account`'s, oldest first.
pub fn all(account: &AccountPaths, now: i64) -> io::Result<Vec<Seen>> {
    let mut seen = Vec::new();
    for id in ids(account)? {
        if let Some(job) = look(account, &id, now)? {
            seen.push(job);
        }
    }
    Ok(seen)
}

/// The ids of `account`'s jobs, oldest first, none of them read yet: what
/// [`all`] reads one by one, and what [`Stopping::every_job_of`] reads so
/// that one job it cannot read keeps no other from being asked to stop.
fn ids(account: &AccountPaths) -> io::Result<Vec<JobId>> {
    let entries = match std::fs::read_dir(account.jobs_dir()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut ids: Vec<JobId> = entries
        .flatten()
        .filter_map(|entry| JobId::parse(entry.file_name().to_str()?))
        .collect();
    ids.sort();
    Ok(ids)
}

/// Takes the lock every start and every sweep of `account`'s jobs holds
/// (`jobs/.lock`), creating the jobs directory, private, when it is not there.
///
/// `None` when the account's own directory is gone: **never made again here**
/// (`AccountPaths::create_jobs_dir`), since a start that waited on the lock of
/// an account a purge was removing would otherwise put it back, and with no
/// list of who is signed in, which sends as every session (an account with a
/// session stored has its directory, which `login` makes).
pub fn lock_jobs(account: &AccountPaths) -> io::Result<Option<std::fs::File>> {
    if !account.create_jobs_dir().map_err(io::Error::other)? {
        return Ok(None);
    }
    paths::lock_file(&account.jobs_dir().join(".lock")).map(Some)
}

/// [`sweep`], when nobody holds the jobs lock, and nothing when somebody
/// does (a start, which sweeps itself, or a purge or a logout): for the
/// commands that read the jobs (`show`, `list`, `cancel`), so that what is
/// past its keeping goes when it is due and not only when the account's next
/// job starts. A finished job's result is other people's lists, and it is
/// kept a day, as AGENTS.md promises. Makes no directory.
pub fn sweep_if_free(account: &AccountPaths, now: i64) {
    if !account.jobs_dir().is_dir() {
        return;
    }
    if let Ok(Some(_lock)) = paths::try_lock_file(&account.jobs_dir().join(".lock")) {
        sweep(account, now);
    }
}

/// Asks a job to stop.
pub fn ask_to_stop(seen: &Seen) -> io::Result<()> {
    private_marker(&file(&seen.dir, "stop"))
}

/// An empty file whose being there is what it says (`lock`, `stop`), made
/// `0600` on Unix like every other file of a job's, and left as it is when
/// it is there already.
pub fn private_marker(path: &Path) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path).map(drop)
}

/// Forgets what `account`'s jobs no longer need, under the jobs lock the
/// caller holds: a finished job's result after [`KEEP_RESULT`], the whole job
/// after [`KEEP_JOB`], and the oldest past [`MOST_KEPT`]. **A job whose lock
/// is held is never touched**, however old.
pub fn sweep(account: &AccountPaths, now: i64) {
    let Ok(jobs) = all(account, now) else {
        return;
    };
    let mut kept = Vec::new();
    for seen in jobs {
        if seen.held || !seen.status.is_over() {
            kept.push(seen);
            continue;
        }
        let age = now.saturating_sub(seen.ended_at());
        if age > KEEP_JOB.as_millis() as i64 {
            let _ = paths::remove_tree(&seen.dir);
            continue;
        }
        if age > KEEP_RESULT.as_millis() as i64 {
            let _ = std::fs::remove_file(file(&seen.dir, "out.json"));
            let _ = std::fs::remove_file(file(&seen.dir, "out.json.partial"));
        }
        kept.push(seen);
    }
    // Oldest first, so the ones past the count are at the front.
    let over = kept.len().saturating_sub(MOST_KEPT);
    for seen in kept
        .iter()
        .filter(|seen| !seen.held && seen.status.is_over())
        .take(over)
    {
        let _ = paths::remove_tree(&seen.dir);
    }
}

/// `account`'s jobs being stopped, and the jobs lock held, when it can be,
/// so no other starts until this is dropped (`purge`, `logout`).
pub struct Stopping {
    pub ids: Vec<JobId>,
    account: AccountPaths,
    _lock: Option<std::fs::File>,
}

impl Stopping {
    /// Asks every job of `account`'s that is not over to stop, holding the jobs
    /// lock, **taken whether or not the account has run a job yet** (its jobs
    /// directory is made for it), so no job starts until this is dropped: a
    /// start waiting on the lock then finds the session gone and makes nothing
    /// (`start::make`).
    ///
    /// **Never a reason to keep a session**: `purge` and `logout` call this
    /// before they delete anything, and bookkeeping for jobs that may not exist
    /// must not keep a credential from going. A jobs directory or `.lock` that
    /// cannot be made or taken (a stray file in its place, a full disk, an ACL
    /// that refuses) is warned about, and **the jobs are asked to stop all the
    /// same**, without the lock: no new job can start while it cannot be taken
    /// (`start::make` takes the same one), but a running one does not hold it
    /// (each holds its own `lock`) and would otherwise go on with the session
    /// it holds in memory after the session is deleted. **Each job is read on
    /// its own**: one that cannot be read (its `lock` cannot be opened, say) is
    /// warned about, still sent a `stop` in case it is running, and not waited
    /// for, and every other is asked to stop as usual.
    pub fn every_job_of(account: &AccountPaths) -> Self {
        let lock = match lock_jobs(account) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                return Self {
                    ids: Vec::new(),
                    account: account.clone(),
                    _lock: None,
                };
            }
            Err(e) => {
                crate::host::warn(&format!(
                    "could not hold the jobs of {}: {e}; asking them to stop without it",
                    account.dir().display()
                ));
                None
            }
        };
        Self {
            ids: ask_every_job_to_stop(account),
            account: account.clone(),
            _lock: lock,
        }
    }

    /// Waits, at most `within`, for every job asked to stop to be over and let
    /// go of its lock.
    pub fn wait(&self, within: Duration) -> bool {
        let deadline = std::time::Instant::now() + within;
        loop {
            let now = snob_core::clock::now_ms().get();
            let settled = self
                .ids
                .iter()
                .all(|id| match look(&self.account, id, now) {
                    Ok(Some(seen)) => seen.status.is_over() && !seen.held,
                    _ => true,
                });
            if settled {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Asks every job of `account`'s that is not over to stop, each read on its own
/// ([`Stopping::every_job_of`]), and answers the ones asked, to be waited for.
fn ask_every_job_to_stop(account: &AccountPaths) -> Vec<JobId> {
    let now = snob_core::clock::now_ms().get();
    let every = ids(account).unwrap_or_else(|e| {
        crate::host::warn(&format!(
            "could not read the jobs of {}: {e}; going on without stopping them",
            account.dir().display()
        ));
        Vec::new()
    });
    let mut asked = Vec::new();
    for id in every {
        let seen = match look(account, &id, now) {
            Ok(Some(seen)) => seen,
            Ok(None) => continue,
            Err(e) => {
                // What it is doing is unknown, so it is asked all the
                // same; a `stop` in a job that is over changes nothing.
                let sent = private_marker(&file(&account.job_dir(&id), "stop"));
                crate::host::warn(&format!(
                    "could not read job {}: {e}; {}, and going on",
                    id.as_str(),
                    match sent {
                        Ok(()) => "asked it to stop",
                        Err(_) => "could not ask it to stop either",
                    }
                ));
                continue;
            }
        };
        if seen.status.is_over() {
            continue;
        }
        match ask_to_stop(&seen) {
            Ok(()) => asked.push(seen.id),
            Err(e) => crate::host::warn(&format!(
                "could not ask job {} to stop: {e}; going on",
                seen.id.as_str()
            )),
        }
    }
    asked
}

/// How long [`Stopping::wait`] waits: the grace a walk gets, and a little for
/// the supervisor to write its ending.
pub const STOPPING_WITHIN: Duration = Duration::from_secs(GRACE.as_secs() + 5);

/// For `purge` and `logout`: stops the jobs of `accounts`, holding each
/// one's jobs lock so no other starts, and waits for them to have stopped
/// (`jobs::Stopping`), answering whether every one did; **the removal goes on
/// either way** ([`Stopping::every_job_of`] does not fail), so a session is
/// never kept over its jobs.
pub async fn stop_every_job(accounts: Vec<AccountPaths>) -> anyhow::Result<(Vec<Stopping>, bool)> {
    Ok(tokio::task::spawn_blocking(move || {
        let stopping: Vec<Stopping> = accounts.iter().map(Stopping::every_job_of).collect();
        // Every one waited for, whether or not one before it settled.
        let late = stopping
            .iter()
            .filter(|jobs| !jobs.wait(STOPPING_WITHIN))
            .count();
        let settled = late == 0;
        (stopping, settled)
    })
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(status: Status) -> State {
        State {
            status,
            started_at: 0,
            updated_at: 0,
            finished_at: None,
            exit_number: None,
            cancel: false,
            failure: None,
            supervisor_pid: None,
            child_pid: None,
        }
    }

    #[test]
    fn a_job_whose_lock_is_free_and_state_unfinished_is_lost() {
        let running = state(Status::Running);
        assert_eq!(derive(Some(&running), false, 0, 1), Status::Lost);
        assert_eq!(derive(Some(&running), true, 0, 1), Status::Running);
        // Queued for longer than a supervisor takes to start.
        let late = LOST_AFTER.as_millis() as i64 + 1;
        assert_eq!(derive(None, false, 0, late), Status::Lost);
        // An ending is the answer, held or not.
        for over in [
            Status::Succeeded,
            Status::Failed,
            Status::Canceled,
            Status::Stopped,
        ] {
            assert_eq!(derive(Some(&state(over)), true, 0, 1), over);
            assert_eq!(derive(Some(&state(over)), false, 0, late), over);
        }
    }

    /// A supervisor writes its ending and lets go of its lock in a few
    /// microseconds; a reader between the two reads is never told `lost`.
    #[test]
    fn a_job_read_as_it_ends_or_starts_is_never_lost() {
        let running = state(Status::Running);
        let ended = state(Status::Succeeded);
        // Running when first read, its lock free by the time it was asked,
        // and its ending there when read again: the ending.
        assert_eq!(
            derive_around(Some(&running), false, Some(&ended), 0, 1),
            Status::Succeeded
        );
        // Not there when first read, its lock free, and running when read
        // again: its supervisor took it in between, so it is starting.
        assert_eq!(
            derive_around(None, false, Some(&running), 1_000, 1_000),
            Status::Queued
        );
        // Running before and after, with its lock free: gone.
        assert_eq!(
            derive_around(Some(&running), false, Some(&running), 0, 1),
            Status::Lost
        );
        assert_eq!(
            derive_around(Some(&running), true, Some(&running), 0, 1),
            Status::Running
        );
    }

    #[test]
    fn a_young_queued_job_is_not_lost() {
        assert_eq!(derive(None, false, 1_000, 1_000), Status::Queued);
        assert_eq!(
            derive(None, false, 1_000, 1_000 + LOST_AFTER.as_millis() as i64),
            Status::Queued
        );
        assert_eq!(derive(None, true, 1_000, 1_000), Status::Running);
    }

    fn account(tmp: &tempfile::TempDir) -> AccountPaths {
        paths::AppPaths::rooted_at(tmp.path()).account(Pk::new(42))
    }

    /// A job directory as `start` and a supervisor leave it.
    fn made(account: &AccountPaths, at: i64, over: Option<(Status, i64)>) -> JobId {
        let id = id::new(at as u64).unwrap();
        let dir = account.job_dir(&id);
        paths::create_private_dir(&dir).unwrap();
        let job = Job {
            schema: SCHEMA,
            id: id.to_string(),
            created_at: at,
            viewer_pk: account.pk(),
            tool: None,
            arguments: Value::Null,
            argv: vec!["followers".into()],
            globals: Vec::new(),
            estimate: None,
            detached: None,
            started_by: None,
            max_wait_seconds: None,
        };
        write(&dir, "job.json", &job).unwrap();
        std::fs::write(file(&dir, "out.json"), b"[]").unwrap();
        if let Some((status, finished)) = over {
            let mut ended = state(status);
            ended.finished_at = Some(finished);
            write(&dir, "state.json", &ended).unwrap();
        }
        id
    }

    #[test]
    fn retention_keeps_running_jobs_and_drops_old_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let account = account(&tmp);
        let day = 24 * 3600 * 1000;
        let now = 100 * day;
        let ancient_running = made(&account, now - 30 * day, None);
        let held = paths::try_lock_file(&file(&account.job_dir(&ancient_running), "lock"))
            .unwrap()
            .unwrap();
        let week_old = made(
            &account,
            now - 9 * day,
            Some((Status::Succeeded, now - 8 * day)),
        );
        let two_days = made(
            &account,
            now - 2 * day,
            Some((Status::Failed, now - 2 * day)),
        );
        let fresh = made(&account, now - 1000, Some((Status::Succeeded, now - 1000)));

        sweep(&account, now);
        assert!(account.job_dir(&ancient_running).join("out.json").exists());
        assert!(!account.job_dir(&week_old).exists());
        assert!(account.job_dir(&two_days).exists());
        assert!(!account.job_dir(&two_days).join("out.json").exists());
        assert!(account.job_dir(&fresh).join("out.json").exists());

        // Past the count, the oldest finished go and the running one stays.
        for n in 0..MOST_KEPT {
            made(
                &account,
                now - 500 + n as i64,
                Some((Status::Succeeded, now - 10)),
            );
        }
        sweep(&account, now);
        let left = all(&account, now).unwrap();
        assert_eq!(left.len(), MOST_KEPT);
        assert!(left.iter().any(|seen| seen.id == ancient_running));
        assert!(!left.iter().any(|seen| seen.id == two_days));
        drop(held);
    }

    #[test]
    fn stopping_writes_a_stop_file_into_every_job_that_is_not_over() {
        let tmp = tempfile::tempdir().unwrap();
        let account = account(&tmp);
        let now = snob_core::clock::now_ms().get();
        let queued = made(&account, now, None);
        let done = made(&account, now, Some((Status::Succeeded, now)));
        let stopping = Stopping::every_job_of(&account);
        assert_eq!(stopping.ids, std::slice::from_ref(&queued));
        assert!(account.job_dir(&queued).join("stop").exists());
        assert!(!account.job_dir(&done).join("stop").exists());
        // A queued job nobody runs is not over until it is lost.
        assert!(!stopping.wait(Duration::from_millis(200)));
    }

    /// A job that cannot be read (its `lock` cannot be opened) keeps no
    /// other from being asked to stop: before it, a `logout` that met one
    /// asked none of the account's jobs to stop and deleted the session
    /// under their walks. It is sent a `stop` itself, in case it runs, and
    /// not waited for.
    #[test]
    fn a_job_that_cannot_be_read_keeps_no_other_from_being_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let account = account(&tmp);
        let now = snob_core::clock::now_ms().get();
        let unreadable = made(&account, now - 1, None);
        let lock = file(&account.job_dir(&unreadable), "lock");
        // On Windows a directory cannot be opened as a file. On Unix it can,
        // and be locked too, so there the lock is a link to itself, which
        // fails to open with ELOOP even as root (`tools/ci`'s container
        // runs as root, past a file without permissions).
        #[cfg(windows)]
        std::fs::create_dir(&lock).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("lock", &lock).unwrap();
        assert!(look(&account, &unreadable, now).is_err());
        let queued = made(&account, now, None);
        let stopping = Stopping::every_job_of(&account);
        assert_eq!(stopping.ids, std::slice::from_ref(&queued));
        assert!(account.job_dir(&queued).join("stop").exists());
        assert!(account.job_dir(&unreadable).join("stop").exists());
    }

    /// An account that has never run a job is still held while it is left,
    /// so none starts in between; its jobs directory is made to hold it.
    #[test]
    fn an_account_with_no_jobs_yet_is_held_while_it_is_left() {
        let tmp = tempfile::tempdir().unwrap();
        let account = account(&tmp);
        account.ensure_dirs().unwrap();
        let stopping = Stopping::every_job_of(&account);
        assert!(stopping.ids.is_empty());
        let lock = account.jobs_dir().join(".lock");
        assert!(paths::try_lock_file(&lock).unwrap().is_none(), "held");
        drop(stopping);
        assert!(paths::try_lock_file(&lock).unwrap().is_some(), "let go of");
    }

    /// A jobs directory that cannot be made (a plain file in its place) is no
    /// jobs to stop, not a failure that would keep `logout` or `purge` from
    /// deleting the session; no job could start there either.
    #[tokio::test]
    async fn a_jobs_directory_that_cannot_be_made_stops_nothing_and_fails_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let account = account(&tmp);
        account.ensure_dirs().unwrap();
        std::fs::write(account.jobs_dir(), b"x").unwrap();
        assert!(lock_jobs(&account).is_err(), "the lock cannot be taken");
        let (stopping, _) = stop_every_job(vec![account.clone()]).await.unwrap();
        assert_eq!(stopping.len(), 1);
        assert!(stopping[0].ids.is_empty());
        assert!(stopping[0]._lock.is_none());
    }

    /// A jobs lock that cannot be taken (`.lock` a directory, which cannot
    /// be opened) still has every job that is not over asked to stop, and
    /// waited for: a running job does not hold that lock, and before this a
    /// `logout` that met one deleted the session under its walk without a
    /// word to it.
    #[test]
    fn a_jobs_lock_that_cannot_be_taken_still_stops_every_job() {
        let tmp = tempfile::tempdir().unwrap();
        let account = account(&tmp);
        account.ensure_dirs().unwrap();
        let now = snob_core::clock::now_ms().get();
        let queued = made(&account, now, None);
        let done = made(&account, now, Some((Status::Succeeded, now)));
        std::fs::create_dir(account.jobs_dir().join(".lock")).unwrap();
        assert!(lock_jobs(&account).is_err(), "the lock cannot be taken");
        let stopping = Stopping::every_job_of(&account);
        assert!(stopping._lock.is_none());
        assert_eq!(stopping.ids, std::slice::from_ref(&queued));
        assert!(account.job_dir(&queued).join("stop").exists());
        assert!(!account.job_dir(&done).join("stop").exists());
    }

    /// A read sweeps what is past its keeping when nobody holds the jobs
    /// lock, and leaves it while somebody does.
    #[test]
    fn a_read_sweeps_when_the_lock_is_free() {
        let tmp = tempfile::tempdir().unwrap();
        let account = account(&tmp);
        let day = 24 * 3600 * 1000;
        let now = 100 * day;
        let old = made(
            &account,
            now - 2 * day,
            Some((Status::Succeeded, now - 2 * day)),
        );
        let held = lock_jobs(&account).unwrap().unwrap();
        sweep_if_free(&account, now);
        assert!(account.job_dir(&old).join("out.json").exists(), "busy");
        drop(held);
        sweep_if_free(&account, now);
        assert!(!account.job_dir(&old).join("out.json").exists());
    }

    #[test]
    fn an_account_with_no_jobs_has_none_to_stop_and_gets_no_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let account = account(&tmp);
        let stopping = Stopping::every_job_of(&account);
        assert!(stopping.ids.is_empty());
        assert!(stopping.wait(Duration::ZERO));
        assert!(!account.jobs_dir().exists());
    }
}
