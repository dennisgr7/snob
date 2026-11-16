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
use snob_engine::{browser, host, log, logout};
use snob_ig::pace::CancelToken;
use snob_store::paths::AppPaths;
use snob_store::registry::Registry;
use snob_store::secrets::SecretStore;
use snob_store::viewer::Card;
use tokio::sync::watch;

use super::{Account, Answer, Backend, Browsers, Loaded, SignIn, answered};
use crate::report;
use crate::runtime::Runtime;
use crate::state::flow::{Found, Method, Named, Update};
use crate::state::prefs::{Placement, Prefs, Resume};

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
}
