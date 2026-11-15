//! How much room what snob keeps takes, by kind and by account, and the one
//! kind that can be cleared: the cache (`snob_store::cache`).
//!
//! **Measuring opens nothing**: sizes come from metadata (`snob_store::disk`),
//! so a database is never opened, for writing or at all. **Clearing removes
//! only what can be fetched again**: never a session, a stored list or
//! capture, `watch.toml`, `accounts.toml`, a job or its result, a log, the
//! browser profiles (each holds a live session), a downloaded update (the
//! updater's state names it), nor anything saved for the person, which is
//! never in the data directory. Both block on the disk: run them off the
//! async workers.

use std::path::{Path, PathBuf};

use snob_core::Pk;
use snob_store::disk::{self, Size};
use snob_store::paths::AppPaths;

/// What a file is kept for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// Each account's lists, walks, budget and monitor history, `shared.db`
    /// and a database no login has claimed yet.
    Databases,
    /// What a view shows at once; the only kind [`clear_cache`] removes.
    Cache,
    /// The browser each account's requests leave from.
    BrowserProfiles,
    /// Jobs and their results.
    Jobs,
    /// An app's log and its crash reports.
    Logs,
    /// The updater's state and the version it downloaded.
    Update,
    /// Stories handed to the system viewer, in the system's temporary
    /// directory.
    Scratch,
    /// The rest: sessions kept in files, cards, locks, configuration.
    Other,
}

impl Kind {
    pub const ALL: [Self; 8] = [
        Self::Databases,
        Self::Cache,
        Self::BrowserProfiles,
        Self::Jobs,
        Self::Logs,
        Self::Update,
        Self::Scratch,
        Self::Other,
    ];

    /// Whether [`clear_cache`] removes this kind.
    pub fn clearable(self) -> bool {
        self == Self::Cache
    }
}

/// What each [`Kind`] weighs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kinds([Size; Kind::ALL.len()]);

impl Kinds {
    pub fn get(&self, kind: Kind) -> Size {
        self.0[kind as usize]
    }

    fn add(&mut self, kind: Kind, size: Size) {
        self.0[kind as usize] += size;
    }

    /// Every kind together.
    pub fn total(&self) -> Size {
        self.0
            .iter()
            .fold(Size::default(), |all, size| all.plus(*size))
    }

    /// Each kind with what it weighs, in [`Kind::ALL`]'s order.
    pub fn iter(&self) -> impl Iterator<Item = (Kind, Size)> + '_ {
        Kind::ALL.into_iter().map(|kind| (kind, self.get(kind)))
    }
}

/// What one account holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUsage {
    pub pk: Pk,
    pub kinds: Kinds,
}

/// What snob keeps on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    /// By kind, every account's included.
    pub kinds: Kinds,
    /// What of that each account holds, by pk: its databases, cache, jobs,
    /// browser profile and other files.
    pub accounts: Vec<AccountUsage>,
}

impl Usage {
    pub fn total(&self) -> Size {
        self.kinds.total()
    }

    fn add(&mut self, account: Option<Pk>, kind: Kind, size: Size) {
        self.kinds.add(kind, size);
        if let Some(pk) = account {
            let at = match self.accounts.binary_search_by_key(&pk, |a| a.pk) {
                Ok(at) => at,
                Err(at) => {
                    self.accounts.insert(
                        at,
                        AccountUsage {
                            pk,
                            kinds: Kinds::default(),
                        },
                    );
                    at
                }
            };
            self.accounts[at].kinds.add(kind, size);
        }
    }
}

