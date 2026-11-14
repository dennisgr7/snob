//! Where the tool keeps its data, following each platform's conventions.
//!
//! **Per user, never per directory**: everything comes from `directories`.
//! Data goes to the **local** directory, not the one that roams: the
//! database uses WAL, and WAL on a synced folder (OneDrive, a roaming
//! profile) is a documented cause of SQLite corruption. Configuration stays
//! in the syncable one. The two differ on Windows only.

use std::path::{Path, PathBuf};

use thiserror::Error;

const QUALIFIER: &str = "";
const ORGANIZATION: &str = "";
const APPLICATION: &str = "snob-ig";

#[derive(Debug, Error)]
pub enum PathError {
    #[error("could not determine the user's data directory")]
    NoHome,
    #[error("could not create the directory {path}: {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The directory exists and could not be limited to this account: the
    /// half worth naming, since the database inside is the whole follower
    /// history in the clear.
    #[error("could not restrict {path} to your account: {detail}")]
    NotPrivate { path: PathBuf, detail: String },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct AppPaths {
    config: PathBuf,
    data: PathBuf,
    /// The data directory that roams with the profile, on Windows only.
    /// Nothing reads or writes it; [`Self::owned_dirs`] lists it because a
    /// build older than the first release may have left a session there, and
    /// `purge` leaves nothing behind.
    legacy_data: Option<PathBuf>,
    /// Set by [`Self::rooted_at`]: the scratch directory comes from the
    /// environment, and under a sandbox it must stay under the root like
    /// every other file the run touches.
    sandbox: Option<PathBuf>,
}

impl AppPaths {
    pub fn discover() -> Result<Self, PathError> {
        let dirs = directories::ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
            .ok_or(PathError::NoHome)?;

        let data = dirs.data_local_dir().to_path_buf();
        let previous = dirs.data_dir().to_path_buf();

        Ok(Self {
            config: dirs.config_dir().to_path_buf(),
            legacy_data: (previous != data).then_some(previous),
            data,
            sandbox: None,
        })
    }

    /// For tests: puts every directory under one root.
    pub fn rooted_at(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            config: root.join("config"),
            data: root.join("data"),
            // Always defined, so that `purge` removing it is tested on every
            // platform.
            legacy_data: Some(root.join("legacy-data")),
            sandbox: Some(root.to_path_buf()),
        }
    }

    /// Where configuration lives: `watch.toml`, written by `snob watch setup`.
    /// Created by what writes there, never by [`Self::ensure_dirs`].
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// The accounts' own directories, one per account snob has signed in as.
    pub fn accounts_dir(&self) -> PathBuf {
        self.data.join("accounts")
    }

    /// The accounts snob holds a session for, and which one is active.
    pub fn registry_file(&self) -> PathBuf {
        self.data.join("accounts.toml")
    }

    /// What every account shares: the push-backs any of them received.
    pub fn shared_db_file(&self) -> PathBuf {
        self.data.join("shared.db")
    }

    /// Where a database whose account could not be told goes, until a login
    /// claims it.
    pub fn unclaimed_dir(&self) -> PathBuf {
        self.accounts_dir().join("unclaimed")
    }

    /// The accounts that have a directory, named after their id, by id: the
    /// order a directory is read in differs between systems.
    pub fn account_dirs(&self) -> Vec<snob_core::Pk> {
        let Ok(entries) = std::fs::read_dir(self.accounts_dir()) else {
            return Vec::new();
        };
        let mut pks: Vec<snob_core::Pk> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
            .collect();
        pks.sort();
        pks
    }

    /// Everything that belongs to one account.
    pub fn account(&self, pk: snob_core::Pk) -> AccountPaths {
        AccountPaths {
            app: self.clone(),
            pk,
        }
    }

    // The single-account layout, which the per-account one replaced. Only the
    // migration and the removal of every secret read them.

    /// The single-account layout's database.
    pub(crate) fn legacy_db_file(&self) -> PathBuf {
        self.data.join("snob.db")
    }

    /// The single-account layout's session file, used when the system keyring
    /// is unavailable.
    pub(crate) fn legacy_session_file(&self) -> PathBuf {
        self.data.join("session.json")
    }

    /// The directory every browser profile snob uses lives under. Never the
    /// user's real profile. `snob logout --all` removes every profile in it:
    /// each holds a live session.
    pub fn browser_profile(&self) -> PathBuf {
        self.data.join("browser-profile")
    }

    /// The browser profile of one account: the device its requests are sent
    /// from. **One per account**: a profile shared by two would be emptied at
    /// every switch, and each session would come back from a browser
    /// Instagram had never seen.
    pub fn browser_profile_for(&self, pk: snob_core::Pk) -> PathBuf {
        self.browser_profile().join(pk.to_string())
    }

    /// What the browsers said about this machine, kept once for every profile:
    /// it is about the machine, not about any account.
    pub fn browser_hints_file(&self) -> PathBuf {
        self.data.join("browser-hints.json")
    }

    /// Where a story about to be handed to the system viewer is put: under
    /// the system's temporary directory (`$XDG_RUNTIME_DIR` on Linux,
    /// [`runtime_dir`]), one directory per process. Measured on Windows 11:
    /// the viewers hold no handle on the file, `%TEMP%` and `%LOCALAPPDATA%`
    /// carry identical ACLs, and Windows Search crawls the latter, so the
    /// data directory would buy nothing.
    ///
    /// **The process id is in the name on purpose**: two runs must not share
    /// a directory one of them will delete, and a fixed name in a shared
    /// temporary space is one anybody can create first. `purge` removes the
    /// parent ([`Self::owned_dirs`]): the system's cleaner is not to be
    /// leaned on.
    pub fn story_scratch(&self) -> PathBuf {
        self.stories_root()
            .join(format!("run-{}", std::process::id()))
    }

    /// The parent of every [`Self::story_scratch`], which is what `purge`
    /// removes and what the age sweep walks.
    pub fn stories_root(&self) -> PathBuf {
        // A sandbox root replaces every location a run touches, this one
        // included.
        match &self.sandbox {
            Some(root) => root.join("stories"),
            None => runtime_dir().join("snob-ig-stories"),
        }
    }

    /// Every directory this tool may have created, for `snob purge`: listed
    /// here so that one added later is not forgotten. Deduplicated, since on
    /// macOS the configuration and data directories are the same path.
    pub fn owned_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let stories = self.stories_root();
        for dir in [
            Some(&self.data),
            Some(&self.config),
            self.legacy_data.as_ref(),
            // The one location outside the data directory this tool writes to.
            Some(&stories),
        ]
        .into_iter()
        .flatten()
        {
            if !dirs.contains(dir) {
                dirs.push(dir.clone());
            }
        }
        dirs
    }

    /// Creates what every run writes to: the data directory, and only that.
    /// The configuration directory is made by what writes configuration, so
    /// that no empty folder is left in every roaming profile.
    pub fn ensure_dirs(&self) -> Result<(), PathError> {
        create_private_dir(&self.data)
    }
}

/// Where one account's own files are: its database, its session file and its
/// browser profile. A type of its own so that the database and the session
/// file are reachable only from here, never from the [`AppPaths`] it derefs
/// to.
#[derive(Debug, Clone)]
pub struct AccountPaths {
    app: AppPaths,
    pk: snob_core::Pk,
}

impl AccountPaths {
    pub fn pk(&self) -> snob_core::Pk {
        self.pk
    }

    /// The account's directory, named after its id: a username can change,
    /// the id cannot.
    pub fn dir(&self) -> PathBuf {
        self.app.accounts_dir().join(self.pk.to_string())
    }

    /// Its lists, walks, monitor history, budget and cooldowns.
    pub fn db_file(&self) -> PathBuf {
        self.dir().join("snob.db")
    }

    /// Its session, when the system keyring is unavailable.
    pub fn session_file(&self) -> PathBuf {
        self.dir().join("session.json")
    }

    /// The browser profile its requests are sent from.
    pub fn browser_profile(&self) -> PathBuf {
        self.app.browser_profile_for(self.pk)
    }

    /// Held, with [`try_lock_file`], by a run that may send requests for this
    /// account only while no other such run does (`--spend-alone`). The lock
    /// goes with the process; the empty file left behind means nothing.
    pub fn spend_lock(&self) -> PathBuf {
        self.dir().join("spend.lock")
    }

    /// Where the account's jobs are kept (`snob-cli`'s `jobs`), one
    /// directory each, beside the `.lock` every start and every sweep holds.
    pub fn jobs_dir(&self) -> PathBuf {
        self.dir().join("jobs")
    }

