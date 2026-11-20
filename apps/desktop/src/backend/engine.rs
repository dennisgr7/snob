//! The engine, answering the views: what `snob login`, `snob logout` and
//! `snob account use` do, called in process (`snob_engine`), on the app's
//! runtime and never on the window's thread.
//!
//! **Nothing here is a second way to do what a command does.** A sign-in is
//! `snob_engine::login`, a sign-out `snob_engine::logout`, the account in use
//! the registry's; what this file adds is which of them to call and how to
//! say what came back to a screen.
//!
//! **A profile page is what `snob profile` reads, less the stories**:
//! `snob_engine::profile::read`, which keeps the profile and its highlights
//! in the cache a page is drawn from at once next time, and a tab's first
//! page once the tab is shown (`posts::first_page`, kept the same way). A
//! post's thumbnail already kept is not downloaded again. Its lists are `engine::list`, the yes for
//! another account's given in the app before it is called. Its largest
//! picture is `snob pfp`'s (`profile::picture`). Pictures come from the CDN
//! into the account's cache (`cache::Keeper`), never from Instagram's API.
//! A tab reads on a page at a time from the cursor the engine handed out
//! (`posts::page_after`), which stays here: a view holds only its id. A
//! list is shown first from what was stored (`cache`, `--offline`'s
//! answer), and the names typed in a form are suggested from the account's
//! database (`cache::known_named`): neither sends anything.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use snob_core::secret::Secret;
use snob_core::{Epoch, Pk};
use snob_engine::app::Viewer;
use snob_engine::cache::Drawing;
use snob_engine::login::{self, Agent, ByBrowser, Failure, Opened};
use snob_engine::posts::{After, Grid};
use snob_engine::{browser, host, log, logout, storage};
use snob_ig::pace::CancelToken;
use snob_store::paths::AppPaths;
use snob_store::registry::Registry;
use snob_store::secrets::SecretStore;
use tokio::sync::{mpsc, watch};

use super::{
    Account, AccountStorage, Answer, Automation, Backend, Browsers, Cleared, Cursor, Forgotten,
    Highlight, Listed, Loaded, MOST_PICTURES, Next, Person, Problem, Profile, Run, SignIn, Storage,
    Tile, TileKind, Tiles, Unread, Walked, answered,
};
use crate::report;
use crate::runtime::Runtime;
use crate::state::flow::{Found, Method, Named, Update};
use crate::state::prefs::{Appearance, Placement, Prefs, Resume};
use crate::state::profile::{Side, Tab};

/// Where an account's largest picture is (`snob_engine::profile::picture`).
#[derive(Clone)]
struct Largest {
    username: String,
    fallback: Option<String>,
    full_size: Option<String>,
}

/// How many cursors are held for the views at most: a tab of each profile
/// page the way back can still show, and more. Past it the oldest goes, and
/// its tab reads no further.
const MOST_CURSORS: usize = 64;

/// How many of a list's small pictures are downloaded at once: the three of
/// the `join!` in [`Engine::pictures`].
const AT_ONCE: usize = 3;

/// How many known accounts one call answers at most, whatever is asked.
const MOST_KNOWN: usize = 50;

/// Whose tab a cursor reads on: the account in use it was handed out to,
/// and the profile and tab it is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Whose {
    viewer: Pk,
    pk: Pk,
    tab: Tab,
}

/// Where each tab handed to a view reads on (the engine's [`After`]), by
/// the id its [`Cursor`] carries, with whose tab it is.
struct Cursors<T = After> {
    made: u64,
    /// Moved on by [`Self::let_go`]: what a page read before it found is
    /// neither put back nor handed out.
    era: u64,
    held: BTreeMap<u64, (Whose, T)>,
}

impl<T> Default for Cursors<T> {
    fn default() -> Self {
        Self {
            made: 0,
            era: 0,
            held: BTreeMap::new(),
        }
    }
}

impl<T> Cursors<T> {
    /// Keeps `at` for `whose` tab under an id no cursor had, unless the
    /// cursors were let go of since `era`.
    fn hand_out(&mut self, era: u64, whose: Whose, at: T) -> Option<Cursor> {
        if era != self.era {
            return None;
        }
        self.made += 1;
        self.held.insert(self.made, (whose, at));
        while self.held.len() > MOST_CURSORS {
            self.held.pop_first();
        }
        Some(Cursor(self.made))
    }

    /// What `cursor` stands for, taken out while its page is read, and the
    /// era it was taken in: none when it is not held, or not `whose` tab
    /// (another account's in use included), which leaves it held.
    fn take(&mut self, cursor: Cursor, whose: Whose) -> Option<(T, u64)> {
        match self.held.get(&cursor.0) {
            Some((of, _)) if *of == whose => {
                let (_, at) = self.held.remove(&cursor.0)?;
                Some((at, self.era))
            }
            _ => None,
        }
    }

    /// Puts `at` back under `cursor`, as its page left it, unless the
    /// cursors were let go of since it was taken in `era`.
    fn put_back(&mut self, era: u64, cursor: Cursor, whose: Whose, at: T) {
        if era == self.era {
            self.held.insert(cursor.0, (whose, at));
        }
    }

    /// Forgets every cursor, and what a page being read would hand back:
    /// **a browser released or another account's may never have been
    /// navigated to the tab**, nor see what the account that was handed
    /// it saw.
    fn let_go(&mut self) {
        self.held.clear();
        self.era += 1;
    }
}

/// The cursors, locked whatever a panic elsewhere left.
fn cursors(cursors: &Mutex<Cursors>) -> std::sync::MutexGuard<'_, Cursors> {
    cursors.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The engine, for the data directory `paths` is.
pub struct Engine {
    runtime: Arc<Runtime>,
    paths: AppPaths,
    secrets: SecretStore,
    detail: log::Detail,
    files: Arc<log::Files>,
    /// What the system is, for a diagnostic report.
    about: String,
    /// Where each account's largest picture is, as its profile last named
    /// it: what the viewer fetches, with no request of its own.
    addresses: Arc<Mutex<HashMap<Pk, Largest>>>,
    /// Where the tabs handed to the views read on.
    cursors: Arc<Mutex<Cursors>>,
    /// How many browsers are listed, whichever machine a test's pictures
    /// are taken on.
    #[cfg(feature = "testing")]
    listed: Option<usize>,
    /// Whether what is kept is said the same on every run, for pictures.
    #[cfg(feature = "testing")]
    fixed_storage: bool,
}

impl Engine {
    pub fn new(
        runtime: Arc<Runtime>,
        paths: AppPaths,
        secrets: SecretStore,
        log: (log::Detail, Arc<log::Files>),
        about: String,
    ) -> Self {
        Self {
            runtime,
            paths,
            secrets,
            detail: log.0,
            files: log.1,
            about,
            addresses: Arc::default(),
            cursors: Arc::default(),
            #[cfg(feature = "testing")]
            listed: None,
            #[cfg(feature = "testing")]
            fixed_storage: false,
        }
    }