/// Measures everything snob keeps: the data directory, the configuration and
/// the stories' scratch ([`AppPaths::owned_dirs`]). **Reads metadata only.**
pub fn measure(paths: &AppPaths) -> Usage {
    let mut usage = Usage::default();
    let data = paths.data_dir();
    let accounts = paths.accounts_dir();
    let browsers = paths.browser_profile();
    let logs = crate::log::dir(paths);
    let shared = paths.shared_db_file();
    let shared = shared.file_name().unwrap_or_default().to_string_lossy();
    for (path, name) in entries(data) {
        if path == accounts {
            measure_accounts(paths, &mut usage);
        } else if path == browsers {
            for (profile, name) in entries(&path) {
                usage.add(
                    name.parse().ok(),
                    Kind::BrowserProfiles,
                    disk::tree(&profile),
                );
            }
        } else if path == logs {
            usage.add(None, Kind::Logs, disk::tree(&path));
        } else if name.starts_with("update") {
            // `update/`, `update.json` and the updater's locks (`update::state`).
            usage.add(None, Kind::Update, disk::tree(&path));
        } else if name.starts_with(&*shared) || name.starts_with("snob.db") {
            // `snob.db`: the single-account layout's, before its migration.
            usage.add(None, Kind::Databases, disk::tree(&path));
        } else {
            usage.add(None, Kind::Other, disk::tree(&path));
        }
    }
    let stories = paths.stories_root();
    for dir in paths.owned_dirs() {
        if dir == stories {
            usage.add(None, Kind::Scratch, disk::tree(&dir));
        } else if !dir.starts_with(data) && !data.starts_with(&dir) {
            usage.add(None, Kind::Other, disk::tree(&dir));
        }
    }
    usage
}

fn measure_accounts(paths: &AppPaths, usage: &mut Usage) {
    for (dir, name) in entries(&paths.accounts_dir()) {
        let Ok(pk) = name.parse::<Pk>() else {
            let kind = if dir == paths.unclaimed_dir() {
                Kind::Databases
            } else {
                Kind::Other
            };
            usage.add(None, kind, disk::tree(&dir));
            continue;
        };
        let account = paths.account(pk);
        let jobs = account.jobs_dir();
        for (path, name) in entries(&dir) {
            let kind = if snob_store::cache::holds(&account, &path) {
                Kind::Cache
            } else if path == jobs {
                Kind::Jobs
            } else if name.starts_with("snob.db") {
                Kind::Databases
            } else {
                Kind::Other
            };
            usage.add(Some(pk), kind, disk::tree(&path));
        }
    }
}

/// The entries of `dir` with their names: none when it cannot be read.
fn entries(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .map(|entry| {
            (
                entry.path(),
                entry.file_name().to_string_lossy().into_owned(),
            )
        })
        .collect()
}

/// What [`clear_cache`] removed, and what refused to go.
#[derive(Debug, Default)]
pub struct Cleared {
    pub freed: Size,
    /// The first cache that could not be removed whole, and why.
    pub failure: Option<(PathBuf, std::io::Error)>,
}

