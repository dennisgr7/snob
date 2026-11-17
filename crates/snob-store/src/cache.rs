//! What a view shows at once while the requests it makes anyway run: an
//! account as the latest answer about it showed it ([`Shown`]), its small
//! picture, its highlights ([`KeptTray`]), the first page of each of its tabs
//! ([`KeptGrid`]), and the other pictures a view drew ([`Drawing`]), under
//! `accounts/<viewer>/cache/`.
//!
//! **Filled only from answers already paid for, never a reason to send a
//! request**, nor to skip one but a post's thumbnail already kept, which
//! never changes (AGENTS.md, "Local data"). **Kept by the account that saw
//! it**: what a private account showed one viewer is in that viewer's
//! directory, never another's. An account is found by pk, never by name, a
//! highlight by its id and a post by its code. Everything here
//! can be fetched again, so it is swept by one rule ([`sweep`]) and removed
//! whole ([`forget`]) by a sign-out, a purge and a clear.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use snob_core::{Epoch, Pk};

use crate::disk::{self, Size};
use crate::paths::{self, AccountPaths};

const PROFILES: &str = "profiles";
const PICTURES: &str = "pictures";
const TRAYS: &str = "trays";
const GRIDS: &str = "grids";

/// A picture a view drew besides an account's small one, kept under a name of
/// its own and swept by the same rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drawing {
    /// An account's largest picture, by pk.
    Largest,
    /// A highlight's cover, by the highlight's id.
    Cover,
    /// A post's thumbnail, by the post's code.
    Thumb,
}

impl Drawing {
    const ALL: [Self; 3] = [Self::Largest, Self::Cover, Self::Thumb];

    fn sub(self) -> &'static str {
        match self {
            Self::Largest => "full",
            Self::Cover => "highlights",
            Self::Thumb => "posts",
        }
    }
}

/// The most one kept account is read: a few short strings and numbers, and a
/// bio Instagram holds to 150 characters.
const MAX_SHOWN_BYTES: u64 = 64 * 1024;

/// The most a kept tray or grid is read: a first page holds a dozen posts,
/// each caption's first line at most the 2,200 characters Instagram allows a
/// caption, and a tray some hundred highlights' short titles.
const MAX_LIST_BYTES: u64 = 256 * 1024;

/// How long an entry nobody's answer renewed is kept: the 30 days old
/// captures are kept for (`store::snapshots::prune`).
pub const KEPT_FOR: Duration = Duration::from_secs(30 * 24 * 3600);

/// The most one account's cache holds, the oldest going first past it. A
/// choice, not a measurement: 64 pictures at the most a small one may weigh
/// (`snob_engine::media::MAX_SMALL_PICTURE`, 512 KiB).
pub const AT_MOST: u64 = 32 * 1024 * 1024;

/// An account as the latest answer about it showed the viewer. Every field is
/// absent until an answer says it. **Its strings are another person's
/// words**: whoever keeps one passes them through
/// `snob_core::model::printable` first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shown {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub biography: Option<String>,
    #[serde(default)]
    pub external_url: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub is_private: Option<bool>,
    #[serde(default)]
    pub is_verified: Option<bool>,
    #[serde(default)]
    pub followers: Option<u64>,
    #[serde(default)]
    pub following: Option<u64>,
    #[serde(default)]
    pub posts: Option<u64>,
    /// Whether the viewer follows the account.
    #[serde(default)]
    pub you_follow: Option<bool>,
    /// Whether the account follows the viewer.
    #[serde(default)]
    pub follows_you: Option<bool>,
    /// When an answer last said any of this.
    #[serde(default)]
    pub seen_at: Option<Epoch>,
}