    /// Lists `count` browsers, whatever is installed: the first ones found,
    /// then the others snob knows by name. **Only the list**: a session is
    /// still sent as a browser that is installed, or not at all.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn listing_browsers(mut self, count: usize) -> Self {
        self.listed = Some(count);
        self
    }

    /// Says the same sizes on every run, whatever is on disk, and the same
    /// moment for when every list was read, so a picture of them is the
    /// same on every machine. **Only what is shown**: clearing the cache
    /// still removes it.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn fixing_storage(mut self) -> Self {
        self.fixed_storage = true;
        self
    }

    /// When every list is said to have been read with `fixing_storage`.
    fn fixed_scan(&self) -> Option<Epoch> {
        #[cfg(feature = "testing")]
        if self.fixed_storage {
            return Some(Epoch::new(1_790_000_000));
        }
        None
    }
}

/// What the test build says is kept with `fixed_storage`: a disk of 512 GB
/// with a fifth free, a size for each kind, the update and the scratch left
/// empty, the accounts signed in, and one left behind by a sign-out.
#[cfg(feature = "testing")]
fn fixed(accounts: &[Account]) -> Storage {
    use snob_engine::storage::Kind;
    let size = |bytes: u64, files: u64| snob_store::disk::Size { bytes, files };
    const GIB: u64 = 1024 * 1024 * 1024;
    let left = AccountStorage {
        pk: Pk::new(9_000_000_001),
        username: Some("left_behind".to_string()),
        signed_in: false,
        history: size(1_153_434, 2),
        cache: size(0, 0),
    };
    Storage {
        disk: Some(snob_store::disk::Volume {
            total: 512 * GIB,
            free: 102 * GIB + 418 * 1024 * 1024,
        }),
        kinds: vec![
            (Kind::Databases, size(6_815_744, 4)),
            (Kind::Cache, size(3_250_586, 212)),
            (Kind::BrowserProfiles, size(51_904_512, 1_310)),
            (Kind::Jobs, size(18_432, 3)),
            (Kind::Logs, size(2_097_152, 5)),
            (Kind::Update, size(0, 0)),
            (Kind::Scratch, size(0, 0)),
            (Kind::Other, size(41_984, 9)),
        ],
        accounts: accounts
            .iter()
            .map(|account| AccountStorage {
                pk: account.named.pk,
                username: account.named.username.clone(),
                signed_in: true,
                history: size(5_662_310, 5),
                cache: size(3_250_586, 212),
            })
            .chain(std::iter::once(left))
            .collect(),
    }
}

/// What each account with a directory here holds: those signed in in
/// `here`'s order, then the others by id, named as `accounts.toml` keeps
/// them. **One signed out of is listed while its history or cache is still
/// here**, so it can be seen and deleted; a directory with nothing of
/// either left is not.
fn held_by_accounts(
    paths: &AppPaths,
    here: &[Account],
    usage: &storage::Usage,
) -> Vec<AccountStorage> {
    let registry = Registry::load(paths).ok();
    let named = |pk: Pk| {
        here.iter()
            .find(|account| account.named.pk == pk)
            .and_then(|account| account.named.username.clone())
            .or_else(|| {
                let registered = registry.as_ref()?.accounts.iter().find(|a| a.pk == pk)?;
                Some(snob_core::model::printable(&registered.username))
            })
    };
    let mut held: Vec<AccountStorage> = paths
        .account_dirs()
        .into_iter()
        .map(|pk| {
            let kinds = usage
                .accounts
                .iter()
                .find(|held| held.pk == pk)
                .map(|held| held.kinds.clone())
                .unwrap_or_default();
            AccountStorage {
                pk,
                username: named(pk),
                signed_in: here.iter().any(|account| account.named.pk == pk),
                history: kinds
                    .get(storage::Kind::Databases)
                    .plus(kinds.get(storage::Kind::Jobs)),
                cache: kinds.get(storage::Kind::Cache),
            }
        })
        .filter(|held: &AccountStorage| {
            held.signed_in || held.history.bytes > 0 || held.cache.bytes > 0
        })
        .collect();
    let at = |pk: Pk| {
        here.iter()
            .position(|account| account.named.pk == pk)
            .unwrap_or(usize::MAX)
    };
    held.sort_by_key(|account| (at(account.pk), account.pk));
    held
}

/// The account `pk` as a view shows it.
fn account_of(paths: &AppPaths, pk: Pk, username: Option<String>, active: bool) -> Account {
    let account = paths.account(pk);
    // The card, and what the cache kept of the account's own page where the
    // card says nothing: a bio the tab's document left out, say.
    let shown = snob_engine::cache::shown(paths, pk, pk).unwrap_or_default();
    let said = |text: Option<String>| text.filter(|text| !text.trim().is_empty());
    let picture = account.viewer_picture();
    let kept_at = std::fs::metadata(&picture)
        .and_then(|file| file.modified())
        .ok();
    Account {
        named: Named {
            pk,
            username: said(username.map(|name| snob_core::model::printable(&name))),
        },
        full_name: said(shown.full_name),
        biography: said(shown.biography),
        picture: picture.is_file().then_some(picture),
        picture_kept_at: kept_at,
        active,
    }
}

/// Every account signed in here, the one in use first, and which that is.
/// **From the list of accounts and each one's `signed-in` list**: no keyring
/// is read and nothing is sent, so this is what a start can afford.
fn signed_in(paths: &AppPaths) -> Vec<Account> {
    let registry = match Registry::load(paths) {
        Ok(registry) => registry,
        Err(e) => {
            tracing::warn!(error = %e, "the list of accounts could not be read");
            return Vec::new();
        }
    };
    let here = |pk: Pk| paths.account(pk).anybody_signed_in();
    // The account in use, or when it was signed out the first that was not.
    let active = registry
        .active
        .filter(|pk| here(*pk))
        .or_else(|| registry.accounts.iter().map(|a| a.pk).find(|pk| here(*pk)));
    if active != registry.active
        && let Some(pk) = active
        && let Err(e) = Registry::update(paths, |registry| registry.active = Some(pk))
    {
        tracing::warn!(error = %e, "the account in use could not be written down");
    }
    let mut accounts: Vec<Account> = registry
        .accounts
        .iter()
        .filter(|account| here(account.pk))
        .map(|account| {
            let name = (!account.username.is_empty()).then(|| account.username.clone());
            account_of(paths, account.pk, name, Some(account.pk) == active)
        })
        .collect();
    accounts.sort_by_key(|account| !account.active);
    accounts
}

/// What a failed sign-in is said as.
fn failed(paths: &AppPaths, pk: Option<Pk>, error: &anyhow::Error) -> SignIn {
    let why = login::failure_of(error);
    let until = (why == Failure::PushedBack)
        .then(|| pk.and_then(|pk| paused(paths, pk)))
        .flatten();
    SignIn::Failed {
        why,
        id: log::failed(error),
        until,
    }
}

/// Until when the account `pk` may send nothing, from its budget: read, and
/// nothing written.
fn paused(paths: &AppPaths, pk: Pk) -> Option<Epoch> {
    let account = paths.account(pk);
    let now = snob_core::clock::now_ms();
    let (_, budget) = snob_engine::app::standing(&account, now).ok()?;
    snob_engine::app::held_in(&budget, &account, now).map(|held| held.until_ms.to_epoch())
}