    /// Makes [`Self::jobs_dir`], private, **only inside an account directory
    /// that is there**: `false`, and nothing made, when it is not. Never by
    /// `create_dir_all`, which would make the account's directory again when
    /// a `purge` removed it between a look and the making, and with no list
    /// of who is signed in ([`Self::signed_in`]), which sends as every
    /// session.
    pub fn create_jobs_dir(&self) -> Result<bool, PathError> {
        let dir = self.jobs_dir();
        match std::fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => {}
            Err(source) => return Err(PathError::Create { path: dir, source }),
        }
        restrict_to_this_account(&dir)?;
        Ok(true)
    }

    /// One job's directory. Only ever from a [`JobId`], which is checked, so
    /// no name read from anywhere can reach outside [`AccountPaths::jobs_dir`].
    pub fn job_dir(&self, id: &JobId) -> PathBuf {
        self.jobs_dir().join(id.as_str())
    }

    /// Creates the data directory and the account's, both private.
    ///
    /// **An account directory made here starts with nobody signed in**
    /// ([`Self::signed_in`], empty): what made it again after a `purge` is
    /// not a login, and a session somebody still holds from before the
    /// purge must stay refused once it is back. A login lets its own session
    /// in after ([`Self::admit_session`]).
    pub fn ensure_dirs(&self) -> Result<(), PathError> {
        self.app.ensure_dirs()?;
        let made = !self.dir().try_exists().unwrap_or(true);
        create_private_dir(&self.dir())?;
        if made {
            start_signed_out(&self.dir()).map_err(|source| PathError::Write {
                path: self.signed_in(),
                source,
            })?;
        }
        Ok(())
    }

    /// Which sessions of the account may be sent as, by any process on this
    /// machine: one `created_at` a line, in whole seconds. There and empty,
    /// none is. **Not there, in a directory that is, every session is sent
    /// as**: a directory from before this file existed. Every way a directory
    /// is made starts one, empty ([`Self::ensure_dirs`], the layout's move),
    /// and the first sign-out or login of an old one starts it.
    ///
    /// A session is named by when it was made because Instagram rotates the
    /// `sessionid` itself, and what the browser rotated is written back with
    /// the `created_at` it had. **Compared for equality only, never
    /// ordered**: a clock set back would make a later login look older.
    ///
    /// **Every rewrite holds [`Self::signed_in_lock`]**. Only making it where
    /// there is none goes without ([`start_signed_out`]): a file made new
    /// cannot put back what another process took out.
    pub fn signed_in(&self) -> PathBuf {
        self.dir().join(SIGNED_IN)
    }

    /// The lock beside [`Self::signed_in`] that its writers take, one at a
    /// time ([`lock_file`]): a login that trims a full list reads it and
    /// moves a copy over it, and a sign-out between the two would be undone
    /// by the move. A separate file because the list itself is replaced.
    /// Readers take none: the move and the appended lines are each seen
    /// whole.
    pub fn signed_in_lock(&self) -> PathBuf {
        self.dir().join(SIGNED_IN_LOCK)
    }

    /// Takes [`Self::signed_in_lock`], waiting for whoever holds it; held
    /// until the file returned is dropped. Never makes the account's
    /// directory: with none it fails as `NotFound`.
    fn hold_signed_in(&self) -> std::io::Result<std::fs::File> {
        pause_at(Seam::Locking);
        lock_file(&self.signed_in_lock())
    }

    /// Signs the account out for every process on this machine: no session
    /// it has now is sent as ([`Self::refuses_session`]) until a login lets
    /// its own in ([`Self::admit_session`]). With no directory it writes
    /// nothing and makes none: a directory that is gone refuses every
    /// session already.
    pub fn sign_out(&self) -> std::io::Result<()> {
        // The lock's file is made in the directory, never the directory, so
        // one that is gone, or goes as this looks, is not made again.
        let _held = match self.hold_signed_in() {
            Ok(held) => held,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        std::fs::File::create(self.signed_in()).map(drop)
    }

    /// Lets the session a login has just made, at `made`, be sent as by every
    /// process: it goes on [`Self::signed_in`], making the account's
    /// directory if it has none.
    ///
    /// **A list not there is started with this one line**, never read as
    /// "every session is sent as": the directory may be one another process
    /// has just made and is about to give its empty list. **Kept to
    /// [`SIGNED_IN_KEPT`] sessions**, the earliest let in going first.
    pub fn admit_session(&self, made: snob_core::Epoch) -> Result<(), PathError> {
        self.ensure_dirs()?;
        let _held = self.hold_signed_in().map_err(|source| PathError::Write {
            path: self.signed_in_lock(),
            source,
        })?;
        self.admit_session_held(made)
    }

    /// [`Self::admit_session`], for a caller holding
    /// [`Self::signed_in_lock`]. Never takes it again: a second lock on the
    /// same file, through another handle, waits for the first even in the
    /// same process.
    fn admit_session_held(&self, made: snob_core::Epoch) -> Result<(), PathError> {
        use std::io::Write as _;
        let list = self.signed_in();
        let wrote = |source| PathError::Write {
            path: list.clone(),
            source,
        };
        let line = format!("{}\n", made.get());
        let read = std::fs::read(&list);
        pause_at(Seam::Read);
        let mut admitted = match read {
            Ok(bytes) => sessions_in(&bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&list)
                {
                    Ok(mut file) => return file.write_all(line.as_bytes()).map_err(wrote),
                    // Made a moment ago by another process, empty or with
                    // another login's line: this one is added to it.
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Vec::new(),
                    Err(e) => return Err(wrote(e)),
                }
            }
            Err(e) => return Err(wrote(e)),
        };
        if admitted.contains(&made.get()) {
            return Ok(());
        }
        if admitted.len() < SIGNED_IN_KEPT {
            // Appended, one write of a whole line: a reader sees it whole or
            // not at all ([`sessions_in`] takes only finished lines).
            return std::fs::OpenOptions::new()
                .append(true)
                .open(&list)
                .and_then(|mut file| file.write_all(line.as_bytes()))
                .map_err(wrote);
        }
        admitted.drain(..=admitted.len() - SIGNED_IN_KEPT);
        admitted.push(made.get());
        let text: String = admitted.iter().map(|at| format!("{at}\n")).collect();
        // Written beside and moved over it, so no reader sees it half done.
        // **Only under the lock**: the move puts back what was read, and a
        // sign-out since would be undone ([`Self::signed_in_lock`]).
        let beside = list.with_extension(format!("{}", std::process::id()));
        std::fs::write(&beside, text)
            .and_then(|()| std::fs::rename(&beside, &list))
            .map_err(|e| {
                let _ = std::fs::remove_file(&beside);
                wrote(e)
            })
    }

    /// Lets a login's session in ([`Self::admit_session`]), where `kept` is
    /// the session stored for the account before it, if any.
    ///
    /// **A login that fails leaves what worked before working.** A login is
    /// let in before Instagram is asked about it. On a directory from before
    /// the list existed (every session sent as), starting the list with the
    /// login alone would refuse the session still stored, so there the stored
    /// one is let in with it, **only in the step that makes the list**: a
    /// list already there, a logout's included, gets the login's line alone.
    /// Both steps hold [`Self::signed_in_lock`].
    pub fn admit_login(
        &self,
        made: snob_core::Epoch,
        kept: Option<snob_core::Epoch>,
    ) -> Result<(), PathError> {
        use std::io::Write as _;
        let Some(kept) = kept else {
            return self.admit_session(made);
        };
        // With no directory, nothing from before is let in: the one a login
        // makes starts empty ([`Self::ensure_dirs`]).
        let _held = match self.hold_signed_in() {
            Ok(held) => held,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return self.admit_session(made);
            }
            Err(source) => {
                return Err(PathError::Write {
                    path: self.signed_in_lock(),
                    source,
                });
            }
        };
        let list = self.signed_in();
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&list)
        {
            Ok(mut file) => {
                let lines = format!("{}\n{}\n", kept.get(), made.get());
                file.write_all(lines.as_bytes())
                    .map_err(|source| PathError::Write { path: list, source })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                self.admit_session_held(made)
            }
            Err(source) => Err(PathError::Write { path: list, source }),
        }
    }

    /// Whether the session made at `made` may no longer be sent as: the
    /// account was signed out of here since, and no login let that session
    /// in again ([`Self::signed_in`]); or the account's directory is gone, as
    /// `purge` leaves it; or the list cannot be read. A look at one small
    /// file, asked before every request.
    pub fn refuses_session(&self, made: snob_core::Epoch) -> bool {
        match std::fs::read(self.signed_in()) {
            Ok(bytes) => !sessions_in(&bytes).contains(&made.get()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                !self.dir().try_exists().unwrap_or(false)
            }
            Err(_) => true,
        }
    }
}

/// How many sessions [`AccountPaths::signed_in`] keeps.
pub const SIGNED_IN_KEPT: usize = 64;

/// The name of [`AccountPaths::signed_in`] in an account's directory.
const SIGNED_IN: &str = "signed-in";

/// The name of [`AccountPaths::signed_in_lock`] in an account's directory.
const SIGNED_IN_LOCK: &str = "signed-in.lock";

/// The points in a rewrite of [`AccountPaths::signed_in`] where a test may
/// hold one thread still, to put another's rewrite in between
/// ([`pause_at`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seam {
    /// About to take [`AccountPaths::signed_in_lock`].
    Locking,
    /// A login has read the list and not yet written it.
    Read,
}

/// What a test runs at a [`Seam`], on its own thread.
#[cfg(test)]
type Pause = Box<dyn Fn(Seam)>;

#[cfg(test)]
thread_local! {
    /// What [`pause_at`] runs on this thread, set by a test; nothing by
    /// default. Per thread, so the other tests, running beside it in the
    /// same process, are never held.
    static PAUSE: std::cell::RefCell<Option<Pause>> = const { std::cell::RefCell::new(None) };
}

/// Nothing outside the tests; in them, whatever this thread's [`PAUSE`] says.
fn pause_at(seam: Seam) {
    #[cfg(test)]
    PAUSE.with_borrow(|pause| {
        if let Some(pause) = pause {
            pause(seam);
        }
    });
    #[cfg(not(test))]
    let _ = seam;
}