/// Removes the cache of `only`, or with none of every account here: what a
/// view showed at once, which the requests it makes anyway fetch again.
/// **Nothing else**: see this module's header. A process still running for
/// an account may keep an entry again as its next answer arrives.
pub fn clear_cache(paths: &AppPaths, only: Option<Pk>) -> Cleared {
    let accounts = match only {
        Some(pk) => vec![pk],
        None => paths.account_dirs(),
    };
    let mut cleared = Cleared::default();
    for pk in accounts {
        let account = paths.account(pk);
        let before = snob_store::cache::size(&account);
        let removed = snob_store::cache::forget(&account);
        let after = snob_store::cache::size(&account);
        cleared.freed += Size {
            bytes: before.bytes.saturating_sub(after.bytes),
            files: before.files.saturating_sub(after.files),
        };
        if let Err(e) = removed {
            cleared.failure.get_or_insert((account.cache_dir(), e));
        }
    }
    cleared
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    /// Two accounts, and a file of every kind snob keeps.
    fn kept(root: &Path) -> AppPaths {
        let paths = AppPaths::rooted_at(root);
        for (pk, n) in [(42u64, 1usize), (43, 2)] {
            let account = paths.account(Pk::new(pk));
            write(&account.db_file(), 1000 * n);
            write(&account.dir().join("snob.db-wal"), 100 * n);
            write(&account.session_file(), 10 * n);
            write(&account.jobs_dir().join("one").join("result.json"), 20 * n);
            write(&account.cached_profile(Pk::new(7)), 30 * n);
            write(&account.cached_picture(Pk::new(7)), 40 * n);
            write(
                &paths
                    .browser_profile_for(Pk::new(pk))
                    .join("Default/Cookies"),
                500 * n,
            );
        }
        write(&paths.shared_db_file(), 7);
        write(&crate::log::dir(&paths).join("snob.log"), 9);
        write(
            &paths.data_dir().join("update").join("staged").join("snob"),
            11,
        );
        write(&paths.data_dir().join("update.json"), 2);
        write(&paths.registry_file(), 3);
        write(&paths.config_dir().join("watch.toml"), 5);
        write(&paths.stories_root().join("run-1").join("a.jpg"), 13);
        paths
    }

    #[test]
    fn everything_kept_is_counted_once_by_kind_and_by_account() {
        let root = tempfile::tempdir().unwrap();
        let paths = kept(root.path());

        let usage = measure(&paths);

        let bytes = |kind| usage.kinds.get(kind).bytes;
        assert_eq!(bytes(Kind::Databases), 1100 + 2200 + 7);
        assert_eq!(bytes(Kind::Cache), 70 + 140);
        assert_eq!(bytes(Kind::Jobs), 20 + 40);
        assert_eq!(bytes(Kind::BrowserProfiles), 500 + 1000);
        assert_eq!(bytes(Kind::Logs), 9);
        assert_eq!(bytes(Kind::Update), 13);
        assert_eq!(bytes(Kind::Scratch), 13);
        assert_eq!(bytes(Kind::Other), 10 + 20 + 3 + 5);
        assert_eq!(usage.total().bytes, disk::tree(root.path()).bytes);

        let pks: Vec<Pk> = usage.accounts.iter().map(|a| a.pk).collect();
        assert_eq!(pks, [Pk::new(42), Pk::new(43)]);
        let second = &usage.accounts[1].kinds;
        assert_eq!(second.get(Kind::Databases).bytes, 2200);
        assert_eq!(
            second.get(Kind::Cache),
            Size {
                bytes: 140,
                files: 2
            }
        );
        assert_eq!(second.get(Kind::BrowserProfiles).bytes, 1000);
        assert_eq!(second.get(Kind::Jobs).bytes, 40);
        assert_eq!(second.get(Kind::Other).bytes, 20);
    }

    #[test]
    fn measuring_makes_no_file_and_opens_no_database() {
        let root = tempfile::tempdir().unwrap();
        let paths = kept(root.path());
        let before = listing(root.path());

        measure(&paths);

        assert_eq!(listing(root.path()), before);
    }

    fn listing(root: &Path) -> Vec<(PathBuf, u64)> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let meta = entry.metadata().unwrap();
                if meta.is_dir() {
                    pending.push(entry.path());
                } else {
                    found.push((entry.path(), meta.len()));
                }
            }
        }
        found.sort();
        found
    }

    #[test]
    fn clearing_the_cache_removes_it_and_nothing_else() {
        let root = tempfile::tempdir().unwrap();
        let paths = kept(root.path());
        let legacy = paths.account(Pk::new(42)).dir().join("viewer.jpg");
        write(&legacy, 50);
        let before = measure(&paths);
        assert_eq!(before.kinds.get(Kind::Cache).bytes, 70 + 140 + 50);
        assert_eq!(before.accounts[0].kinds.get(Kind::Other).bytes, 10);

        let cleared = clear_cache(&paths, None);

        assert!(cleared.failure.is_none());
        assert_eq!(
            cleared.freed,
            Size {
                bytes: 260,
                files: 5
            }
        );
        let after = measure(&paths);
        for kind in Kind::ALL {
            let expected = if kind.clearable() {
                Size::default()
            } else {
                before.kinds.get(kind)
            };
            assert_eq!(after.kinds.get(kind), expected, "{kind:?}");
        }
        for pk in [42, 43] {
            let account = paths.account(Pk::new(pk));
            assert!(account.db_file().exists() && account.session_file().exists());
            assert!(!account.cache_dir().exists());
        }
        assert!(!legacy.exists());
        assert!(paths.registry_file().exists());
        assert!(paths.config_dir().join("watch.toml").exists());
    }

    #[test]
    fn clearing_one_accounts_cache_leaves_the_others() {
        let root = tempfile::tempdir().unwrap();
        let paths = kept(root.path());

        clear_cache(&paths, Some(Pk::new(42)));

        assert!(!paths.account(Pk::new(42)).cache_dir().exists());
        assert!(paths.account(Pk::new(43)).cache_dir().exists());
    }
}