/// The account that just signed in, with what the browser its check went
/// out from says of it (`viewer::look_again`), then its own profile read
/// once, its counters and both its pictures kept (`viewer::read_after_login`),
/// so the welcome, the menu and its page draw at once. **Not at any other
/// time**: a start reads the files alone, and the day's check of the
/// session only looks at the tab again.
async fn done(paths: &AppPaths, secrets: &SecretStore, viewer: Viewer) -> SignIn {
    let account = paths.account(viewer.pk);
    // **The name the list of accounts keeps when the login's check did not
    // read one** (a push-back on it, a check skipped in a pause): what a
    // start would show, shown from the welcome on rather than once the
    // files are read again.
    let username = viewer.username.filter(|name| !name.is_empty()).or_else(|| {
        let registry = Registry::load(paths).ok()?;
        snob_engine::account::username(&registry, viewer.pk).map(str::to_string)
    });
    let opened =
        snob_engine::app::App::open(&secrets.session_of(&account), &account, host::Front::quiet);
    match opened {
        Ok(Some(app)) => {
            snob_engine::viewer::look_again(app.client(), &account).await;
            if let Some(name) = &username {
                snob_engine::viewer::read_after_login(&app, &account, name).await;
            }
        }
        Ok(None) => {}
        Err(e) => tracing::debug!(error = %e, "the account's picture was not fetched"),
    }
    Prefs::change(paths, |prefs| {
        prefs.expired = prefs.expired.filter(|pk| *pk != viewer.pk);
        // A login is a check that answered.
        prefs.note_checked(viewer.pk, snob_core::clock::now());
    });
    SignIn::Done(account_of(paths, viewer.pk, username, true))
}

/// The store a login writes to: the keyring, or a private file where there
/// is none, as `snob login` settles it before anything is asked.
fn writable(secrets: &SecretStore) -> anyhow::Result<SecretStore> {
    let usable = secrets.probe_writable()?;
    Ok(secrets.clone().using(usable))
}

/// The account the views act as: the one in use.
fn in_use(paths: &AppPaths) -> Option<Pk> {
    Registry::load(paths).ok()?.active
}

/// The engine, opened as the account in use, saying nothing and asking
/// nobody: `stop` ends what it does. **Refused in a pause**, before
/// anything is sent.
fn opened(
    paths: &AppPaths,
    secrets: &SecretStore,
    stop: Option<CancelToken>,
    progress: Option<Arc<dyn host::Progress>>,
) -> Result<(Pk, snob_engine::app::App), Unread> {
    let viewer = in_use(paths).ok_or(Unread::NoSession)?;
    let account = paths.account(viewer);
    let front = move || {
        let quiet = host::Front::quiet();
        host::Front {
            progress: progress.unwrap_or(quiet.progress),
            ask: quiet.ask,
            cancel: stop,
        }
    };
    let app = match snob_engine::app::App::open(&secrets.session_of(&account), &account, front) {
        Ok(Some(app)) => app,
        Ok(None) => return Err(Unread::NoSession),
        Err(e) => return Err(unread(paths, viewer, &e)),
    };
    if paused(paths, viewer).is_some() {
        return Err(Unread::Paused(paused(paths, viewer)));
    }
    Ok((viewer, app))
}

/// What a failed read is said as: its code logged, never its words.
fn unread(paths: &AppPaths, viewer: Pk, error: &anyhow::Error) -> Unread {
    use snob_engine::exit::{ExitCode, exit_code_for};
    use snob_ig::error::IgError;
    let instagram = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<IgError>());
    if matches!(instagram, Some(IgError::NotFound { .. })) {
        return Unread::Nobody;
    }
    match exit_code_for(error) {
        ExitCode::NoSession => Unread::NoSession,
        ExitCode::RateLimited | ExitCode::Challenge => Unread::Paused(paused(paths, viewer)),
        _ => Unread::Failed(log::failed(error)),
    }
}

/// What a page shows of a list's account.
fn person(user: &snob_core::model::User, picture: Option<PathBuf>) -> Person {
    let printable = snob_core::model::printable;
    Person {
        named: Named {
            pk: user.pk,
            username: Some(printable(&user.username)).filter(|name| !name.is_empty()),
        },
        full_name: user
            .full_name
            .as_deref()
            .map(printable)
            .filter(|name| !name.trim().is_empty()),
        is_private: user.is_private.unwrap_or(false),
        is_verified: user.is_verified.unwrap_or(false),
        picture,
        picture_from: user.pfp_url.clone().filter(|address| !address.is_empty()),
    }
}

/// A list walk, shown as it goes: after each page, what the walk saved of
/// it, read from a connection of its own (`snapshots::walking_here`).
struct Showing {
    account: snob_store::paths::AccountPaths,
    owner: Pk,
    kind: snob_core::model::ListKind,
    db: Mutex<Option<snob_store::store::Store>>,
    walked: watch::Sender<Walked>,
}

impl Showing {
    fn show(&self) {
        let mut db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        if db.is_none() {
            *db = snob_store::store::Store::read_existing(&self.account)
                .ok()
                .flatten();
        }
        let Some(db) = db.as_ref() else {
            return;
        };
        match snob_store::store::snapshots::walking_here(db.conn(), self.owner, self.kind) {
            Ok((users, expected)) if !users.is_empty() => {
                let people = users.iter().map(|user| person(user, None)).collect();
                self.walked.send_replace(Walked { people, expected });
            }
            Ok(_) => {}
            Err(e) => tracing::debug!(error = %e, "a list being read could not be shown"),
        }
    }
}

impl host::Progress for Showing {
    fn event(&self, event: &snob_ig::pager::Event) {
        match event {
            snob_ig::pager::Event::Started { estimated, .. } => {
                let expected = *estimated;
                self.walked.send_modify(|walked| walked.expected = expected);
            }
            snob_ig::pager::Event::Page { .. } => self.show(),
            _ => {}
        }
    }
    fn warn(&self, text: &str) {
        host::warn(text);
    }
    fn waiting(&self, _: host::Wait, _: std::time::Duration) {}
    fn begin(&self, _: &str) {}
    fn walking(&self, _: snob_core::model::ListKind) {}
    fn served(
        &self,
        _: snob_core::model::ListKind,
        _: &str,
        _: Epoch,
        _: Option<Epoch>,
        _: snob_core::model::StopReason,
        _: usize,
    ) {
    }
    fn finish(&self) {}
    fn paused(&self, f: &mut dyn FnMut()) {
        f();
    }
}

/// What a tab's post is, for its tile, as the cache keeps it
/// (`snob_engine::cache::kept_post`) whether it was read now or before.
fn tile_of(post: snob_engine::cache::KeptPost, thumb: Option<PathBuf>) -> Tile {
    use snob_engine::cache::KeptKind;
    Tile {
        code: post.code,
        kind: match post.kind {
            Some(KeptKind::Photo) | None => TileKind::Photo,
            Some(KeptKind::Video) => TileKind::Video,
            Some(KeptKind::Reel) => TileKind::Reel,
            Some(KeptKind::Carousel(items)) => TileKind::Carousel(items),
        },
        thumb,
        caption: post.caption,
        owner: post.owner,
        taken_at: post.taken_at,
        likes: post.likes,
        comments: post.comments,
        pinned: post.pinned,
    }
}