/// Starts the list of who is signed in, empty, in `dir`, an account's
/// directory or one about to become it. **Never over a list already there**:
/// another process may have let a login in since.
pub(crate) fn start_signed_out(dir: &Path) -> std::io::Result<()> {
    match std::fs::File::create_new(dir.join(SIGNED_IN)) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

/// The sessions a [`AccountPaths::signed_in`] list names: its finished lines,
/// each a number. A line still being written, with no end yet, names
/// nothing, and neither does one that is not a number.
fn sessions_in(bytes: &[u8]) -> Vec<i64> {
    let finished = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(&[][..], |end| &bytes[..end]);
    String::from_utf8_lossy(finished)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

impl std::ops::Deref for AccountPaths {
    type Target = AppPaths;

    fn deref(&self) -> &AppPaths {
        &self.app
    }
}

/// The name of one job: `j` and 26 characters of Crockford's base32
/// (`snob-cli`'s `jobs/id.rs` makes them; this only checks them).
///
/// **Parsed, never trusted.** An id arrives from an agent's call and from a
/// directory listing, and becomes a path ([`AccountPaths::job_dir`]): the
/// pattern admits no separator and no dot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(String);

impl JobId {
    /// The letters Crockford's base32 is written in: the digits and the
    /// upper-case letters but I, L, O and U.
    pub const ALPHABET: &'static [u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

    /// `^j[0-9A-HJKMNP-TV-Z]{26}$`, and nothing else.
    pub fn parse(text: &str) -> Option<Self> {
        let rest = text.strip_prefix('j')?;
        (rest.len() == 26 && rest.bytes().all(|b| Self::ALPHABET.contains(&b)))
            .then(|| Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether somebody holds the lock on `path`, asked **without writing
/// anything** (`snob status` and `--dry-run` promise that): `None` when
/// there is no file. Opens what is there for reading, tries the lock and
/// lets it go at once.
pub fn lock_is_held(path: &Path) -> std::io::Result<Option<bool>> {
    let file = match std::fs::OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(false)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(Some(true)),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// The folder name every path here is built from, so that `snob purge`
/// compares against the value `ProjectDirs` was given, not a copy of it.
pub fn app_dir_name() -> &'static str {
    APPLICATION
}

/// Whether a directory is plausible as one of ours, and so may be deleted
/// whole: a `ProjectDirs` that resolved oddly (an empty `HOME`) must not turn
/// `purge` into something far worse. Anything this tool creates sits at
/// least two levels below the root and is never the home directory.
pub fn is_safe_to_remove(dir: &Path) -> bool {
    // Rejects `/`, `C:\`, `/data` and `C:\data` alike.
    if dir.parent().is_none_or(|parent| parent.parent().is_none()) {
        return false;
    }

    match directories::BaseDirs::new() {
        Some(base) => dir != base.home_dir(),
        None => true,
    }
}

/// Where a file that should not outlive the session goes: `$XDG_RUNTIME_DIR`
/// (owned by the user, `0700`, removed with the session, by specification)
/// when it is an absolute path that exists, else [`std::env::temp_dir`],
/// which is per-user on Windows and macOS.
fn runtime_dir() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime);
        if path.is_absolute() && path.is_dir() {
            return path;
        }
    }
    std::env::temp_dir()
}

/// Removes what earlier runs left in the scratch directory, by age: a
/// process id is reused, and nothing else cleans up after a killed process.
/// `older_than` is generous; every failure is ignored.
///
/// **It walks only a root that is this user's own.** The root sits at a
/// predictable name in a directory anybody can write to, and the sweep runs
/// before [`create_fresh_scratch_dir`] refuses a foreign one. On Unix the
/// root is opened without following a link and the descriptor is asked
/// whether it is a directory, the effective user's and closed to everybody
/// else ([`own_private_dir`]); on Linux the walk goes through
/// `/proc/self/fd`, so it is of the directory that was checked. Windows
/// keeps the link check alone: its temporary directory is the user's own.
pub fn sweep_old_scratch(root: &Path, older_than: std::time::Duration) {
    let Some(walk) = ScratchWalk::of(root) else {
        return; // nothing has ever run, which is the common case
    };
    let Ok(entries) = std::fs::read_dir(&walk.path) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        // `DirEntry::metadata` does not follow a link, and `remove_dir_all`
        // removes a link as itself, so an entry is never reached through.
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if now
            .duration_since(modified)
            .is_ok_and(|age| age > older_than)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The scratch root as [`sweep_old_scratch`] may walk it, or `None` when it
/// must not be walked at all.
struct ScratchWalk {
    /// The path the walk reads and removes through.
    path: PathBuf,
    /// Held open for the walk, so `/proc/self/fd/<n>` in `path` keeps naming
    /// the directory that was checked.
    #[cfg(unix)]
    _held: std::fs::File,
}

impl ScratchWalk {
    #[cfg(unix)]
    fn of(root: &Path) -> Option<Self> {
        let (held, found) = own_private_dir(root).ok()?;
        let path = through_descriptor(&held, &found).unwrap_or_else(|| root.to_path_buf());
        Some(Self { path, _held: held })
    }

    /// Never through a link at the root: `read_dir` follows one, and the root
    /// could be a link into somebody's home with everything in it older than
    /// the cutoff. A link is left alone here; creating the scratch afterwards
    /// removes it as a link, the same as at the leaf.
    #[cfg(not(unix))]
    fn of(root: &Path) -> Option<Self> {
        let found = std::fs::symlink_metadata(root).ok()?;
        found.is_dir().then(|| Self {
            path: root.to_path_buf(),
        })
    }
}

/// Opens `dir` without following a link at its name, and answers the
/// descriptor with what `fstat` read from it, if it is a real directory. A
/// name swapped afterwards does not move the check.
#[cfg(unix)]
fn open_dir(dir: &Path) -> std::io::Result<(std::fs::File, std::fs::Metadata)> {
    use std::os::unix::fs::OpenOptionsExt as _;

    let held = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(dir)?;
    let found = held.metadata()?;
    if !found.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "it is not a directory",
        ));
    }
    Ok((held, found))
}

/// An `Err` of kind `PermissionDenied` unless what `found` describes is owned
/// by the user `me`.
#[cfg(unix)]
fn owned_by(found: &std::fs::Metadata, me: u32) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;

    if found.uid() != me {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "it belongs to another account",
        ));
    }
    Ok(())
}

/// The effective user of this process.
#[cfg(unix)]
fn effective_user() -> u32 {
    // SAFETY: `geteuid` takes no argument, touches no memory of ours and
    // cannot fail.
    unsafe { libc::geteuid() }
}

/// [`open_dir`], provided the directory is the effective user's own; an
/// `Err` of kind `PermissionDenied` for one that is somebody else's.
#[cfg(unix)]
fn own_dir(dir: &Path) -> std::io::Result<(std::fs::File, std::fs::Metadata)> {
    let (held, found) = open_dir(dir)?;
    owned_by(&found, effective_user())?;
    Ok((held, found))
}

/// [`own_dir`], and also closed to everybody else: no group or other bits,
/// which is what [`create_private_root`] leaves it. A root this user owns but
/// left open may hold entries somebody else put there, so it is not walked.
#[cfg(unix)]
fn own_private_dir(dir: &Path) -> std::io::Result<(std::fs::File, std::fs::Metadata)> {
    use std::os::unix::fs::MetadataExt as _;

    let (held, found) = own_dir(dir)?;
    if found.mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "it is open to other accounts",
        ));
    }
    Ok((held, found))
}

/// The path that reaches the directory `held` is open on, rather than
/// whatever is at its name now: `/proc/self/fd/<n>` is a link the kernel
/// resolves to the open directory itself. Taken only when what it reaches is
/// the same directory (`st_dev` and `st_ino`), so a `/proc` that is missing,
/// or mounted so as to hide it, falls back to the name.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn through_descriptor(held: &std::fs::File, found: &std::fs::Metadata) -> Option<PathBuf> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::MetadataExt as _;

    let path = PathBuf::from(format!("/proc/self/fd/{}", held.as_raw_fd()));
    let seen = std::fs::metadata(&path).ok()?;
    (seen.dev() == found.dev() && seen.ino() == found.ino()).then_some(path)
}

/// No `/proc` to go through: the walk goes by the name.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn through_descriptor(_held: &std::fs::File, _found: &std::fs::Metadata) -> Option<PathBuf> {
    None
}

/// Replaces `path` with `contents` by way of `temporary`, written whole
/// ([`write_new_private`]) and renamed over it, so a reader never sees half
/// a file. The error carries the path of the step that failed.
pub fn replace_private(
    path: &Path,
    temporary: &Path,
    contents: &[u8],
) -> Result<(), (PathBuf, std::io::Error)> {
    write_new_private(temporary, contents).map_err(|e| (temporary.to_path_buf(), e))?;
    std::fs::rename(temporary, path).map_err(|e| (path.to_path_buf(), e))
}