impl Shown {
    /// This, with what `newer` says in place of what it said: what `newer`
    /// leaves out stays as it was.
    #[must_use]
    pub fn updated_by(self, newer: Self) -> Self {
        Self {
            username: newer.username.or(self.username),
            full_name: newer.full_name.or(self.full_name),
            biography: newer.biography.or(self.biography),
            external_url: newer.external_url.or(self.external_url),
            category: newer.category.or(self.category),
            is_private: newer.is_private.or(self.is_private),
            is_verified: newer.is_verified.or(self.is_verified),
            followers: newer.followers.or(self.followers),
            following: newer.following.or(self.following),
            posts: newer.posts.or(self.posts),
            you_follow: newer.you_follow.or(self.you_follow),
            follows_you: newer.follows_you.or(self.follows_you),
            seen_at: newer.seen_at.or(self.seen_at),
        }
    }
}

/// A tab of an account's page, as its kept first page is named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    /// The grid itself.
    Posts,
    Reels,
    Reposts,
    Tagged,
}

impl Tab {
    fn name(self) -> &'static str {
        match self {
            Self::Posts => "posts",
            Self::Reels => "reels",
            Self::Reposts => "reposts",
            Self::Tagged => "tagged",
        }
    }
}

/// An account's highlights as the latest read of its profile showed them,
/// none when it showed none; each cover is the [`Drawing::Cover`] of its id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptTray {
    #[serde(default)]
    pub highlights: Vec<KeptHighlight>,
}

/// A highlight of a [`KeptTray`]. **Its title is another person's words**,
/// passed through `snob_core::model::printable` before it is kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptHighlight {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
}

/// The first page of a grid or a tab as it was last shown; each thumbnail is
/// the [`Drawing::Thumb`] of its post's code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptGrid {
    #[serde(default)]
    pub posts: Vec<KeptPost>,
    /// There were more past it.
    #[serde(default)]
    pub more: bool,
}

/// A post of a [`KeptGrid`], as its tile showed it. **Its strings are
/// another person's words**, passed through `snob_core::model::printable`
/// before they are kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptPost {
    pub code: String,
    #[serde(default)]
    pub kind: Option<KeptKind>,
    /// The first line of its caption.
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub taken_at: Option<Epoch>,
    #[serde(default)]
    pub likes: Option<u64>,
    #[serde(default)]
    pub comments: Option<u64>,
    #[serde(default)]
    pub pinned: bool,
}

/// What a kept post is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeptKind {
    Photo,
    Video,
    Reel,
    /// A carousel, and how many items it holds.
    Carousel(usize),
}

impl AccountPaths {
    /// What the viewer was last shown of account `pk` ([`Shown`]).
    pub fn cached_profile(&self, pk: Pk) -> PathBuf {
        self.cache_dir().join(PROFILES).join(format!("{pk}.json"))
    }

    /// The highlights of account `pk` the viewer was last shown ([`KeptTray`]).
    pub fn cached_tray(&self, pk: Pk) -> PathBuf {
        self.cache_dir().join(TRAYS).join(format!("{pk}.json"))
    }

    /// The first page of account `pk`'s `tab` the viewer was last shown
    /// ([`KeptGrid`]).
    pub fn cached_grid(&self, pk: Pk, tab: Tab) -> PathBuf {
        self.cache_dir()
            .join(GRIDS)
            .join(format!("{pk}-{}.json", tab.name()))
    }

    /// The small picture of account `pk`, as the CDN served it.
    pub fn cached_picture(&self, pk: Pk) -> PathBuf {
        self.cache_dir().join(PICTURES).join(format!("{pk}.jpg"))
    }

    /// The picture `drawing` named `key` (a pk, a highlight's id, a post's
    /// code), under a name made safe for any file system.
    pub fn cached_drawing(&self, drawing: Drawing, key: &str) -> PathBuf {
        self.cache_dir()
            .join(drawing.sub())
            .join(format!("{}.jpg", file_key(key)))
    }

    /// Where the account's own picture was kept before the cache held it.
    fn picture_before_the_cache(&self) -> PathBuf {
        self.dir().join("viewer.jpg")
    }
}

