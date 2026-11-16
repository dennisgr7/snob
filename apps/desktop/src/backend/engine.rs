//! The engine, answering the views: what `snob login`, `snob logout` and
//! `snob account use` do, called in process (`snob_engine`), on the app's
//! runtime and never on the window's thread.
//!
//! **Nothing here is a second way to do what a command does.** A sign-in is
//! `snob_engine::login`, a sign-out `snob_engine::logout`, the account in use
//! the registry's; what this file adds is which of them to call and how to
//! say what came back to a screen.

use std::path::PathBuf;
use std::sync::Arc;

use snob_core::secret::Secret;
use snob_core::{Epoch, Pk};
use snob_engine::app::Viewer;
use snob_engine::login::{self, Agent, ByBrowser, Failure, Opened};
use snob_engine::{browser, host, log, logout, storage};
use snob_ig::pace::CancelToken;
use snob_store::paths::AppPaths;
use snob_store::registry::Registry;
use snob_store::secrets::SecretStore;
use snob_store::viewer::Card;
use tokio::sync::watch;

use super::{
    Account, AccountStorage, Answer, Backend, Browsers, Cleared, Forgotten, Loaded, SignIn,
    Storage, answered,
};
use crate::report;
use crate::runtime::Runtime;
use crate::state::flow::{Found, Method, Named, Update};
use crate::state::prefs::{Appearance, Placement, Prefs, Resume};

/// The engine, for the data directory `paths` is.
pub struct Engine {
    runtime: Arc<Runtime>,
    paths: AppPaths,
    secrets: SecretStore,
    detail: log::Detail,
    files: Arc<log::Files>,
    /// What the system is, for a diagnostic report.
    about: String,
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

    /// Says the same sizes on every run, whatever is on disk, so a picture
    /// of them is the same on every machine. **Only what is shown**:
    /// clearing the cache still removes it.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn fixing_storage(mut self) -> Self {
        self.fixed_storage = true;
        self
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
    let card = Card::load(&account).unwrap_or_default();
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
        full_name: said(card.full_name),
        biography: said(card.biography),
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
/// out from says of it, its picture included, when the browser is still
/// there (`viewer::look_again`): a check pushed back kept no card, and the
/// tab's document says it for nothing. **Not at any other time but the
/// day's check of the session**: a start does not open a browser for a
/// picture.
async fn done(paths: &AppPaths, secrets: &SecretStore, viewer: Viewer) -> SignIn {
    let account = paths.account(viewer.pk);
    let opened =
        snob_engine::app::App::open(&secrets.session_of(&account), &account, host::Front::quiet);
    match opened {
        Ok(Some(app)) => {
            snob_engine::viewer::look_again(app.client(), &account).await;
        }
        Ok(None) => {}
        Err(e) => tracing::debug!(error = %e, "the account's picture was not fetched"),
    }
    Prefs::change(paths, |prefs| {
        prefs.expired = prefs.expired.filter(|pk| *pk != viewer.pk);
        // A login is a check that answered.
        prefs.note_checked(viewer.pk, snob_core::clock::now());
    });
    // **The name the list of accounts keeps when the login's check did not
    // read one** (a push-back on it, a check skipped in a pause): what a
    // start would show, shown from the welcome on rather than once the
    // files are read again.
    let username = viewer.username.filter(|name| !name.is_empty()).or_else(|| {
        Registry::load(paths)
            .ok()?
            .get(viewer.pk)
            .map(|account| account.username.clone())
            .filter(|name| !name.is_empty())
    });
    SignIn::Done(account_of(paths, viewer.pk, username, true))
}

/// The store a login writes to: the keyring, or a private file where there
/// is none, as `snob login` settles it before anything is asked.
fn writable(secrets: &SecretStore) -> anyhow::Result<SecretStore> {
    let usable = secrets.probe_writable()?;
    Ok(secrets.clone().using(usable))
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
        let (paths, secrets) = (self.paths.clone(), self.secrets.clone());
        self.runtime.run(move || async move {
            tracing::info!(method = "browser", "signing in");
            let Some(browser) = browser::detect() else {
                return failed(&paths, None, &anyhow::Error::new(login::NoBrowser));
            };
            let renewing = renewing.map(|pk| Viewer {
                pk,
                username: Registry::load(&paths)
                    .ok()
                    .and_then(|registry| registry.get(pk).map(|a| a.username.clone()))
                    .filter(|name| !name.is_empty()),
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
}