/// Opens `path`, creating it empty when it is not there, and takes an
/// exclusive lock on it that lasts until the returned file is dropped.
///
/// Bind the result to a name for as long as the lock has to hold: `let _ =`
/// drops the file, and the lock with it, at once.
pub fn lock_file(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

/// [`lock_file`] that does not wait: `None` when another holder has the lock,
/// in this process or another, and the lock otherwise, held until the file is
/// dropped.
pub fn try_lock_file(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Creates a file only its owner can read, at exactly this name, synced to
/// disk before returning.
///
/// - What a failed run left is removed first, so `create_new` works at a
///   fixed name.
/// - `create_new`, never `create`: an existing file keeps the permissions it
///   had, and a link planted while the directory was lax would be followed.
/// - On Unix the mode is set **at creation**: a later chmod leaves a window.
/// - `sync_all`, or a rename could put a file in place with its contents
///   still in the page cache.
fn write_new_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let _ = std::fs::remove_file(path);
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Creates the directory and restricts it to its owner: `0700` on Unix; on
/// Windows a protected DACL (inheritance stopped, not added to) with one
/// entry, for this user, since the ACL inherited from `%LOCALAPPDATA%`
/// limits nothing where the profile's is not the default. Administrators
/// and `SYSTEM` are not listed: they can take ownership anyway.
///
/// **A failure is an error, not a warning**: the database and the browser
/// profile are the follower history and a live session in the clear.
pub fn create_private_dir(dir: &Path) -> Result<(), PathError> {
    std::fs::create_dir_all(dir).map_err(|source| PathError::Create {
        path: dir.to_path_buf(),
        source,
    })?;
    restrict_to_this_account(dir)
}

/// Where the parent of a fresh directory sits ([`create_fresh_private_dir`],
/// [`create_fresh_scratch_dir`]), which is who could have made a directory at
/// its name before this run did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Root {
    /// The scratch root, at a predictable name in a temporary directory
    /// anybody can write to (`/tmp`, wherever `$XDG_RUNTIME_DIR` is unset):
    /// a directory found there must be this account's own.
    Shared,
    /// A directory under the data directory, which only this account can
    /// enter, or the data directory itself. **A link found at such a name is
    /// the person's own** (a data directory moved to another disk), so it is
    /// followed and what it reaches made private. The owner is not asked: a
    /// filesystem may report a wrong one (NFSv4 with broken id mapping, sshfs
    /// without `idmap=user`) while letting this account change the mode.
    Private,
}

/// Creates and restricts the parent of a fresh directory: a
/// [`Root::Private`] name with [`create_private_dir`], and the
/// [`Root::Shared`] name, which somebody else can predict, without ever
/// following a link.
///
/// The shared name is the scratch root, which [`sweep_old_scratch`] walks
/// with `remove_dir_all`: `create_dir_all` answers `Ok` over a planted link,
/// and the `0700` would land on whatever it points at. A link or a stray
/// file there is removed as itself; a real directory is kept (it holds other
/// runs' scratch), and refused when it is somebody else's.
///
/// On Unix the `0700` goes on through a descriptor opened without following
/// a link ([`open_dir`]), never through the name again, and the descriptor
/// is asked who owns it, so a planted directory is refused even for root.
fn create_private_root(dir: &Path, root: Root) -> Result<(), PathError> {
    if root == Root::Private {
        return create_private_dir(dir);
    }
    #[cfg(unix)]
    {
        create_shared_root_as(dir, effective_user())
    }
    #[cfg(not(unix))]
    {
        // Windows' temporary directory is the user's own, so nobody else
        // could have made the scratch root there either; a link or a
        // junction found at its name is still taken away, not followed.
        create_root_entry(dir)?;
        restrict_to_this_account(dir)
    }
}

/// The first half of [`create_private_root`] at the [`Root::Shared`] name: a
/// real directory at the name, whatever was there before it. **Never for a
/// [`Root::Private`] name**, where what it removes is the person's own link.
fn create_root_entry(dir: &Path) -> Result<(), PathError> {
    let create = |source| PathError::Create {
        path: dir.to_path_buf(),
        source,
    };
    if let Err(e) = std::fs::create_dir(dir) {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(create(e));
        }
        // `symlink_metadata` does not follow, so a link is seen as a link.
        let found = std::fs::symlink_metadata(dir).map_err(create)?;
        if !found.is_dir() {
            remove_entry(dir, &found).map_err(create)?;
            std::fs::create_dir(dir).map_err(create)?;
        }
    }
    Ok(())
}

/// [`create_private_root`] at the [`Root::Shared`] name for the user `me`,
/// which a test can make somebody the filesystem does not report as the
/// owner.
#[cfg(unix)]
fn create_shared_root_as(dir: &Path, me: u32) -> Result<(), PathError> {
    use std::os::unix::fs::PermissionsExt as _;

    create_root_entry(dir)?;
    let not_private = |e: std::io::Error| PathError::NotPrivate {
        path: dir.to_path_buf(),
        detail: e.to_string(),
    };
    let (held, found) = open_dir(dir).map_err(not_private)?;
    owned_by(&found, me).map_err(not_private)?;
    held.set_permissions(std::fs::Permissions::from_mode(0o700))
        .map_err(not_private)
}

/// Creates a private directory that **did not exist a moment ago**, under a
/// parent in the data directory that is made private first. For a name
/// somebody else can predict, such as one with the process id:
/// [`create_private_dir`] would adopt whatever is at the path, a link
/// included. The scratch, outside the data directory, is
/// [`create_fresh_scratch_dir`].
pub fn create_fresh_private_dir(dir: &Path) -> Result<(), PathError> {
    create_fresh(dir, Root::Private)
}

/// [`create_fresh_private_dir`] for the scratch, whose root sits at a
/// predictable name in a temporary directory anybody can write to: a
/// `run-<pid>` planted there as a link would choose where a run writes every
/// story it fetches.
///
/// The parent is created and restricted first ([`create_private_root`],
/// [`Root::Shared`]), and the leaf is made with `create_dir`, which fails
/// rather than adopts. What is already there (an earlier run's directory
/// under a reused process id) is removed, a link as the link itself, and the
/// creation tried once more.
pub fn create_fresh_scratch_dir(dir: &Path) -> Result<(), PathError> {
    create_fresh(dir, Root::Shared)
}

/// What [`create_fresh_private_dir`] and [`create_fresh_scratch_dir`] share.
fn create_fresh(dir: &Path, root: Root) -> Result<(), PathError> {
    let create = |source| PathError::Create {
        path: dir.to_path_buf(),
        source,
    };
    if let Some(parent) = dir.parent() {
        create_private_root(parent, root)?;
    }
    if let Err(e) = std::fs::create_dir(dir) {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(create(e));
        }
        remove_tree(dir).map_err(create)?;
        std::fs::create_dir(dir).map_err(create)?;
    }
    restrict_to_this_account(dir)
}

/// Removes a directory tree, or a link found at the name as the link itself:
/// `purge` reaches the scratch root in the shared temporary directory, and
/// what a planted link points at is left alone.
pub fn remove_tree(dir: &Path) -> std::io::Result<()> {
    let found = std::fs::symlink_metadata(dir)?;
    if found.is_dir() {
        std::fs::remove_dir_all(dir)
    } else {
        remove_entry(dir, &found)
    }
}

/// Removes an entry already judged not to be a real directory, as itself, by
/// the metadata the caller read: whatever appears at the name in between, no
/// directory's contents are removed.
fn remove_entry(dir: &Path, found: &std::fs::Metadata) -> std::io::Result<()> {
    if is_link_to_a_directory(found) {
        std::fs::remove_dir(dir)
    } else {
        std::fs::remove_file(dir)
    }
}

/// [`remove_tree`], tried a few times over a few seconds; a tree already
/// gone is removed. For a browser profile: Windows keeps a directory while a
/// handle inside it is open.
pub fn remove_tree_patiently(dir: &Path) -> std::io::Result<()> {
    patiently(|| match remove_tree(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        done => done,
    })
}

/// A rename, tried the way [`remove_tree_patiently`] tries a removal.
pub fn rename_patiently(from: &Path, to: &Path) -> std::io::Result<()> {
    patiently(|| std::fs::rename(from, to))
}

/// Ten attempts over about five seconds. Blocks the thread, so a caller on an
/// async runtime whose workers are all needed runs it off them. Nothing is
/// retried once the name is not there: that does not wait itself away.
fn patiently(mut attempt: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut tries = 0u64;
    loop {
        match attempt() {
            Ok(()) => return Ok(()),
            Err(e) if tries == 9 || e.kind() == std::io::ErrorKind::NotFound => return Err(e),
            Err(_) => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100 * tries));
            }
        }
    }
}

/// Whether a directory entry is a link whose target is a directory, which
/// only Windows tells apart: it goes with `RemoveDirectory`, and `DeleteFile`
/// answers `ERROR_ACCESS_DENIED` on it.
fn is_link_to_a_directory(found: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt as _;
        found.file_type().is_symlink_dir()
    }
    #[cfg(not(windows))]
    {
        let _ = found;
        false
    }
}

/// The half of [`create_private_dir`] that makes the directory private.
fn restrict_to_this_account(dir: &Path) -> Result<(), PathError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(dir, perms).map_err(|source| PathError::Create {
            path: dir.to_path_buf(),
            source,
        })?;
    }

    #[cfg(windows)]
    windows_acl::restrict_to_owner(dir)?;
    #[cfg(windows)]
    keep_out_of_the_search_index(dir);

    Ok(())
}

/// Marks a directory so that Windows Search does not index what is inside:
/// a query against the live index returned files out of the browser profile.
/// `FILE_ATTRIBUTE_NOT_CONTENT_INDEXED` is inherited by files created inside
/// afterwards, so it is set once, at creation. Failure is ignored: this is
/// defense in depth on top of the ACL.
#[cfg(windows)]
fn keep_out_of_the_search_index(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;

    let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    wide.push(0);

    // SAFETY: `SetFileAttributesW` reads a null-terminated wide string and
    // returns a boolean. The buffer above is null-terminated and outlives the
    // call, and the attribute value is a documented constant. Microsoft
    // documents that it does not clear other attributes when the value is
    // combined, so the directory bit is preserved by reading first.
    unsafe {
        let existing = windows_sys::Win32::Storage::FileSystem::GetFileAttributesW(wide.as_ptr());
        if existing == windows_sys::Win32::Storage::FileSystem::INVALID_FILE_ATTRIBUTES {
            return;
        }
        windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(
            wide.as_ptr(),
            existing | windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NOT_CONTENT_INDEXED,
        );
    }
}

/// Giving a directory a DACL that names only the user running this process:
/// protected, with exactly one entry, which
/// `the_data_directory_is_not_readable_by_other_accounts` reads back. The
/// user and the ACL are [`crate::windows_user`]'s.
#[cfg(windows)]
mod windows_acl {
    use std::collections::BTreeSet;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, CONTAINER_INHERIT_ACE,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
        GetSecurityDescriptorControl, OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED, SECURITY_DESCRIPTOR_CONTROL,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    use super::PathError;
    use crate::windows_user::{OneUserAcl, User};