/// The tab of a profile's page `tab` is read as.
fn profile_tab(tab: Tab) -> Option<snob_ig::client::ProfileTab> {
    use snob_ig::client::ProfileTab;
    match tab {
        Tab::Posts => None,
        Tab::Reposts => Some(ProfileTab::Reposts),
        Tab::Tagged => Some(ProfileTab::Tagged),
    }
}

/// The tiles of the posts a page of `grid` read, their small pictures on
/// disk: one already kept is not downloaded again.
async fn tiles_of(app: &snob_engine::app::App, grid: &Grid) -> Vec<Tile> {
    let keeper = app.cache();
    let mut tiles = Vec::with_capacity(grid.posts.len());
    for post in &grid.posts {
        let address = post.items.first().and_then(|item| item.thumb.as_deref());
        let thumb = match (&keeper, address) {
            (Some(keeper), Some(address)) => {
                keeper.fetch_thumb(app.client(), &post.code, address).await
            }
            _ => None,
        };
        tiles.push(tile_of(snob_engine::cache::kept_post(post), thumb));
    }
    tiles
}

/// When a rest of `left` that begins now ends, to the second after.
fn rest_ends(left: std::time::Duration) -> Epoch {
    let seconds = left
        .as_secs()
        .saturating_add(u64::from(left.subsec_nanos() > 0));
    let now = snob_core::clock::now().get();
    Epoch::new(now.saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX)))
}