/// What `account` was last shown of `pk`: none when nothing is kept, or the
/// file does not read as one. **Reads one file and sends nothing.**
pub fn load(account: &AccountPaths, pk: Pk) -> Option<Shown> {
    read_kept(&account.cached_profile(pk), MAX_SHOWN_BYTES)
}

/// `file` read as a `T` up to `most` bytes: none when it is not there or
/// does not read as one.
fn read_kept<T: serde::de::DeserializeOwned>(file: &Path, most: u64) -> Option<T> {
    let mut text = String::new();
    std::fs::File::open(file)
        .ok()?
        .take(most)
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// The highlights of `pk` `account` was last shown, as [`load`] reads.
pub fn load_tray(account: &AccountPaths, pk: Pk) -> Option<KeptTray> {
    read_kept(&account.cached_tray(pk), MAX_LIST_BYTES)
}

/// Keeps `tray` as the highlights of `pk` `account` was last shown, as
/// [`save`] keeps.
pub fn save_tray(account: &AccountPaths, pk: Pk, tray: &KeptTray) -> std::io::Result<bool> {
    let contents = serde_json::to_vec(tray).map_err(std::io::Error::other)?;
    keep(account, TRAYS, &account.cached_tray(pk), &contents)
}

/// The first page of `pk`'s `tab` `account` was last shown, as [`load`]
/// reads.
pub fn load_grid(account: &AccountPaths, pk: Pk, tab: Tab) -> Option<KeptGrid> {
    read_kept(&account.cached_grid(pk, tab), MAX_LIST_BYTES)
}

/// Keeps `grid` as the first page of `pk`'s `tab` `account` was last shown,
/// as [`save`] keeps.
pub fn save_grid(
    account: &AccountPaths,
    pk: Pk,
    tab: Tab,
    grid: &KeptGrid,
) -> std::io::Result<bool> {
    let contents = serde_json::to_vec(grid).map_err(std::io::Error::other)?;
    keep(account, GRIDS, &account.cached_grid(pk, tab), &contents)
}

/// Keeps `shown` as what `account` was last shown of `pk`. `false`, and
/// nothing written, when the account's directory is not there.
pub fn save(account: &AccountPaths, pk: Pk, shown: &Shown) -> std::io::Result<bool> {
    let contents = serde_json::to_vec_pretty(shown).map_err(std::io::Error::other)?;
    keep(account, PROFILES, &account.cached_profile(pk), &contents)
}

/// Keeps `picture` as the small picture of `pk` that `account` was shown.
/// `false`, and nothing written, when the account's directory is not there.
pub fn keep_picture(account: &AccountPaths, pk: Pk, picture: &[u8]) -> std::io::Result<bool> {
    keep(account, PICTURES, &account.cached_picture(pk), picture)
}

/// The small picture of `pk` kept for `account`, when one is.
pub fn picture(account: &AccountPaths, pk: Pk) -> Option<PathBuf> {
    let file = account.cached_picture(pk);
    file.is_file().then_some(file)
}

/// Keeps `picture` as the drawing named `key` that `account` was shown.
/// `false`, and nothing written, when the account's directory is not there.
pub fn keep_drawing(
    account: &AccountPaths,
    drawing: Drawing,
    key: &str,
    picture: &[u8],
) -> std::io::Result<bool> {
    let file = account.cached_drawing(drawing, key);
    keep(account, drawing.sub(), &file, picture)
}

/// The drawing named `key` kept for `account`, when one is.
pub fn drawing(account: &AccountPaths, drawing: Drawing, key: &str) -> Option<PathBuf> {
    let file = account.cached_drawing(drawing, key);
    file.is_file().then_some(file)
}

/// `key` as a file's name: what is not a letter, a digit, `.`, `_` or `-`
/// becomes `_` (the colon of a highlight's `highlight:1` among them), and
/// never a name starting with a dot, which [`sweep_at`] takes for a save's
/// own file.
fn file_key(key: &str) -> String {
    let name: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    match name.trim_start_matches('.') {
        "" => "_".to_string(),
        name => name.to_string(),
    }
}

/// Writes `contents` at `file` in the cache's `sub`, and sweeps when it adds
/// an entry rather than renewing one.
fn keep(account: &AccountPaths, sub: &str, file: &Path, contents: &[u8]) -> std::io::Result<bool> {
    if !account
        .create_cache_dir(sub)
        .map_err(std::io::Error::other)?
    {
        return Ok(false);
    }
    let new = !file.exists();
    // Two processes of one account may keep the same entry at once.
    paths::replace_private_alone(file, &temporary_of(file), contents).map_err(|(_, e)| e)?;
    if new {
        sweep(account);
    }
    Ok(true)
}

/// Where a file of the cache's is written before it is moved into place.
fn temporary_of(file: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(file.file_name().unwrap_or_default());
    name.push(".tmp");
    file.with_file_name(name)
}

/// The one rule the cache is kept by: what was last written more than
/// [`KEPT_FOR`] ago goes, then the oldest until [`AT_MOST`] is left. Run as
/// an entry is added; a file that will not go is left for the next sweep.
pub fn sweep(account: &AccountPaths) {
    sweep_at(account, SystemTime::now(), KEPT_FOR, AT_MOST);
}

fn sweep_at(account: &AccountPaths, now: SystemTime, kept_for: Duration, at_most: u64) {
    let mut found: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let subs = [PROFILES, PICTURES, TRAYS, GRIDS]
        .into_iter()
        .chain(Drawing::ALL.map(Drawing::sub));
    for sub in subs {
        let Ok(entries) = std::fs::read_dir(account.cache_dir().join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let written = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            // A save's lock and temporary are another process's while it
            // saves: they go by age alone.
            if entry.file_name().to_string_lossy().starts_with('.') {
                if now.duration_since(written).unwrap_or_default() > kept_for {
                    let _ = std::fs::remove_file(entry.path());
                }
                continue;
            }
            found.push((written, meta.len(), entry.path()));
        }
    }
    found.sort();
    let mut total: u64 = found.iter().map(|(_, len, _)| len).sum();
    for (written, len, path) in found {
        let old = now.duration_since(written).unwrap_or_default() > kept_for;
        if (old || total > at_most) && std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

/// What `account`'s cache weighs, the picture kept before it included.
pub fn size(account: &AccountPaths) -> Size {
    let mut size = disk::tree(&account.cache_dir());
    for file in legacy_files(account) {
        size += disk::tree(&file);
    }
    size
}

/// Whether `path`, an entry of `account`'s directory, is its cache: the
/// cache's directory, or the picture kept before it and what a killed save of
/// that left.
pub fn holds(account: &AccountPaths, path: &Path) -> bool {
    path == account.cache_dir() || legacy_files(account).iter().any(|file| file == path)
}

/// Removes `account`'s cache whole, and the picture kept before it. Nothing
/// else of the account's: its database, session, card and jobs stay.
pub fn forget(account: &AccountPaths) -> std::io::Result<()> {
    for file in legacy_files(account) {
        let _ = std::fs::remove_file(file);
    }
    paths::remove_tree_patiently(&account.cache_dir())
}

/// The account's picture as it was kept before the cache, and what a killed
/// save of it left.
fn legacy_files(account: &AccountPaths) -> [PathBuf; 3] {
    let file = account.picture_before_the_cache();
    let temporary = temporary_of(&file);
    let mut lock = temporary.clone().into_os_string();
    lock.push(".lock");
    [file, temporary, PathBuf::from(lock)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::AppPaths;

    fn account(root: &Path, pk: u64) -> AccountPaths {
        let account = AppPaths::rooted_at(root).account(Pk::new(pk));
        std::fs::create_dir_all(account.dir()).unwrap();
        account
    }

    fn shown() -> Shown {
        Shown {
            username: Some("someone".into()),
            full_name: Some("Invented Name".into()),
            followers: Some(25),
            following: Some(11),
            posts: Some(3),
            is_private: Some(true),
            seen_at: Some(Epoch::new(1_790_000_000)),
            ..Shown::default()
        }
    }

    #[test]
    fn what_is_kept_is_what_is_read_back() {
        let root = tempfile::tempdir().unwrap();
        let account = account(root.path(), 42);
        assert_eq!(load(&account, Pk::new(7)), None);

        assert!(save(&account, Pk::new(7), &shown()).unwrap());
        assert_eq!(load(&account, Pk::new(7)), Some(shown()));
        assert!(keep_picture(&account, Pk::new(7), b"\xff\xd8\xff").unwrap());
        assert_eq!(
            picture(&account, Pk::new(7)),
            Some(account.cached_picture(Pk::new(7)))
        );
    }

    #[test]
    fn what_one_account_was_shown_is_never_another_accounts() {
        let root = tempfile::tempdir().unwrap();
        let (one, other) = (account(root.path(), 42), account(root.path(), 43));

        save(&one, Pk::new(7), &shown()).unwrap();
        keep_picture(&one, Pk::new(7), b"\xff\xd8\xff").unwrap();

        assert_eq!(load(&other, Pk::new(7)), None);
        assert_eq!(picture(&other, Pk::new(7)), None);
    }

    #[test]
    fn nothing_is_kept_for_an_account_whose_directory_is_gone() {
        let root = tempfile::tempdir().unwrap();
        let account = AppPaths::rooted_at(root.path()).account(Pk::new(42));

        assert!(!save(&account, Pk::new(7), &shown()).unwrap());
        assert!(!keep_picture(&account, Pk::new(7), b"\xff\xd8\xff").unwrap());
        assert!(
            !account.dir().exists(),
            "the account's directory is not made"
        );
    }

    #[test]
    fn what_a_newer_answer_leaves_out_stays_as_it_was() {
        let newer = Shown {
            followers: Some(26),
            ..Shown::default()
        };
        let kept = shown().updated_by(newer);
        assert_eq!(kept.followers, Some(26));
        assert_eq!(kept.following, Some(11));
        assert_eq!(kept.full_name.as_deref(), Some("Invented Name"));
    }

    #[test]
    fn a_file_that_is_not_an_entry_reads_as_none() {
        let root = tempfile::tempdir().unwrap();
        let account = account(root.path(), 42);
        account.create_cache_dir(PROFILES).unwrap();
        std::fs::write(account.cached_profile(Pk::new(7)), b"not json").unwrap();
        assert_eq!(load(&account, Pk::new(7)), None);
    }

    #[test]
    fn the_sweep_takes_what_is_too_old_then_the_oldest_past_the_size() {
        let root = tempfile::tempdir().unwrap();
        let account = account(root.path(), 42);
        for pk in 1..=3 {
            keep_picture(&account, Pk::new(pk), &[0u8; 100]).unwrap();
            let written = SystemTime::now() - Duration::from_secs(pk * 3600);
            std::fs::File::options()
                .write(true)
                .open(account.cached_picture(Pk::new(pk)))
                .unwrap()
                .set_modified(written)
                .unwrap();
        }

        // Three hours old is too old; then 150 bytes leave one of the two.
        sweep_at(
            &account,
            SystemTime::now(),
            Duration::from_secs(3 * 3600 - 60),
            150,
        );

        assert!(picture(&account, Pk::new(1)).is_some(), "the newest stays");
        assert!(
            picture(&account, Pk::new(2)).is_none(),
            "the oldest past the size"
        );
        assert!(picture(&account, Pk::new(3)).is_none(), "too old");
    }

    #[test]
    fn a_drawing_is_kept_under_a_safe_name_and_swept_with_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let account = account(root.path(), 42);
        assert!(keep_drawing(&account, Drawing::Cover, "highlight:1", b"\xff\xd8\xff").unwrap());
        let kept = drawing(&account, Drawing::Cover, "highlight:1").unwrap();
        assert_eq!(kept.file_name().unwrap(), "highlight_1.jpg");
        assert_eq!(drawing(&account, Drawing::Thumb, "highlight:1"), None);
        assert_eq!(file_key("..x"), "x");
        assert_eq!(file_key(""), "_");

        std::fs::File::options()
            .write(true)
            .open(&kept)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        sweep_at(
            &account,
            SystemTime::now(),
            Duration::from_secs(60),
            u64::MAX,
        );
        assert_eq!(drawing(&account, Drawing::Cover, "highlight:1"), None);
    }

    fn grid() -> KeptGrid {
        KeptGrid {
            posts: vec![KeptPost {
                code: "Cabc".into(),
                kind: Some(KeptKind::Carousel(3)),
                caption: Some("A first line".into()),
                taken_at: Some(Epoch::new(1_790_000_000)),
                likes: Some(12),
                pinned: true,
                ..KeptPost::default()
            }],
            more: true,
        }
    }

    #[test]
    fn a_tray_and_each_tabs_first_page_are_read_back_and_never_another_accounts() {
        let root = tempfile::tempdir().unwrap();
        let (one, other) = (account(root.path(), 42), account(root.path(), 43));
        let tray = KeptTray {
            highlights: vec![KeptHighlight {
                id: "highlight:1".into(),
                title: Some("Trips".into()),
            }],
        };
        assert_eq!(load_tray(&one, Pk::new(7)), None);

        assert!(save_tray(&one, Pk::new(7), &tray).unwrap());
        assert!(save_grid(&one, Pk::new(7), Tab::Tagged, &grid()).unwrap());

        assert_eq!(load_tray(&one, Pk::new(7)), Some(tray));
        assert_eq!(load_grid(&one, Pk::new(7), Tab::Tagged), Some(grid()));
        assert_eq!(load_grid(&one, Pk::new(7), Tab::Posts), None, "a tab each");
        assert_eq!(load_tray(&other, Pk::new(7)), None);
        assert_eq!(load_grid(&other, Pk::new(7), Tab::Tagged), None);
    }

    #[test]
    fn a_kept_tray_and_grid_are_swept_with_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let account = account(root.path(), 42);
        save_tray(&account, Pk::new(7), &KeptTray::default()).unwrap();
        save_grid(&account, Pk::new(7), Tab::Posts, &grid()).unwrap();
        for file in [
            account.cached_tray(Pk::new(7)),
            account.cached_grid(Pk::new(7), Tab::Posts),
        ] {
            std::fs::File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(3600))
                .unwrap();
        }

        sweep_at(
            &account,
            SystemTime::now(),
            Duration::from_secs(60),
            u64::MAX,
        );

        assert_eq!(load_tray(&account, Pk::new(7)), None);
        assert_eq!(load_grid(&account, Pk::new(7), Tab::Posts), None);
    }

    #[test]
    fn forgetting_leaves_none_of_the_cache_and_all_of_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let account = account(root.path(), 42);
        save(&account, Pk::new(7), &shown()).unwrap();
        keep_picture(&account, Pk::new(42), b"\xff\xd8\xff").unwrap();
        std::fs::write(account.dir().join("viewer.jpg"), b"\xff\xd8\xff").unwrap();
        std::fs::write(account.db_file(), b"a database").unwrap();
        std::fs::write(account.viewer_card(), b"{}").unwrap();
        // The two kept, the one from before the cache, and the locks of
        // their saves.
        assert!(size(&account).files >= 3);

        forget(&account).unwrap();

        assert!(!account.cache_dir().exists());
        assert!(!account.dir().join("viewer.jpg").exists());
        assert!(account.db_file().exists() && account.viewer_card().exists());
        assert_eq!(size(&account), Size::default());
    }
}