    /// What the entry hands on: to the directories and the files inside,
    /// because the point is the database and the browser profile rather than
    /// the folder itself.
    const INHERITANCE: u32 = CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE;

    /// `ACCESS_ALLOWED_ACE_TYPE`, as an entry's header spells it.
    const ACCESS_ALLOWED: u8 = 0;

    /// The directories this process has already given the DACL.
    ///
    /// `SetNamedSecurityInfoW` hands the entry on to everything already
    /// inside, whether or not anything changes: about 0.75 s for a tree of
    /// 5,000 files (Windows 11, 2026-11-14), and one command asks for the
    /// data directory several times. So that walk is paid once a process for
    /// each directory, and a later call only reads the DACL back
    /// ([`is_owners_alone`]): a directory removed and made again since
    /// inherits its parent's, is not read as private, and is written again.
    static RESTRICTED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

    fn failed(dir: &Path, what: &str, error: std::io::Error) -> PathError {
        PathError::NotPrivate {
            path: dir.to_path_buf(),
            detail: format!("{what} failed: {error}"),
        }
    }

    pub(super) fn restrict_to_owner(dir: &Path) -> Result<(), PathError> {
        let user = User::this_process().map_err(|e| failed(dir, "reading this user", e))?;
        let mut wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let restricted = || RESTRICTED.lock().unwrap_or_else(|e| e.into_inner());
        if restricted().contains(dir) && is_owners_alone(&mut wide, &user) {
            return Ok(());
        }

        let mut acl =
            OneUserAcl::new(&user, INHERITANCE).map_err(|e| failed(dir, "building the ACL", e))?;

        // `PROTECTED_DACL_SECURITY_INFORMATION` is the half that matters:
        // without it the inherited entries stay.
        //
        // SAFETY: a null-terminated path, and an ACL that outlives the call.
        let set = unsafe {
            SetNamedSecurityInfoW(
                wide.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl.as_ptr(),
                std::ptr::null_mut(),
            )
        };
        if set != 0 {
            return Err(PathError::NotPrivate {
                path: dir.to_path_buf(),
                detail: format!("SetNamedSecurityInfo failed: Windows error {set}"),
            });
        }
        restricted().insert(dir.to_path_buf());
        Ok(())
    }

    /// Whether the DACL of the directory `wide` names is the one
    /// [`restrict_to_owner`] writes: protected, and one entry of its own that
    /// allows `user` everything and is handed on to what is inside. `false`
    /// for anything else and for whatever cannot be read, which is then
    /// written again.
    fn is_owners_alone(wide: &mut [u16], user: &User) -> bool {
        let mut acl: *mut ACL = std::ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: a null-terminated path and out-parameters; the descriptor,
        // which `acl` points into, is freed below.
        let read = unsafe {
            GetNamedSecurityInfoW(
                wide.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if read != 0 {
            return false;
        }

        let mut control: SECURITY_DESCRIPTOR_CONTROL = 0;
        let mut revision: u32 = 0;
        let mut sizes = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        let mut entry: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `descriptor` is the one just read and `acl`, checked not to
        // be null (no DACL at all lets everybody in), points into it; both
        // live until the `LocalFree` below. The out-parameters are locals of
        // the types asked for. Entry 0 is asked for only of an ACL that says
        // it has one, and is read as an allowed entry (its header, the mask,
        // then the SID) only once its header says it is one.
        let ours = unsafe {
            !acl.is_null()
                && GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) != 0
                && control & SE_DACL_PROTECTED != 0
                && GetAclInformation(
                    acl,
                    (&raw mut sizes).cast(),
                    std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                ) != 0
                && sizes.AceCount == 1
                && GetAce(acl, 0, &mut entry) != 0
                && {
                    let allowed = entry.cast::<ACCESS_ALLOWED_ACE>();
                    (*allowed).Header.AceType == ACCESS_ALLOWED
                        && u32::from((*allowed).Header.AceFlags) == INHERITANCE
                        && (*allowed).Mask == FILE_ALL_ACCESS
                        && EqualSid((&raw mut (*allowed).SidStart).cast(), user.sid()) != 0
                }
        };
        // SAFETY: the descriptor the read produced, freed once.
        unsafe { LocalFree(descriptor) };
        ours
    }

    /// [`is_owners_alone`] for this process's user, by the directory's path.
    #[cfg(test)]
    pub(super) fn reads_as_private(dir: &Path) -> bool {
        let mut wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let user = User::this_process().expect("this process has a token");
        is_owners_alone(&mut wide, &user)
    }

    /// What the directory's DACL actually says, for the test that reads it
    /// back. Returns whether the DACL is protected, and every SID in it.
    #[cfg(test)]
    pub(super) fn describe(dir: &Path) -> (bool, Vec<String>) {
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows_sys::Win32::Security::{ACE_HEADER, PSID};

        let mut wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut acl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();

        // SAFETY: a null-terminated path and out-parameters; the descriptor is
        // freed below.
        let read = unsafe {
            GetNamedSecurityInfoW(
                wide.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(read, 0, "GetNamedSecurityInfo failed");

        let mut control: SECURITY_DESCRIPTOR_CONTROL = 0;
        let mut revision: u32 = 0;
        // SAFETY: a descriptor the call above produced.
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) };
        let protected = control & SE_DACL_PROTECTED != 0;

        // SAFETY: an out parameter of three integers, which `GetAclInformation`
        // fills below; all-zero is a valid starting value for every one of them.
        let mut sizes: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the ACL points into the descriptor, which is still alive.
        unsafe {
            GetAclInformation(
                acl,
                &mut sizes as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        };

        let mut sids = Vec::new();
        for index in 0..sizes.AceCount {
            let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
            // SAFETY: index is below the count the ACL just reported.
            if unsafe { GetAce(acl, index, &mut ace) } == 0 {
                continue;
            }
            // Every ACE type this can produce puts its SID immediately after
            // the access mask, which is one `u32` past the header.
            //
            // SAFETY: the layout of an access-allowed ACE.
            let sid = unsafe {
                (ace as *const u8)
                    .add(std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>())
                    as PSID
            };
            let mut text: *mut u16 = std::ptr::null_mut();
            // SAFETY: a SID inside the descriptor, and an out-parameter freed
            // immediately after it is read.
            unsafe {
                if ConvertSidToStringSidW(sid, &mut text) != 0 {
                    let mut length = 0;
                    while *text.add(length) != 0 {
                        length += 1;
                    }
                    sids.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                        text, length,
                    )));
                    LocalFree(text as *mut std::ffi::c_void);
                }
            }
        }