impl Backend for Engine {
    fn load(&self) -> Answer<Loaded> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            let accounts = signed_in(&paths);
            let prefs = Prefs::load(&paths);
            let expired = prefs.expired;
            let registry = Registry::load(&paths).ok();
            let found = match accounts.first() {
                Some(active) if expired != Some(active.named.pk) => {
                    Found::SignedIn(active.named.clone())
                }
                Some(active) => Found::Expired(active.named.clone()),
                // Nobody signed in: the account last in use is asked back
                // when its session was found dead, and is nobody otherwise.
                None => registry
                    .as_ref()
                    .and_then(|registry| registry.active_account())
                    .filter(|account| expired == Some(account.pk))
                    .map_or(Found::Nobody, |account| {
                        Found::Expired(Named {
                            pk: account.pk,
                            username: Some(snob_core::model::printable(&account.username)),
                        })
                    }),
            };
            let crash = log::Crash::waiting(&log::dir(&paths)).map(|crash| crash.report);
            Loaded {
                found,
                accounts,
                crash,
                resume: prefs.resumed(snob_core::clock::now()),
                introduced: prefs.introduced,
            }
        })
    }

    fn browsers(&self) -> Answer<Browsers> {
        let paths = self.paths.clone();
        #[cfg(feature = "testing")]
        let listed = self.listed;
        #[cfg(not(feature = "testing"))]
        let listed = None::<usize>;
        self.runtime.run(move || async move {
            let mut found: Vec<String> = Vec::new();
            for browser in browser::detect_all() {
                if !found.iter().any(|name| name == browser.name) {
                    found.push(browser.name.to_string());
                }
            }
            if let Some(count) = listed {
                found.truncate(count);
                for known in ["Chrome", "Edge", "Brave"] {
                    if found.len() < count && !found.iter().any(|name| name == known) {
                        found.push(known.to_string());
                    }
                }
            }
            let last = Prefs::load(&paths)
                .browser
                .filter(|last| found.contains(last));
            Browsers { found, last }
        })
    }

    fn sign_in_with(&self, session_id: Secret, browser: Option<String>) -> Answer<SignIn> {
        cursors(&self.cursors).let_go();
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        self.runtime.run(move || async move {
            tracing::info!(method = "paste", "signing in");
            // The session is sent as the browser it was copied from, which
            // the engine keeps following as that browser updates.
            let detected = match &browser {
                Some(name) => browser::detect_named(name),
                None => browser::detect(),
            };
            let Some(browser) = detected else {
                return failed(&paths, None, &anyhow::Error::new(login::NoBrowser));
            };
            let stored = match writable(&secrets) {
                Ok(secrets) => {
                    login::by_paste(
                        session_id.expose(),
                        Agent::of(&browser),
                        None,
                        secrets,
                        &paths,
                        &CancelToken::default(),
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            match stored {
                Ok(stored) => {
                    Prefs::change(&paths, |prefs| {
                        prefs.browser = Some(browser.name.to_string());
                    });
                    done(&paths, &secrets, stored.viewer).await
                }
                Err(e) => {
                    let pk = snob_core::session::Session::from_sessionid(
                        session_id.expose(),
                        "Mozilla/5.0",
                        snob_core::session::SessionOrigin::Paste,
                    )
                    .ok()
                    .map(|session| session.ds_user_id);
                    failed(&paths, pk, &e)
                }
            }
        })
    }

    fn sign_in_in_a_window(&self, renewing: Option<Pk>, stop: CancelToken) -> Answer<SignIn> {
        cursors(&self.cursors).let_go();
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        self.runtime.run(move || async move {
            tracing::info!(method = "browser", "signing in");
            let Some(browser) = browser::detect() else {
                return failed(&paths, None, &anyhow::Error::new(login::NoBrowser));
            };
            let renewing = renewing.map(|pk| Viewer {
                pk,
                username: Registry::load(&paths).ok().and_then(|registry| {
                    snob_engine::account::username(&registry, pk).map(str::to_string)
                }),
            });
            let stored = match writable(&secrets) {
                Ok(secrets) => {
                    login::by_browser(
                        ByBrowser {
                            browser: &browser,
                            user_agent_pinned: false,
                            renewing: renewing.clone(),
                            method: login::Method::Browser,
                        },
                        secrets,
                        &paths,
                        &stop,
                        async |opened: Opened<'_>| {
                            login::in_a_window(
                                &browser,
                                None,
                                opened.profile,
                                &stop,
                                &host::Unwatched,
                            )
                            .await
                        },
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            match stored {
                Ok(Some(stored)) => done(&paths, &secrets, stored.viewer).await,
                Ok(None) => SignIn::GaveUp,
                // Closing the window is a change of mind.
                Err(e) if login::failure_of(&e) == Failure::WindowClosed => SignIn::GaveUp,
                Err(e) => failed(&paths, renewing.map(|viewer| viewer.pk), &e),
            }
        })
    }

    fn sign_out(&self, pk: Pk) -> Answer<Vec<Account>> {
        cursors(&self.cursors).let_go();
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        self.runtime.run(move || async move {
            tracing::info!("signing out");
            let who = logout::Who::of(&paths, Some(pk), false);
            // Its jobs first, and their lock held until the session is gone.
            match snob_engine::jobs::stop_every_job(who.accounts(&paths)).await {
                Ok((stopping, _)) => {
                    let left = logout::sign_out(&secrets, &paths, &who, &stopping).await;
                    if let Err(e) = &left.sessions {
                        log::failed(&anyhow::anyhow!("the session could not be removed: {e}"));
                    }
                    if let Some((profile, e)) = &left.profile_failure {
                        log::failed(&anyhow::anyhow!(
                            "{} could not be removed: {e}",
                            profile.display()
                        ));
                    }
                }
                Err(e) => {
                    log::failed(&e);
                }
            }
            Prefs::change(&paths, |prefs| {
                prefs.expired = prefs.expired.filter(|expired| *expired != pk);
                prefs.checked.retain(|checked| checked.pk != pk);
            });
            signed_in(&paths)
        })
    }

    fn switch_to(&self, pk: Pk) -> Answer<Vec<Account>> {
        cursors(&self.cursors).let_go();
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            tracing::info!("changing the account in use");
            let changed = Registry::update(&paths, |registry| {
                // Only to an account the file still lists.
                if registry.get(pk).is_some() {
                    registry.active = Some(pk);
                }
            });
            if let Err(e) = changed {
                log::failed(&anyhow::Error::new(e));
            }
            signed_in(&paths)
        })
    }

    fn paused_until(&self) -> Answer<Option<Epoch>> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            let active = Registry::load(&paths).ok()?.active?;
            paused(&paths, active)
        })
    }

    fn expired_session(&self) -> Answer<Option<Named>> {
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        self.runtime.run(move || async move {
            let registry = Registry::load(&paths).ok()?;
            let pk = registry.active?;
            let account = paths.account(pk);
            let now = snob_core::clock::now();
            if !account.anybody_signed_in()
                || Prefs::load(&paths).checked_lately(pk, now)
                // Nothing is sent in a pause, this included.
                || paused(&paths, pk).is_some()
            {
                return None;
            }
            let named = || Named {
                pk,
                username: registry
                    .get(pk)
                    .map(|account| snob_core::model::printable(&account.username))
                    .filter(|name| !name.is_empty()),
            };
            let found_dead = || {
                tracing::info!("the session no longer works");
                Prefs::change(&paths, |prefs| prefs.expired = Some(pk));
                Some(named())
            };
            let opened = snob_engine::app::App::open(
                &secrets.session_of(&account),
                &account,
                host::Front::quiet,
            );
            let app = match opened {
                Ok(Some(app)) => app,
                // Signed in, and no session kept: as good as dead.
                Ok(None) => return found_dead(),
                Err(e) => {
                    tracing::debug!(error = %e, "the session could not be asked about");
                    return None;
                }
            };
            tracing::info!("asking whether the session still works");
            match app.client().validate().await {
                Ok(()) => {
                    Prefs::change(&paths, |prefs| prefs.note_checked(pk, now));
                    // The tab the check went out from says the account's
                    // name and picture for nothing: a new picture is fetched
                    // here, and shown once the files are read again.
                    snob_engine::viewer::look_again(app.client(), &account).await;
                    None
                }
                Err(snob_ig::error::IgError::SessionExpired) => found_dead(),
                // A push-back, no network, anything else: not known, and
                // asked again another time.
                Err(e) => {
                    tracing::debug!(error = %e, "the session could not be asked about");
                    None
                }
            }
        })
    }

    fn remember_method(&self, method: Option<Method>) {
        let paths = self.paths.clone();
        // A file replaced: off the window's thread, and nobody waits for it.
        drop(self.runtime.run(move || async move {
            Prefs::change(&paths, |prefs| {
                prefs.resume = method.map(|method| Resume {
                    method,
                    at: snob_core::clock::now(),
                });
            });
        }));
    }

    fn remember_window(&self, placement: Placement) {
        let paths = self.paths.clone();
        drop(self.runtime.run(move || async move {
            Prefs::change(&paths, |prefs| prefs.window = Some(placement));
        }));
    }

    fn remember_appearance(&self, appearance: Appearance) {
        let paths = self.paths.clone();
        drop(self.runtime.run(move || async move {
            Prefs::change(&paths, |prefs| {
                prefs.theme = appearance.theme;
                prefs.language = appearance.language;
            });
        }));
    }

    fn remember_introduced(&self, pk: Pk) {
        let paths = self.paths.clone();
        drop(self.runtime.run(move || async move {
            Prefs::change(&paths, |prefs| {
                if !prefs.introduced.contains(&pk) {
                    prefs.introduced.push(pk);
                }
            });
        }));
    }

    // The app's own update comes with its packaging: until there is a
    // release of the app to fetch, there is nothing newer to say.
    fn check_for_update(&self) -> Answer<Update> {
        answered(Update::None)
    }

    fn fetch_update(&self, _: watch::Sender<Option<u8>>) -> Answer<bool> {
        answered(false)
    }

    fn detailed_until(&self) -> Option<Epoch> {
        self.detail.on_until()
    }

    fn set_detailed(&self, on: bool) {
        self.detail.set(on);
        tracing::info!(on, "detailed logging");
    }

    fn logs_dir(&self) -> PathBuf {
        self.files.dir().to_path_buf()
    }

    fn diagnostic_report(&self) -> Answer<String> {
        let (paths, files, about) = (
            self.paths.clone(),
            Arc::clone(&self.files),
            self.about.clone(),
        );
        self.runtime.run(move || async move {
            let crash = log::Crash::newest(&log::dir(&paths));
            report::diagnostic(&about, &files.recent(report::LINES), crash.as_deref())
        })
    }

    fn crash_shown(&self) {
        let paths = self.paths.clone();
        // Files renamed: off the window's thread, and nobody waits for it.
        drop(self.runtime.run(move || async move {
            if let Some(crash) = log::Crash::waiting(&log::dir(&paths)) {
                crash.shown();
            }
        }));
    }

    fn storage(&self) -> Answer<Storage> {
        let paths = self.paths.clone();
        #[cfg(feature = "testing")]
        let fixed_storage = self.fixed_storage;
        self.runtime.run(move || async move {
            let here = signed_in(&paths);
            #[cfg(feature = "testing")]
            if fixed_storage {
                return fixed(&here);
            }
            let usage = storage::measure(&paths);
            Storage {
                disk: snob_store::disk::volume(paths.data_dir()),
                kinds: usage.kinds.iter().collect(),
                accounts: held_by_accounts(&paths, &here, &usage),
            }
        })
    }

    fn clear_cache(&self, only: Option<Pk>) -> Answer<Cleared> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            let cleared = storage::clear_cache(&paths, only);
            tracing::info!(
                freed = cleared.freed.bytes,
                whole = only.is_none(),
                "the cache was cleared"
            );
            if let Some((at, e)) = &cleared.failure {
                // Where, in the log alone, under the code it is logged as.
                let error = anyhow::anyhow!("{}: {e}", at.display())
                    .context("the cache could not be removed whole");
                log::failed(&error);
            }
            Cleared {
                freed: cleared.freed,
                whole: cleared.failure.is_none(),
            }
        })
    }

    fn profile_kept(&self, who: Named) -> Answer<Option<Profile>> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            let viewer = in_use(&paths)?;
            let shown = snob_engine::cache::shown(&paths, viewer, who.pk)?;
            Some(Profile {
                named: Named {
                    pk: who.pk,
                    username: shown.username.or(who.username),
                },
                full_name: shown.full_name,
                biography: shown.biography,
                external_url: shown.external_url,
                category: shown.category,
                is_private: shown.is_private.unwrap_or(false),
                is_verified: shown.is_verified.unwrap_or(false),
                followers: shown.followers,
                following: shown.following,
                posts: shown.posts,
                story_up: false,
                picture: snob_engine::cache::picture(&paths, viewer, who.pk),
                highlights: snob_engine::cache::tray(&paths, viewer, who.pk)
                    .map(|tray| {
                        tray.highlights
                            .into_iter()
                            .map(|highlight| Highlight {
                                cover: snob_engine::cache::drawing(
                                    &paths,
                                    viewer,
                                    Drawing::Cover,
                                    &highlight.id,
                                ),
                                id: highlight.id,
                                title: highlight.title.unwrap_or_default(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                visible: None,
                you_follow: shown.you_follow,
                follows_you: shown.follows_you,
                read: false,
            })
        })
    }

    fn profile(&self, who: Named) -> Answer<Result<Profile, Unread>> {
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        let addresses = Arc::clone(&self.addresses);
        self.runtime.run(move || async move {
            use snob_engine::profile::{self, MutualPolicy, Reads, Visibility};
            let (viewer, app) = opened(&paths, &secrets, None, None)?;
            let username = who.username.clone().ok_or(Unread::Nobody)?;
            // `snob profile`'s own read, less the stories.
            let reads = Reads {
                mutual: MutualPolicy::Defer,
                stories: false,
            };
            let read = profile::read(&app, &username, reads, Some(who.pk))
                .await
                .map_err(|e| unread(&paths, viewer, &e))?;
            // The default avatar is no picture to fetch large, as `snob pfp`
            // says of it.
            if !read.pfp_anonymous {
                addresses
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(
                        read.pk,
                        Largest {
                            username: read.username.clone(),
                            fallback: read.pfp_url.clone(),
                            full_size: read.pfp_full_size.clone(),
                        },
                    );
            }
            let keeper = app.cache();
            let picture = match snob_engine::cache::picture(&paths, viewer, read.pk) {
                Some(kept) => Some(kept),
                None if read.pfp_anonymous => None,
                None => match (&keeper, read.pfp_small.as_deref()) {
                    (Some(keeper), Some(address)) => {
                        keeper.fetch_picture(app.client(), read.pk, address).await
                    }
                    _ => None,
                },
            };
            let visible = matches!(read.highlights, Visibility::Shown(_));
            let mut highlights = Vec::new();
            if let Visibility::Shown(tray) = read.highlights {
                for highlight in tray {
                    let cover = match (&keeper, highlight.cover.as_deref()) {
                        (Some(keeper), Some(address)) => {
                            keeper
                                .fetch_drawing(app.client(), Drawing::Cover, &highlight.id, address)
                                .await
                        }
                        _ => None,
                    };
                    highlights.push(Highlight {
                        id: highlight.id,
                        title: highlight.title,
                        cover,
                    });
                }
            }
            Ok(Profile {
                named: Named {
                    pk: read.pk,
                    username: Some(read.username).filter(|name| !name.is_empty()),
                },
                full_name: read.full_name,
                biography: read.biography,
                external_url: read.external_url,
                category: read.category,
                is_private: read.is_private,
                is_verified: read.is_verified,
                followers: read.followers,
                following: read.following,
                posts: read.posts,
                story_up: read.story_up,
                picture,
                highlights,
                visible: Some(visible),
                you_follow: read.relation.map(|relation| relation.you_follow),
                follows_you: read.relation.map(|relation| relation.follows_you),
                read: true,
            })
        })
    }

    fn tiles(&self, who: Named, tab: Tab) -> Answer<Result<Tiles, Unread>> {
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        let held = Arc::clone(&self.cursors);
        let era = cursors(&held).era;
        self.runtime.run(move || async move {
            let (viewer, app) = opened(&paths, &secrets, None, None)?;
            let username = who.username.clone().ok_or(Unread::Nobody)?;
            let grid = snob_engine::posts::first_page(&app, &username, who.pk, profile_tab(tab))
                .await
                .map_err(|e| unread(&paths, viewer, &e))?;
            let tiles = tiles_of(&app, &grid).await;
            let whose = Whose {
                viewer,
                pk: who.pk,
                tab,
            };
            let next = grid
                .after()
                .and_then(|after| cursors(&held).hand_out(era, whose, after));
            Ok(Tiles {
                tiles,
                more: grid.next.is_some(),
                next,
            })
        })
    }

    fn more_tiles(
        &self,
        who: Named,
        tab: Tab,
        after: Cursor,
        resting: watch::Sender<Option<Epoch>>,
        stop: CancelToken,
    ) -> Answer<Result<Tiles, Unread>> {
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        let held = Arc::clone(&self.cursors);
        self.runtime.run(move || async move {
            let whose = Whose {
                viewer: in_use(&paths).ok_or(Unread::NoSession)?,
                pk: who.pk,
                tab,
            };
            let Some((mut at, era)) = cursors(&held).take(after, whose) else {
                // Spent by the page it led to, let go of, being read, or
                // handed to another account than the one in use: nothing
                // is sent from it, and no failure is told.
                tracing::debug!("a page was asked from a cursor not held for it");
                return Err(Unread::Failed(String::new()));
            };
            let (viewer, app) = match opened(&paths, &secrets, Some(stop.clone()), None) {
                Ok((viewer, _)) if viewer != whose.viewer => {
                    cursors(&held).put_back(era, after, whose, at);
                    return Err(Unread::Failed(String::new()));
                }
                Ok(opened) => opened,
                Err(why) => {
                    cursors(&held).put_back(era, after, whose, at);
                    return Err(why);
                }
            };
            // **The cursor as the page left it**, a navigation to the tab
            // already sent included, whether the page came or not.
            let read = snob_engine::posts::page_after(&app, &mut at, |left| {
                resting.send_replace(Some(rest_ends(left)));
            })
            .await;
            resting.send_replace(None);
            match read {
                Ok((grid, next)) => {
                    let tiles = tiles_of(&app, &grid).await;
                    let next = next.and_then(|next| cursors(&held).hand_out(era, whose, next));
                    Ok(Tiles {
                        tiles,
                        more: grid.next.is_some(),
                        next,
                    })
                }
                Err(e) => {
                    cursors(&held).put_back(era, after, whose, at);
                    if stop.is_canceled() {
                        return Err(Unread::Failed(String::new()));
                    }
                    Err(unread(&paths, viewer, &e))
                }
            }
        })
    }

    fn known_accounts(&self, typed: String, limit: usize) -> Answer<Vec<Account>> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            let Some(viewer) = in_use(&paths) else {
                return Vec::new();
            };
            let known = match snob_engine::cache::known_named(
                &paths,
                viewer,
                &typed,
                limit.min(MOST_KNOWN),
            ) {
                Ok(known) => known,
                Err(e) => {
                    log::failed(&e.context("the accounts known could not be read"));
                    return Vec::new();
                }
            };
            known
                .into_iter()
                .map(|known| {
                    let kept_at = known
                        .picture
                        .as_ref()
                        .and_then(|picture| std::fs::metadata(picture).ok())
                        .and_then(|file| file.modified().ok());
                    Account {
                        named: Named {
                            pk: known.pk,
                            username: Some(known.username).filter(|name| !name.is_empty()),
                        },
                        full_name: known.full_name,
                        biography: None,
                        picture: known.picture,
                        picture_kept_at: kept_at,
                        active: known.pk == viewer,
                    }
                })
                .collect()
        })
    }

    fn list_kept(&self, owner: Named, side: Side) -> Answer<Option<Listed>> {
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        let fixed = self.fixed_scan();
        self.runtime.run(move || async move {
            use snob_core::model::ListKind;
            let kind = match side {
                Side::Followers => ListKind::Followers,
                Side::Following => ListKind::Following,
            };
            let viewer = in_use(&paths)?;
            let account = paths.account(viewer);
            // Not `opened`: what was stored is served in a pause too, as
            // `--offline` serves it.
            let opened = snob_engine::app::App::open(
                &secrets.session_of(&account),
                &account,
                host::Front::quiet,
            );
            let mut app = match opened {
                Ok(Some(app)) => app,
                Ok(None) => return None,
                Err(e) => {
                    tracing::debug!(error = %e, "a stored list could not be read");
                    return None;
                }
            };
            let target = if owner.pk == viewer {
                None
            } else {
                Some(owner.username.clone()?)
            };
            let query = snob_engine::engine::ListQuery {
                target,
                // **From storage alone**, which asks nobody anything: no yes
                // is given here for a list to be walked.
                yes: false,
                refresh: false,
                cache: true,
                max_age: snob_engine::engine::DEFAULT_MAX_AGE,
                no_resume: false,
                max_pages: None,
                over_budget: None,
                walk_at_most_every: None,
            };
            let (users, outcome) = match snob_engine::engine::list(&mut app, &query, kind).await {
                Ok(served) => served,
                // Nothing whole stored is the usual answer, and no failure.
                Err(e) => {
                    tracing::debug!(error = %e, "no stored list was served");
                    return None;
                }
            };
            // A name stored as one account's and now another's.
            if outcome.account_pk != owner.pk {
                return None;
            }
            let people = users
                .iter()
                .map(|user| person(user, snob_engine::cache::picture(&paths, viewer, user.pk)))
                .collect();
            Some(Listed {
                people,
                whole: outcome.is_complete(),
                taken_at: fixed.unwrap_or(outcome.taken_at),
            })
        })
    }

    fn pictures(
        &self,
        wanted: Vec<(Pk, String)>,
        came: mpsc::UnboundedSender<(Pk, PathBuf, bool)>,
        stop: CancelToken,
    ) -> Answer<()> {
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        self.runtime.run(move || async move {
            let Some(viewer) = in_use(&paths) else {
                return;
            };
            let mut fetch = Vec::new();
            // The rest are not looked at, as the trait says.
            for (pk, address) in wanted.into_iter().take(MOST_PICTURES) {
                match snob_engine::cache::picture(&paths, viewer, pk) {
                    Some(kept) => {
                        if came.send((pk, kept, false)).is_err() {
                            return;
                        }
                    }
                    None if !address.is_empty() => fetch.push((pk, address)),
                    None => {}
                }
            }
            if fetch.is_empty() || stop.is_canceled() {
                return;
            }
            let Ok((_, app)) = opened(&paths, &secrets, Some(stop.clone()), None) else {
                return;
            };
            let Some(keeper) = app.cache() else {
                return;
            };
            for some in fetch.chunks(AT_ONCE) {
                if stop.is_canceled() || came.is_closed() {
                    return;
                }
                let one = |at: usize| {
                    let (keeper, client) = (&keeper, app.client());
                    async move {
                        let (pk, address) = some.get(at)?;
                        let kept = keeper.fetch_picture(client, *pk, address).await?;
                        Some((*pk, kept))
                    }
                };
                let (first, second, third) = tokio::join!(one(0), one(1), one(2));
                for (pk, kept) in [first, second, third].into_iter().flatten() {
                    if came.send((pk, kept, true)).is_err() {
                        return;
                    }
                }
            }
        })
    }

    fn tiles_kept(&self, who: Named, tab: Tab) -> Answer<Option<Tiles>> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            let viewer = in_use(&paths)?;
            let grid = snob_engine::cache::grid(&paths, viewer, who.pk, profile_tab(tab))?;
            let tiles = grid
                .posts
                .into_iter()
                .map(|post| {
                    let thumb =
                        snob_engine::cache::drawing(&paths, viewer, Drawing::Thumb, &post.code);
                    tile_of(post, thumb)
                })
                .collect();
            Some(Tiles {
                tiles,
                more: grid.more,
                next: None,
            })
        })
    }

    fn full_picture(&self, who: Named) -> Answer<Option<PathBuf>> {
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        let largest = self
            .addresses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&who.pk)
            .cloned();
        self.runtime.run(move || async move {
            let viewer = in_use(&paths)?;
            let key = who.pk.to_string();
            if let Some(kept) = snob_engine::cache::drawing(&paths, viewer, Drawing::Largest, &key)
            {
                return Some(kept);
            }
            let small = || snob_engine::cache::picture(&paths, viewer, who.pk);
            let Some(largest) = largest else {
                return small();
            };
            let (_, app) = opened(&paths, &secrets, None, None).ok()?;
            // `snob pfp`'s own choice of picture.
            let picture = snob_engine::profile::picture(
                app.client(),
                who.pk,
                &largest.username,
                largest.fallback,
                largest.full_size,
            )
            .await;
            let kept = match (picture, app.cache()) {
                (Ok(picture), Some(keeper)) => {
                    keeper.drawing(Drawing::Largest, &key, &picture.bytes)
                }
                (Ok(_), None) => None,
                // Not the error's own words: they can hold the address.
                (Err(_), _) => {
                    tracing::debug!("the largest picture was not fetched");
                    None
                }
            };
            kept.or_else(small)
        })
    }

    fn picture_bytes(&self, who: Named) -> Answer<Option<Vec<u8>>> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            let viewer = in_use(&paths)?;
            std::fs::read(snob_engine::cache::largest(&paths, viewer, who.pk)?).ok()
        })
    }

    fn save_picture(&self, who: Named, folder: PathBuf) -> Answer<Option<PathBuf>> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            use std::io::Write as _;
            let viewer = in_use(&paths)?;
            let bytes = std::fs::read(snob_engine::cache::largest(&paths, viewer, who.pk)?).ok()?;
            let extension = snob_engine::media::extension_of(&bytes);
            let stem = who.username.as_deref().unwrap_or("picture");
            let (path, mut file) =
                match snob_engine::files::create_numbered(&folder, stem, extension) {
                    Ok(made) => made,
                    Err(e) => {
                        log::failed(&e);
                        return None;
                    }
                };
            match file.write_all(&bytes) {
                Ok(()) => Some(path),
                Err(e) => {
                    log::failed(&anyhow::Error::new(e).context("a picture could not be saved"));
                    let _ = std::fs::remove_file(&path);
                    None
                }
            }
        })
    }

    fn list(
        &self,
        owner: Named,
        side: Side,
        walked: watch::Sender<Walked>,
        stop: CancelToken,
    ) -> Answer<Result<Listed, Unread>> {
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        let fixed = self.fixed_scan();
        self.runtime.run(move || async move {
            use snob_core::model::ListKind;
            let kind = match side {
                Side::Followers => ListKind::Followers,
                Side::Following => ListKind::Following,
            };
            let viewer = in_use(&paths).ok_or(Unread::NoSession)?;
            let showing = Arc::new(Showing {
                account: paths.account(viewer),
                owner: owner.pk,
                kind,
                db: Mutex::new(None),
                walked: walked.clone(),
            });
            let progress: Arc<dyn host::Progress> = showing;
            let (viewer, mut app) = opened(&paths, &secrets, Some(stop), Some(progress))?;
            let own = owner.pk == viewer;
            let target = if own {
                None
            } else {
                Some(owner.username.clone().ok_or(Unread::Nobody)?)
            };
            let query = snob_engine::engine::ListQuery {
                target,
                // **The yes the app asked for before this was called**: never
                // given for another account's list without it.
                yes: true,
                refresh: false,
                cache: false,
                max_age: snob_engine::engine::DEFAULT_MAX_AGE,
                no_resume: false,
                max_pages: None,
                over_budget: None,
                walk_at_most_every: None,
            };
            let (users, outcome) = snob_engine::engine::list(&mut app, &query, kind)
                .await
                .map_err(|e| unread(&paths, viewer, &e))?;
            let people: Vec<Person> = users
                .iter()
                .map(|user| person(user, snob_engine::cache::picture(&paths, viewer, user.pk)))
                .collect();
            walked.send_modify(|shown| shown.people = people.clone());
            Ok(Listed {
                people,
                whole: outcome.is_complete(),
                taken_at: fixed.unwrap_or(outcome.taken_at),
            })
        })
    }

    fn forget_history(&self, only: Option<Pk>) -> Answer<Forgotten> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            let accounts = match only {
                Some(pk) => vec![pk],
                None => paths.account_dirs(),
            };
            let mut forgotten = Forgotten::default();
            for pk in accounts {
                match storage::forget_history(&paths, pk) {
                    Ok(forgot) => forgotten.freed += forgot.freed,
                    Err(storage::NotForgotten::Busy) => forgotten.busy += 1,
                    Err(storage::NotForgotten::Failed(e)) => {
                        log::failed(&e.context("an account's history could not be forgotten"));
                        forgotten.failed += 1;
                    }
                }
            }
            tracing::info!(
                freed = forgotten.freed.bytes,
                busy = forgotten.busy,
                failed = forgotten.failed,
                "history was forgotten"
            );
            forgotten
        })
    }

    fn automations(&self) -> Answer<Vec<Automation>> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            let kept = snob_engine::automations::all(&paths);
            for sentence in &kept.unreadable {
                tracing::warn!(why = %sentence, "an automation could not be read");
            }
            kept.automations
        })
    }

    fn automation_problems(&self, automation: Automation) -> Answer<Vec<Problem>> {
        let paths = self.paths.clone();
        self.runtime
            .run(move || async move { snob_engine::automations::problems(&paths, &automation) })
    }

    fn save_automation(&self, automation: Automation) -> Answer<Result<(), String>> {
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        self.runtime.run(move || async move {
            snob_engine::automations::save(&paths, &secrets, &automation).map_err(|e| {
                let why = e.to_string();
                log::failed(&e.context("an automation could not be kept"));
                why
            })
        })
    }

    fn remove_automation(&self, id: String) -> Answer<bool> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            snob_engine::automations::remove(&paths, &id).unwrap_or_else(|e| {
                log::failed(&e.context("an automation could not be removed"));
                false
            })
        })
    }

    fn new_automation_id(&self) -> Answer<String> {
        let paths = self.paths.clone();
        self.runtime
            .run(move || async move { snob_engine::automations::new_id(&paths) })
    }

    fn automation_runs(&self, id: String) -> Answer<Vec<Run>> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            snob_engine::automations::runs(&paths, &id, 50, snob_core::clock::now())
                .unwrap_or_default()
        })
    }

    fn next_automation_run(&self, automation: Automation) -> Answer<Next> {
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            snob_engine::automations::next_run(&paths, &automation, snob_core::clock::now())
                .map_or(Next::Never, |(next, _)| next)
        })
    }

    fn run_automation(&self, id: String) -> Answer<Result<(), String>> {
        // A test build's files are a sandbox's, and the `snob` it would start
        // knows nothing of it: nothing is started.
        if cfg!(feature = "testing") {
            return crate::backend::answered(Err(
                "a test build starts no run of an automation".to_string()
            ));
        }
        let paths = self.paths.clone();
        self.runtime.run(move || async move {
            snob_engine::automations::run_now(&paths, &id, &[]).map_err(|e| {
                let why = e.to_string();
                log::failed(&e.context("an automation could not be started"));
                why
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whose(viewer: u64, pk: u64, tab: Tab) -> Whose {
        Whose {
            viewer: Pk::new(viewer),
            pk: Pk::new(pk),
            tab,
        }
    }

    #[test]
    fn a_cursor_is_taken_by_whose_tab_it_is_alone_and_put_back_under_its_id() {
        let mut held = Cursors::<&str>::default();
        let mine = whose(1, 7, Tab::Tagged);
        let cursor = held.hand_out(held.era, mine, "page 2").unwrap();

        for other in [
            whose(2, 7, Tab::Tagged),
            whose(1, 8, Tab::Tagged),
            whose(1, 7, Tab::Posts),
        ] {
            assert_eq!(held.take(cursor, other), None, "{other:?}");
        }
        let (at, era) = held.take(cursor, mine).expect("still held");
        assert_eq!(at, "page 2");
        assert_eq!(held.take(cursor, mine), None, "out while its page is read");

        held.put_back(era, cursor, mine, "page 2, the tab opened");
        assert_eq!(
            held.take(cursor, mine).map(|(at, _)| at),
            Some("page 2, the tab opened"),
            "the same cursor, as its page left it"
        );
    }

    #[test]
    fn past_the_most_cursors_the_oldest_goes() {
        let mut held = Cursors::<usize>::default();
        let mine = whose(1, 7, Tab::Posts);
        let first = held.hand_out(held.era, mine, 0).unwrap();
        let second = held.hand_out(held.era, mine, 1).unwrap();
        for n in 2..=MOST_CURSORS {
            held.hand_out(held.era, mine, n);
        }
        assert_eq!(held.held.len(), MOST_CURSORS);
        assert_eq!(held.take(first, mine), None);
        assert_eq!(held.take(second, mine).map(|(at, _)| at), Some(1));
    }

    #[test]
    fn a_page_read_before_the_cursors_were_let_go_of_hands_nothing_back() {
        let mut held = Cursors::<&str>::default();
        let mine = whose(1, 7, Tab::Reposts);
        let era = held.era;
        let cursor = held.hand_out(era, mine, "page 2").unwrap();
        let other = held.hand_out(era, mine, "page 3").unwrap();
        let (at, taken_in) = held.take(cursor, mine).unwrap();

        held.let_go();
        assert_eq!(held.take(other, mine), None, "every cursor is forgotten");
        held.put_back(taken_in, cursor, mine, at);
        assert_eq!(held.take(cursor, mine), None, "nor is a page's put back");
        assert_eq!(held.hand_out(era, mine, "page 3"), None);
        assert!(held.hand_out(held.era, mine, "page 2").is_some());
    }
}