        // SAFETY: the descriptor the read produced, freed once.
        unsafe { LocalFree(descriptor) };
        (protected, sids)
    }

    /// This process's user, as a string SID, so a test can compare.
    #[cfg(test)]
    pub(super) fn current_user_sid() -> String {
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;

        let user = crate::windows_user::User::this_process().expect("this process has a token");
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: a SID this function owns, and an out-parameter freed after
        // it is read.
        unsafe {
            assert_ne!(ConvertSidToStringSidW(user.sid(), &mut text), 0);
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            let out = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text as *mut std::ffi::c_void);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every session of an account directory from before the list is sent
    /// as, until its first login, which starts the list with its own. Once
    /// the account is signed out, none it had is, through any later login,
    /// and a login's own always is, whatever the clock said when it was
    /// made. By identity, never by order: a login made "before" the
    /// sign-out, as a clock set back makes one, is let in all the same.
    #[test]
    fn a_session_signed_out_of_stays_refused_and_a_login_is_always_let_in() {
        let root = tempfile::tempdir().unwrap();
        let account = AppPaths::rooted_at(root.path()).account(snob_core::Pk::new(42));
        let (walking, again, set_back) = (
            snob_core::Epoch::new(1_800_000_000),
            snob_core::Epoch::new(1_800_000_600),
            snob_core::Epoch::new(1_700_000_000),
        );

        // An account directory from before the list existed.
        create_private_dir(&account.dir()).unwrap();
        assert!(!account.refuses_session(walking), "never signed out");
        account.admit_session(again).unwrap();
        assert!(!account.refuses_session(again), "a login is let in");
        assert!(
            account.refuses_session(walking),
            "and the session it replaced is not, once it started the list"
        );

        account.sign_out().unwrap();
        assert!(account.refuses_session(walking));
        assert!(account.refuses_session(again));
        account.admit_session(again).unwrap();
        assert!(!account.refuses_session(again), "a login is let in");
        assert!(account.refuses_session(walking), "and only its session");
        account.admit_session(set_back).unwrap();
        assert!(
            !account.refuses_session(set_back),
            "a login stamped before the sign-out is let in all the same"
        );
        assert!(account.refuses_session(walking));
        account.sign_out().unwrap();
        assert!(account.refuses_session(again), "a sign-out takes them all");
    }

    /// With the account's directory gone, as `purge` leaves it, no session is
    /// sent as, and a sign-out makes nothing. Made again by anything but a
    /// login, the directory still sends none it had before; a login lets in
    /// only its own.
    #[test]
    fn a_session_purged_stays_refused_once_the_directory_is_made_again() {
        let root = tempfile::tempdir().unwrap();
        let account = AppPaths::rooted_at(root.path()).account(snob_core::Pk::new(42));
        let (walking, again) = (
            snob_core::Epoch::new(1_800_000_000),
            snob_core::Epoch::new(1_800_000_600),
        );
        create_private_dir(&account.dir()).unwrap();
        assert!(!account.refuses_session(walking));

        std::fs::remove_dir_all(account.dir()).unwrap();
        assert!(account.refuses_session(walking));
        account.sign_out().unwrap();
        assert!(!account.dir().exists(), "a sign-out makes no directory");

        account.ensure_dirs().unwrap();
        assert!(account.refuses_session(walking), "made again, not let in");
        account.ensure_dirs().unwrap();
        assert!(account.refuses_session(walking));
        account.admit_session(again).unwrap();
        assert!(!account.refuses_session(again));
        account.ensure_dirs().unwrap();
        assert!(
            !account.refuses_session(again),
            "a directory already there keeps its list"
        );
        assert!(account.refuses_session(walking));
    }

    /// A login let in while another process is making the account's
    /// directory, between the directory and its empty list, stays let in:
    /// the list it starts is the one the other process finds there, and an
    /// empty one started first is added to.
    #[test]
    fn a_login_let_in_as_the_directory_is_made_elsewhere_stays_let_in() {
        let root = tempfile::tempdir().unwrap();
        let account = AppPaths::rooted_at(root.path()).account(snob_core::Pk::new(42));
        let login = snob_core::Epoch::new(1_800_000_000);

        // The other process has made the directory, and not yet its list.
        create_private_dir(&account.dir()).unwrap();
        account.admit_session(login).unwrap();
        start_signed_out(&account.dir()).unwrap();
        assert!(!account.refuses_session(login));

        // The other process's list was there first.
        std::fs::remove_dir_all(account.dir()).unwrap();
        create_private_dir(&account.dir()).unwrap();
        start_signed_out(&account.dir()).unwrap();
        account.admit_session(login).unwrap();
        assert!(!account.refuses_session(login));
    }

    /// On a directory from before the list, a login that then fails leaves
    /// the session stored before it let in; with a list, or with nothing
    /// stored, only the login is.
    #[test]
    fn a_failed_login_on_an_old_directory_keeps_the_stored_session_let_in() {
        let root = tempfile::tempdir().unwrap();
        let account = AppPaths::rooted_at(root.path()).account(snob_core::Pk::new(42));
        let (stored, login) = (
            snob_core::Epoch::new(1_700_000_000),
            snob_core::Epoch::new(1_800_000_000),
        );

        // As 0.8.0 left it: the directory, and no list in it.
        create_private_dir(&account.dir()).unwrap();
        account.admit_login(login, Some(stored)).unwrap();
        assert!(!account.refuses_session(stored), "what worked still works");
        assert!(!account.refuses_session(login));

        // Signed out of since: the stored one stays refused.
        account.sign_out().unwrap();
        account.admit_login(login, Some(stored)).unwrap();
        assert!(account.refuses_session(stored));
        assert!(!account.refuses_session(login));

        // Made again after a purge: nothing from before is let in.
        std::fs::remove_dir_all(account.dir()).unwrap();
        account.admit_login(login, None).unwrap();
        assert!(account.refuses_session(stored));
        assert!(!account.refuses_session(login));
    }

    /// **A sign-out that comes while a login trims a full list is not
    /// undone by it.** The login has read the list and is about to move a
    /// trimmed copy over it; the sign-out waits for that move and then
    /// empties what it left, so every session the account had is refused,
    /// the login's own included, as a sign-out after it would leave them.
    /// Without the lock the sign-out empties the list first, and the move
    /// puts the sessions back.
    #[test]
    fn a_sign_out_while_a_login_trims_the_list_is_not_undone() {
        use std::sync::mpsc::channel;
        let root = tempfile::tempdir().unwrap();
        let account = AppPaths::rooted_at(root.path()).account(snob_core::Pk::new(42));
        account.ensure_dirs().unwrap();
        let login = |n: usize| snob_core::Epoch::new(1_800_000_000 + i64::try_from(n).unwrap());
        for n in 0..SIGNED_IN_KEPT {
            account.admit_session(login(n)).unwrap();
        }

        // The login that trims, held once it has read the full list.
        let (read, has_read) = channel();
        let (go_on, told_to_go_on) = channel::<()>();
        let trimming = std::thread::spawn({
            let account = account.clone();
            move || {
                PAUSE.set(Some(Box::new(move |seam| {
                    if seam == Seam::Read {
                        read.send(()).unwrap();
                        told_to_go_on.recv().unwrap();
                    }
                })));
                account.admit_session(login(SIGNED_IN_KEPT)).unwrap();
            }
        });
        has_read.recv().unwrap();

        let (locking, is_locking) = channel();
        let signing_out = std::thread::spawn({
            let account = account.clone();
            move || {
                PAUSE.set(Some(Box::new(move |seam| {
                    if seam == Seam::Locking {
                        locking.send(()).unwrap();
                    }
                })));
                account.sign_out().unwrap();
            }
        });
        is_locking.recv().unwrap();
        // Ample for a sign-out that does not wait to empty a 64-line file.
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            !signing_out.is_finished(),
            "the sign-out waits for the login that read the list"
        );

        go_on.send(()).unwrap();
        trimming.join().unwrap();
        signing_out.join().unwrap();
        for n in 0..=SIGNED_IN_KEPT {
            assert!(account.refuses_session(login(n)), "{n} was let back in");
        }
        account.admit_session(login(SIGNED_IN_KEPT + 1)).unwrap();
        assert!(!account.refuses_session(login(SIGNED_IN_KEPT + 1)));
    }

    /// The jobs directory is made only inside the account's directory, never
    /// with it: one `purge` removed stays gone.
    #[test]
    fn the_jobs_directory_never_makes_the_accounts_again() {
        let root = tempfile::tempdir().unwrap();
        let account = AppPaths::rooted_at(root.path()).account(snob_core::Pk::new(42));
        assert!(!account.create_jobs_dir().unwrap());
        assert!(!account.dir().exists());

        account.ensure_dirs().unwrap();
        assert!(account.create_jobs_dir().unwrap());
        assert!(account.create_jobs_dir().unwrap(), "there already");
        assert!(account.jobs_dir().is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(account.jobs_dir())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    /// The list keeps the latest [`SIGNED_IN_KEPT`] logins, and a line not
    /// finished yet, or not a number, names nobody.
    #[test]
    fn the_signed_in_list_keeps_the_latest_logins_and_only_whole_lines() {
        let root = tempfile::tempdir().unwrap();
        let account = AppPaths::rooted_at(root.path()).account(snob_core::Pk::new(42));
        account.ensure_dirs().unwrap();
        let login = |n: usize| snob_core::Epoch::new(1_800_000_000 + i64::try_from(n).unwrap());
        for n in 0..=SIGNED_IN_KEPT {
            account.admit_session(login(n)).unwrap();
        }
        account.admit_session(login(SIGNED_IN_KEPT)).unwrap();
        assert!(account.refuses_session(login(0)), "the earliest went");
        for n in 1..=SIGNED_IN_KEPT {
            assert!(!account.refuses_session(login(n)), "{n}");
        }
        let kept = std::fs::read(account.signed_in()).unwrap();
        assert_eq!(sessions_in(&kept).len(), SIGNED_IN_KEPT);

        assert_eq!(sessions_in(b"1\n2\n3"), [1, 2]);
        assert_eq!(sessions_in(b"1\nnot one\n\n2\n"), [1, 2]);
        assert!(sessions_in(b"12").is_empty());
    }

    /// **A sandboxed run must not reach the real temporary directory.**
    /// The scratch directory is the one location that comes from the
    /// environment rather than from `directories`, so it is the one that would
    /// escape a root — and "every file this run touches is under one
    /// directory" is what `--sandbox-root` promises in `AGENTS.md`.
    #[test]
    fn a_rooted_run_keeps_its_scratch_under_the_root() {
        let paths = AppPaths::rooted_at("/tmp/test");
        assert!(paths.stories_root().starts_with("/tmp/test"));
        assert!(paths.story_scratch().starts_with(paths.stories_root()));
    }

    /// Two runs at once must not share a directory one of them will delete.
    #[test]
    fn each_run_gets_a_scratch_directory_of_its_own() {
        let paths = AppPaths::rooted_at("/tmp/test");
        let mine = paths.story_scratch();
        assert!(
            mine.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(&std::process::id().to_string())),
            "the run has to be named in the path: {}",
            mine.display()
        );
    }

    /// A lock somebody holds is refused at once rather than waited for, by a
    /// second handle in the same process as by another process, and is free
    /// again once its holder lets go.
    #[test]
    fn a_held_lock_is_refused_without_waiting_and_free_once_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("spend.lock");
        let held = try_lock_file(&path).unwrap().expect("nobody holds it yet");
        assert!(try_lock_file(&path).unwrap().is_none(), "it is held");
        drop(held);
        assert!(try_lock_file(&path).unwrap().is_some(), "it was let go");
    }

    /// An id becomes a path, so only the exact shape passes: no separator, no
    /// dot, no lower case after the `j`, none of the four letters Crockford
    /// leaves out, and exactly 26 after the prefix.
    #[test]
    fn a_job_id_is_refused_unless_it_is_ours() {
        let ours = "j01JCZ3X8K9Q2W4E5R6T7Y8V9A0";
        assert_eq!(ours.len(), 27);
        let id = JobId::parse(ours).expect("one of ours");
        assert_eq!(id.as_str(), ours);
        let paths = AppPaths::rooted_at("/tmp/test").account(snob_core::Pk::new(42));
        assert_eq!(paths.job_dir(&id), paths.jobs_dir().join(ours));
        for refused in [
            "",
            "j",
            "J01JCZ3X8K9Q2W4E5R6T7Y8V9A0",
            "j01jcz3x8k9q2w4e5r6t7y8v9a0",
            "j01JCZ3X8K9Q2W4E5R6T7Y8V9A",
            "j01JCZ3X8K9Q2W4E5R6T7Y8V9A00",
            "j01JCZ3X8K9Q2W4E5R6T7Y8V9AI",
            "j01JCZ3X8K9Q2W4E5R6T7Y8V9AU",
            "j../../../../../../../../..",
            "j01JCZ3X8K9Q2W4E5R6T7Y8V9/0",
            " j01JCZ3X8K9Q2W4E5R6T7Y8V9A0",
        ] {
            assert_eq!(JobId::parse(refused), None, "{refused:?}");
        }
    }

    /// Asking whether a job is running creates nothing: a missing lock file
    /// is "nobody", and a present one is held or not without being changed.
    #[test]
    fn liveness_is_read_without_creating_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lock");
        assert_eq!(lock_is_held(&path).unwrap(), None);
        assert!(!path.exists(), "asking made the file");

        let held = try_lock_file(&path).unwrap().expect("nobody holds it yet");
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(lock_is_held(&path).unwrap(), Some(true));
        drop(held);
        assert_eq!(lock_is_held(&path).unwrap(), Some(false));
        // And asking took nothing: the lock is still there to be had.
        assert!(try_lock_file(&path).unwrap().is_some());
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
    }

    /// The scratch root is on the list `purge` reads. It is the only thing this
    /// tool writes outside the data directory, so it is the only one that could
    /// be forgotten there.
    #[test]
    fn the_scratch_root_is_something_purge_removes() {
        let paths = AppPaths::rooted_at("/tmp/test");
        assert!(paths.owned_dirs().contains(&paths.stories_root()));
    }

    /// The sweep takes what is old and leaves what is not. Both halves matter:
    /// deleting a live run's directory takes the file out from under the viewer
    /// it was just handed to.
    #[test]
    fn the_sweep_takes_the_abandoned_and_leaves_the_living() {
        let tmp = tempfile::tempdir().unwrap();
        // Made the way every run makes it: the sweep walks no other kind.
        let root = tmp.path().join("snob-ig-stories");
        create_private_root(&root, Root::Shared).unwrap();
        let old = root.join("run-old");
        let fresh = root.join("run-fresh");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(old.join("story.jpg"), b"x").unwrap();

        // Zero, so everything already on disk counts as abandoned; then a
        // generous one, so nothing does. Testing it this way rather than by
        // waiting keeps the suite off the wall clock.
        sweep_old_scratch(&root, std::time::Duration::ZERO);
        assert!(!old.exists(), "an abandoned directory should have gone");

        std::fs::create_dir_all(&fresh).unwrap();
        sweep_old_scratch(&root, std::time::Duration::from_secs(3600));
        assert!(fresh.exists(), "a live directory must be left alone");
    }

    /// A root this account owns but left open to others may hold entries
    /// somebody else put there, and is not walked: what was planted in it is
    /// theirs to swap for a link between the look and the removal.
    #[cfg(unix)]
    #[test]
    fn the_sweep_walks_no_root_that_is_open_to_other_accounts() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("snob-ig-stories");
        let old = root.join("run-old");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();

        sweep_old_scratch(&root, std::time::Duration::ZERO);
        assert!(old.exists(), "a root open to others was walked");

        // Closed again, the same root is swept: the refusal was the mode.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        sweep_old_scratch(&root, std::time::Duration::ZERO);
        assert!(
            !old.exists(),
            "a private root of this account's was not swept"
        );
    }

    /// A link planted at the root's name is not walked, whatever it points
    /// at: this account's home has plenty in it older than the cutoff.
    #[cfg(unix)]
    #[test]
    fn the_sweep_never_walks_through_a_link_at_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(home.join("documents")).unwrap();
        let root = tmp.path().join("snob-ig-stories");
        std::os::unix::fs::symlink(&home, &root).unwrap();

        sweep_old_scratch(&root, std::time::Duration::ZERO);
        assert!(
            home.join("documents").exists(),
            "the sweep followed the link"
        );
    }

    /// On Linux the walk goes through the descriptor that was checked, so a
    /// name renamed away and replaced by a link after the check still walks
    /// the directory the check was about.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn the_sweep_walks_the_directory_it_checked_not_what_the_name_became() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("snob-ig-stories");
        create_private_root(&root, Root::Shared).unwrap();
        std::fs::create_dir(root.join("run-ours")).unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(home.join("documents")).unwrap();

        let walk = ScratchWalk::of(&root).expect("a private root of ours is walked");
        assert!(
            walk.path.starts_with("/proc/self/fd"),
            "{}",
            walk.path.display()
        );

        std::fs::rename(&root, tmp.path().join("moved")).unwrap();
        std::os::unix::fs::symlink(&home, &root).unwrap();
        let seen: Vec<_> = std::fs::read_dir(&walk.path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(seen, vec![std::ffi::OsString::from("run-ours")]);
    }

    /// A root that does not exist is not an error. It is the common case: the
    /// first run of `snob stories` on a machine.
    #[test]
    fn sweeping_a_root_nothing_has_created_is_quiet() {
        sweep_old_scratch(
            Path::new("/nonexistent-snob-scratch"),
            std::time::Duration::ZERO,
        );
    }

    #[test]
    fn files_hang_off_their_directories() {
        let paths = AppPaths::rooted_at("/tmp/test");
        assert!(paths.browser_profile().starts_with(paths.data_dir()));
        let account = paths.browser_profile_for(snob_core::Pk::new(42));
        assert_eq!(account.parent(), Some(paths.browser_profile().as_path()));
        assert!(paths.browser_hints_file().starts_with(paths.data_dir()));
        for global in [paths.registry_file(), paths.shared_db_file()] {
            assert_eq!(global.parent(), Some(paths.data_dir()));
        }
        assert!(paths.accounts_dir().starts_with(paths.data_dir()));
        assert_eq!(
            paths.unclaimed_dir().parent(),
            Some(paths.accounts_dir().as_path())
        );
    }

    /// An account's files are its own: under a directory named after its id,
    /// and nowhere another account's are.
    #[test]
    fn an_accounts_files_are_under_its_own_directory() {
        let paths = AppPaths::rooted_at("/tmp/test");
        let a = paths.account(snob_core::Pk::new(42));
        let b = paths.account(snob_core::Pk::new(43));

        assert_eq!(a.pk(), snob_core::Pk::new(42));
        assert_eq!(a.dir(), paths.accounts_dir().join("42"));
        for file in [a.db_file(), a.session_file()] {
            assert_eq!(file.parent(), Some(a.dir().as_path()));
            assert!(!file.starts_with(b.dir()));
        }
        assert_eq!(
            a.browser_profile(),
            paths.browser_profile_for(snob_core::Pk::new(42))
        );
        assert_ne!(a.db_file(), paths.legacy_db_file());
        assert_ne!(a.session_file(), paths.legacy_session_file());
        // What is not the account's is the machine's, through the deref.
        assert_eq!(a.data_dir(), paths.data_dir());
    }

    /// Both directories are created, and the account's is private too.
    #[test]
    fn an_account_creates_its_own_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let account = AppPaths::rooted_at(tmp.path()).account(snob_core::Pk::new(42));
        account.ensure_dirs().unwrap();
        assert!(account.data_dir().is_dir());
        assert!(account.dir().is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(account.dir())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    /// Only what gets written to, for the reason [`AppPaths::ensure_dirs`]
    /// gives: somebody who never runs the monitor gets no empty folder in
    /// their roaming profile.
    #[test]
    fn only_the_data_directory_is_created() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        paths.ensure_dirs().unwrap();

        assert!(paths.data_dir().is_dir());
        assert!(
            !paths.config_dir().exists(),
            "the directory belongs to whatever writes configuration, not to every run"
        );
    }

    /// The data directory is limited to this account, and says so to the
    /// operating system rather than in a comment.
    ///
    /// Two things are read back, and the first is the one that is easy to lose:
    /// the DACL has to be **protected**, because a DACL set without that flag
    /// keeps every inherited entry and the exposure is exactly those entries.
    /// The second is that the only account named is this one.
    #[cfg(windows)]
    #[test]
    fn the_data_directory_is_not_readable_by_other_accounts() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        paths.ensure_dirs().unwrap();

        let (protected, sids) = windows_acl::describe(paths.data_dir());
        assert!(
            protected,
            "the DACL is not protected, so whatever the profile grants still applies"
        );
        assert_eq!(
            sids,
            vec![windows_acl::current_user_sid()],
            "somebody other than this account is named in the directory's DACL"
        );
    }

    /// The DACL is written once a process for each directory and only read
    /// back after that. A directory removed and made again at the same name
    /// inherits whatever its parent grants: the read has to see that it is
    /// not the one written, or the skip would leave it open.
    #[cfg(windows)]
    #[test]
    fn a_directory_made_again_is_restricted_again() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");

        create_private_dir(&dir).unwrap();
        assert!(
            windows_acl::reads_as_private(&dir),
            "the DACL just written is not read back as the one written, so every call \
             would write it again"
        );

        std::fs::remove_dir(&dir).unwrap();
        std::fs::create_dir(&dir).unwrap();
        assert!(
            !windows_acl::reads_as_private(&dir),
            "a directory with the DACL it inherited reads as private"
        );

        create_private_dir(&dir).unwrap();
        let (protected, sids) = windows_acl::describe(&dir);
        assert!(protected, "the directory made again kept what it inherited");
        assert_eq!(sids, vec![windows_acl::current_user_sid()]);
    }

    /// Where the data goes must not depend on the directory the command was
    /// run from: it is the user's data, not the folder's.
    #[test]
    fn the_paths_do_not_depend_on_the_working_directory() {
        let Ok(paths) = AppPaths::discover() else {
            return; // no HOME in the test environment
        };
        assert!(
            paths
                .account(snob_core::Pk::new(42))
                .db_file()
                .is_absolute(),
            "a relative path would follow whoever ran the command around"
        );
        assert!(paths.data_dir().is_absolute());
    }

    /// A scratch directory with a guessable name is never adopted.
    ///
    /// The planted entry is a symbolic link to a directory of the victim's
    /// own. `create_dir_all` answers `Ok` on it and the permissions land on the
    /// target, and every story fetched afterwards would be written wherever the
    /// link points. The fresh variant removes the link as a link -- the target
    /// is untouched -- and makes a real directory.
    #[test]
    fn a_planted_link_under_the_scratch_name_is_replaced_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("theirs.txt"), b"untouched").unwrap();

        let root = tmp.path().join("snob-ig-stories");
        std::fs::create_dir(&root).unwrap();
        let leaf = root.join("run-1234");
        #[cfg(unix)]
        let planted = std::os::unix::fs::symlink(&elsewhere, &leaf).is_ok();
        // A link to a directory needs a privilege on Windows that a test
        // runner does not always have; without it the case is a leftover
        // directory, which the second half covers.
        #[cfg(windows)]
        let planted = std::os::windows::fs::symlink_dir(&elsewhere, &leaf).is_ok();

        create_fresh_scratch_dir(&leaf).unwrap();

        let made = std::fs::symlink_metadata(&leaf).unwrap();
        assert!(made.is_dir() && !made.is_symlink(), "a link was adopted");
        assert!(
            elsewhere.join("theirs.txt").exists(),
            "removing the link must not reach through it"
        );
        let _ = planted;

        // An earlier run's directory under a reused process id is replaced,
        // and what it held does not survive into the new run.
        std::fs::write(leaf.join("stale.jpg"), b"x").unwrap();
        create_fresh_scratch_dir(&leaf).unwrap();
        assert!(!leaf.join("stale.jpg").exists());
        assert!(leaf.is_dir());
    }

    /// The parent is made private before the leaf, so on a first run nobody
    /// else can put an entry inside it.
    #[cfg(unix)]
    #[test]
    fn the_scratch_root_is_private_too() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let leaf = tmp.path().join("snob-ig-stories").join("run-1");
        create_fresh_scratch_dir(&leaf).unwrap();
        for dir in [leaf.parent().unwrap(), leaf.as_path()] {
            let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", dir.display());
        }
    }

    /// A filesystem may report an owner other than the account that can
    /// change the mode there. Under the data directory, which nobody else can
    /// enter, the parent is not asked who owns it, so a login is not refused
    /// on such a home; the scratch root, at a name anybody could take first,
    /// still is.
    #[cfg(unix)]
    #[test]
    fn only_the_scratch_root_is_refused_for_an_owner_that_is_not_this_account() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let somebody_else = effective_user().wrapping_add(1);

        let profiles = tmp.path().join("browser-profile");
        create_private_root(&profiles, Root::Private).unwrap();
        let mode = std::fs::metadata(&profiles).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);

        let scratch = tmp.path().join("snob-ig-stories");
        std::fs::create_dir(&scratch).unwrap();
        std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o755)).unwrap();
        let refused = create_shared_root_as(&scratch, somebody_else);
        assert!(
            matches!(refused, Err(PathError::NotPrivate { .. })),
            "{refused:?}"
        );
        let mode = std::fs::metadata(&scratch).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "a refused root was made private");

        // As the account it is, the same root is made private.
        create_shared_root_as(&scratch, effective_user()).unwrap();
        let mode = std::fs::metadata(&scratch).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    /// A data directory (or a directory of browser profiles) the person
    /// moved to another disk and linked back is theirs: a fresh directory
    /// under it is made through the link, which is still there afterwards,
    /// and what it reaches is made private. Removing it would leave every
    /// later run an empty directory, with no sessions and no databases.
    #[cfg(unix)]
    #[test]
    fn a_linked_data_directory_is_kept_and_a_fresh_directory_made_through_it() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let moved = tmp.path().join("big-disk").join("snob-ig");
        std::fs::create_dir_all(&moved).unwrap();
        std::fs::set_permissions(&moved, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(moved.join("accounts.toml"), b"kept").unwrap();
        let data = tmp.path().join("snob-ig");
        std::os::unix::fs::symlink(&moved, &data).unwrap();

        let probe = data.join("browser-probe-1-0");
        create_fresh_private_dir(&probe).unwrap();

        let found = std::fs::symlink_metadata(&data).unwrap();
        assert!(found.is_symlink(), "the person's link was taken away");
        assert_eq!(std::fs::read(moved.join("accounts.toml")).unwrap(), b"kept");
        assert!(
            moved.join("browser-probe-1-0").is_dir(),
            "not made through the link"
        );
        let mode = std::fs::metadata(&moved).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "what the link reaches was left open");
    }

    /// A failed replacement names the step that failed: the temporary when it
    /// could not be written, the file itself when the rename could not land.
    #[test]
    fn a_failed_replacement_names_the_path_that_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file.toml");
        let temporary = tmp.path().join("file.toml.new");

        std::fs::create_dir(&temporary).unwrap();
        let (failed, _) = replace_private(&path, &temporary, b"x").unwrap_err();
        assert_eq!(failed, temporary);
        std::fs::remove_dir(&temporary).unwrap();

        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("inside"), b"x").unwrap();
        let (failed, _) = replace_private(&path, &temporary, b"x").unwrap_err();
        assert_eq!(failed, path);
    }

    /// A link or a stray file planted at the scratch root's name is taken as
    /// itself and replaced by a private directory; what a link pointed at is
    /// left alone.
    #[cfg(unix)]
    #[test]
    fn a_planted_entry_at_the_scratch_root_is_replaced_not_followed() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o755)).unwrap();

        let root = tmp.path().join("snob-ig-stories");
        std::os::unix::fs::symlink(&elsewhere, &root).unwrap();
        create_fresh_scratch_dir(&root.join("run-1")).unwrap();
        let made = std::fs::symlink_metadata(&root).unwrap();
        assert!(made.is_dir() && !made.is_symlink(), "the link was followed");
        assert_eq!(made.permissions().mode() & 0o777, 0o700);
        let theirs = std::fs::metadata(&elsewhere).unwrap().permissions().mode();
        assert_eq!(theirs & 0o777, 0o755, "the link's target was made private");
        assert!(!elsewhere.join("run-1").exists());

        std::fs::remove_dir_all(&root).unwrap();
        std::fs::write(&root, b"stray").unwrap();
        create_fresh_scratch_dir(&root.join("run-1")).unwrap();
        let made = std::fs::symlink_metadata(&root).unwrap();
        assert!(made.is_dir(), "the stray file is still there");
        assert_eq!(made.permissions().mode() & 0o777, 0o700);
    }

    /// Everything `purge` has to take away has to be under something this
    /// returns, or the command quietly stops leaving nothing behind.
    #[test]
    fn every_directory_written_to_is_on_the_purge_list() {
        let paths = AppPaths::rooted_at("/tmp/test");
        let owned = paths.owned_dirs();
        let account = paths.account(snob_core::Pk::new(42));

        for path in [
            account.db_file(),
            account.session_file(),
            account.browser_profile(),
            paths.registry_file(),
            paths.shared_db_file(),
            paths.unclaimed_dir(),
            paths.legacy_db_file(),
            paths.legacy_session_file(),
            paths.browser_profile(),
            paths.browser_hints_file(),
            // The configuration file, which lives outside the data directory.
            crate::config::path(&paths),
        ] {
            assert!(
                owned.iter().any(|dir| path.starts_with(dir)),
                "{} is under no directory purge would remove",
                path.display()
            );
        }
    }

    /// On macOS the configuration and data directories are one path. Listed
    /// twice, the second removal fails on what the first already took.
    #[test]
    fn the_purge_list_has_no_repeats() {
        let Ok(paths) = AppPaths::discover() else {
            return; // no HOME in the test environment
        };
        let owned = paths.owned_dirs();
        let mut unique = owned.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), owned.len(), "{owned:?}");
    }

    /// The real directories have to survive the guard, or purge removes
    /// nothing at all — a failure that would look exactly like success.
    #[test]
    fn the_real_directories_are_removable() {
        let Ok(paths) = AppPaths::discover() else {
            return; // no HOME in the test environment
        };
        for dir in paths.owned_dirs() {
            assert!(is_safe_to_remove(&dir), "{}", dir.display());
        }
    }

    #[test]
    fn nothing_near_the_root_is_removable() {
        for dangerous in ["/", "/home", "/data", "C:\\", "C:\\Users"] {
            assert!(
                !is_safe_to_remove(Path::new(dangerous)),
                "{dangerous} should never be removable"
            );
        }
    }

    /// A `ProjectDirs` that resolved to the home directory itself would take
    /// everything the user owns with it.
    #[test]
    fn the_home_directory_is_not_removable() {
        let Some(base) = directories::BaseDirs::new() else {
            return;
        };
        assert!(!is_safe_to_remove(base.home_dir()));
    }

    /// The database must never end up in a directory that syncs. On Windows
    /// that means Local rather than Roaming; elsewhere both coincide and the
    /// check is trivially true.
    #[test]
    fn data_goes_to_the_local_directory() {
        let Ok(paths) = AppPaths::discover() else {
            return; // no HOME in the test environment
        };
        let dirs = directories::ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION).unwrap();
        assert_eq!(paths.data_dir(), dirs.data_local_dir());
        #[cfg(windows)]
        assert!(!paths.accounts_dir().to_string_lossy().contains("Roaming"));
    }
}
