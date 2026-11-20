//! A backend that answers what a test told it to, at once or when the test
//! lets it: for the tests that drive the views with no engine under them.
//! What it was asked is kept, so a test reads back that a screen called what
//! it should and nothing else. **Only a `testing` build has it.**

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use snob_core::secret::Secret;
use snob_core::{Epoch, Pk};
use snob_ig::pace::CancelToken;
use tokio::sync::{mpsc, oneshot, watch};

use super::{
    Account, AccountStorage, Answer, Automation, Backend, Browsers, Cleared, Cursor, Forgotten,
    Highlight, Listed, Loaded, MOST_PICTURES, Next, Person, Problem, Profile, Run, SignIn, Storage,
    Tile, TileKind, Tiles, Unread, Walked, answered,
};
use crate::state::flow::{Found, Method, Named, Update};
use crate::state::prefs::{Appearance, Placement};
use crate::state::profile::{Side, Tab};

/// What the backend was asked, in order. **Never the session ID itself**:
/// only that one was handed over, and whether it was the one expected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    Load,
    SignInWith {
        expected: bool,
    },
    SignInInAWindow {
        renewing: Option<Pk>,
    },
    SignOut(Pk),
    SwitchTo(Pk),
    PausedUntil,
    CheckForUpdate,
    FetchUpdate,
    SetDetailed(bool),
    RememberMethod(Option<Method>),
    RememberWindow,
    RememberAppearance(Appearance),
    DiagnosticReport,
    CrashShown,
    Storage,
    ClearCache(Option<Pk>),
    ForgetHistory(Option<Pk>),
    RememberIntroduced(Pk),
    Profile(Pk),
    Tiles(Pk, Tab),
    /// A page past the first, by its number: the second is 2.
    MoreTiles(Pk, Tab, u32),
    /// The names known that begin with what was typed.
    KnownAccounts(String),
    /// The small pictures of these rows of a list.
    Pictures(Vec<Pk>),
    FullPicture(Pk),
    PictureBytes(Pk),
    SavePicture(Pk),
    List(Pk, Side),
    Automations,
    SaveAutomation(Automation),
    RemoveAutomation(String),
    NewAutomationId,
    AutomationRuns(String),
    RunAutomation(String),
}

type Held = Box<dyn FnOnce() + Send>;

struct State {
    accounts: Vec<Account>,
    expired: Option<Named>,
    /// The account whose session the next check finds dead.
    dies: Option<Named>,
    /// How many times the session was asked about.
    session_checks: usize,
    crash: Option<String>,
    /// The session ID a paste is expected to carry.
    session_id: String,
    browsers: Browsers,
    /// The browser the last paste was said to come from.
    sent_as: Option<String>,
    sign_in: SignIn,
    update: Update,
    /// The update checks left unanswered, kept so they stay that way.
    stalled: Option<Vec<oneshot::Sender<Update>>>,
    /// The progress a fetch reports before it answers, and whether it ends
    /// ready.
    fetch: (Vec<Option<u8>>, bool),
    paused_until: Option<Epoch>,
    detailed_until: Option<Epoch>,
    resume: Option<Method>,
    placed: Option<Placement>,
    asked: Vec<Asked>,
    /// Whether answers wait for [`Scripted::release`].
    holding: bool,
    /// Whether clearing and forgetting do.
    holding_work: bool,
    held: Vec<Held>,
    /// The token of the window login in flight, to see it stopped.
    window: Option<CancelToken>,
    storage: Storage,
    /// Whether forgetting finds a job of every account running.
    busy: bool,
    /// The accounts remembered as past the automations' introduction.
    introduced: Vec<Pk>,
    /// How many accounts each list holds; the people are invented.
    list_size: usize,
    /// The account whose profile answers as kept from the viewer.
    private: Option<Pk>,
    /// The highlights and first pages answered, which the cache keeps.
    kept_trays: HashMap<Pk, Vec<Highlight>>,
    kept_tiles: HashMap<(Pk, Tab), Tiles>,
    /// How many pages the posts tab has past its first: none unless a test
    /// says so.
    more_pages: u32,
    /// How many of the next pages past the first fail.
    more_fails: u32,
    /// Until when the next pages say they rest before they are read.
    rests_until: Option<Epoch>,
    /// Each cursor handed out, by its id: whose tab and which page it leads
    /// to.
    cursors: HashMap<u64, (Pk, Tab, u32)>,
    cursors_made: u64,
    /// The token of the last page past the first asked for.
    more_stop: Option<CancelToken>,
    /// The accounts a name typed is suggested from.
    known: Vec<Account>,
    /// The lists answered, and those a test said were stored before.
    kept_lists: HashMap<(Pk, Side), Listed>,
    /// The pictures a row's is downloaded as, by its pk.
    pictures: HashMap<Pk, PathBuf>,
    /// The automations kept, in the order they were kept.
    automations: Vec<Automation>,
    runs: Vec<Run>,
    next_run: Next,
    ids_made: u32,
    /// Why every save of an automation fails, when it does.
    save_fails: Option<String>,
}

/// The scripted backend. Clones are the same backend.
#[derive(Clone)]
pub struct Scripted(Arc<Mutex<State>>);

/// An account a script signs in.
pub fn account(pk: u64, username: &str, active: bool) -> Account {
    Account {
        named: Named {
            pk: pk.into(),
            username: Some(username.to_string()),
        },
        full_name: Some(format!("Invented {username}")),
        biography: Some(format!("the invented bio of {username}")),
        picture: None,
        picture_kept_at: None,
        active,
    }
}

impl Scripted {
    /// A backend with `accounts` signed in, the active one first.
    pub fn with(accounts: Vec<Account>) -> Self {
        Self(Arc::new(Mutex::new(State {
            accounts,
            expired: None,
            dies: None,
            session_checks: 0,
            crash: None,
            session_id: "42%3AAbCdEfGhIjKl%3A20".to_string(),
            browsers: Browsers {
                found: vec!["Chrome".to_string()],
                last: None,
            },
            sent_as: None,
            sign_in: SignIn::Done(account(42, "me", true)),
            update: Update::None,
            stalled: None,
            fetch: (vec![Some(0), Some(50), Some(100)], true),
            paused_until: None,
            detailed_until: None,
            resume: None,
            placed: None,
            asked: Vec::new(),
            holding: false,
            holding_work: false,
            held: Vec::new(),
            window: None,
            storage: Storage::default(),
            busy: false,
            introduced: Vec::new(),
            list_size: 30,
            private: None,
            kept_trays: HashMap::new(),
            kept_tiles: HashMap::new(),
            more_pages: 0,
            more_fails: 0,
            rests_until: None,
            cursors: HashMap::new(),
            cursors_made: 0,
            more_stop: None,
            known: Vec::new(),
            kept_lists: HashMap::new(),
            pictures: HashMap::new(),
            automations: Vec::new(),
            runs: Vec::new(),
            next_run: Next::Never,
            ids_made: 0,
            save_fails: None,
        })))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many times the session was asked about.
    pub fn session_checks(&self) -> usize {
        self.state().session_checks
    }

    /// The browser the last paste was said to come from.
    pub fn sent_as(&self) -> Option<String> {
        self.state().sent_as.clone()
    }

    /// Changes what the backend answers from now on.
    pub fn script(&self, change: impl FnOnce(&mut Script<'_>)) {
        let mut state = self.state();
        change(&mut Script(&mut state));
    }

    /// What it was asked so far.
    pub fn asked(&self) -> Vec<Asked> {
        self.state().asked.clone()
    }

    /// From now on an answer waits until [`Self::release`]: what a screen
    /// shows while it waits can then be looked at.
    pub fn hold(&self) {
        self.state().holding = true;
    }

    /// From now on clearing the cache, forgetting history, the end of a
    /// list's walk, a page past a tab's first and a list's pictures wait
    /// until [`Self::release`]; everything else is answered.
    pub fn hold_the_work(&self) {
        self.state().holding_work = true;
    }

    /// Lets every answer held go, and stops holding.
    pub fn release(&self) {
        let held = {
            let mut state = self.state();
            state.holding = false;
            state.holding_work = false;
            std::mem::take(&mut state.held)
        };
        for answer in held {
            answer();
        }
    }

    /// The automations kept now.
    pub fn automations(&self) -> Vec<Automation> {
        self.state().automations.clone()
    }

    /// Where the window was last said to be.
    pub fn window(&self) -> Option<Placement> {
        self.state().placed
    }

    /// Whether the window login in flight was told to stop.
    pub fn window_was_stopped(&self) -> bool {
        self.state()
            .window
            .as_ref()
            .is_some_and(CancelToken::is_canceled)
    }

    /// Whether the last page past a tab's first asked for was told to stop.
    pub fn more_stopped(&self) -> bool {
        self.state()
            .more_stop
            .as_ref()
            .is_some_and(CancelToken::is_canceled)
    }

    fn answer<T: Send + 'static>(
        &self,
        asked: Asked,
        value: impl FnOnce(&mut State) -> T + Send + 'static,
    ) -> Answer<T> {
        let (answer, answered) = oneshot::channel();
        let backend = self.clone();
        let send = move || {
            let value = value(&mut backend.state());
            let _ = answer.send(value);
        };
        let held: Option<Held> = {
            let mut state = self.state();
            let work = matches!(
                asked,
                Asked::ClearCache(_)
                    | Asked::ForgetHistory(_)
                    | Asked::List(..)
                    | Asked::MoreTiles(..)
                    | Asked::Pictures(_)
            );
            state.asked.push(asked);
            if state.holding || (state.holding_work && work) {
                state.held.push(Box::new(send));
                None
            } else {
                Some(Box::new(send))
            }
        };
        if let Some(send) = held {
            send();
        }
        answered
    }
}

/// What a test may change about a scripted backend.
pub struct Script<'a>(&'a mut State);

impl Script<'_> {
    pub fn accounts(&mut self, accounts: Vec<Account>) -> &mut Self {
        self.0.accounts = accounts;
        self
    }

    /// The account in use, as one whose session stopped working.
    pub fn expired(&mut self, account: Named) -> &mut Self {
        self.0.expired = Some(account);
        self
    }

    /// The browsers found installed, and the one chosen the last time.
    pub fn browsers(&mut self, found: &[&str], last: Option<&str>) -> &mut Self {
        self.0.browsers = Browsers {
            found: found.iter().map(|name| (*name).to_string()).collect(),
            last: last.map(str::to_string),
        };
        self
    }

    /// The next check of the session finds this account's dead.
    pub fn session_dies(&mut self, account: Named) -> &mut Self {
        self.0.dies = Some(account);
        self
    }

    pub fn crash(&mut self, report: &str) -> &mut Self {
        self.0.crash = Some(report.to_string());
        self
    }

    /// How the next sign-in ends.
    pub fn sign_in(&mut self, outcome: SignIn) -> &mut Self {
        self.0.sign_in = outcome;
        self
    }

    pub fn update(&mut self, update: Update) -> &mut Self {
        self.0.update = update;
        self
    }

    /// From now on the update check never answers.
    pub fn check_never_answers(&mut self) -> &mut Self {
        self.0.stalled = Some(Vec::new());
        self
    }

    pub fn fetch(&mut self, progress: Vec<Option<u8>>, ready: bool) -> &mut Self {
        self.0.fetch = (progress, ready);
        self
    }

    pub fn paused_until(&mut self, until: Option<Epoch>) -> &mut Self {
        self.0.paused_until = until;
        self
    }

    /// What is said to be kept: `kinds` by size in bytes, and each account
    /// signed in holding `each` of history and `each` of cache, on a disk of
    /// a thousand times what is kept, half of it free. Clearing the cache
    /// empties its kind and the accounts' cache; forgetting empties their
    /// history too.
    pub fn storage(&mut self, kinds: &[(snob_engine::storage::Kind, u64)], each: u64) -> &mut Self {
        let size = |bytes: u64| snob_store::disk::Size { bytes, files: 1 };
        let kept: u64 = kinds.iter().map(|(_, bytes)| bytes).sum();
        self.0.storage = Storage {
            disk: Some(snob_store::disk::Volume {
                total: kept * 1000,
                free: kept * 500,
            }),
            kinds: kinds
                .iter()
                .map(|(kind, bytes)| (*kind, size(*bytes)))
                .collect(),
            accounts: self
                .0
                .accounts
                .iter()
                .map(|account| AccountStorage {
                    pk: account.named.pk,
                    username: account.named.username.clone(),
                    signed_in: true,
                    history: size(each),
                    cache: size(each),
                })
                .collect(),
        };
        self
    }

    /// An account no longer signed in whose files are still here.
    pub fn left_behind(&mut self, pk: u64, username: &str, each: u64) -> &mut Self {
        let size = |bytes: u64| snob_store::disk::Size { bytes, files: 1 };
        self.0.storage.accounts.push(AccountStorage {
            pk: pk.into(),
            username: Some(username.to_string()),
            signed_in: false,
            history: size(each),
            cache: size(0),
        });
        self
    }

    /// Answers every forgetting as left alone, as while a job runs.
    pub fn busy(&mut self) -> &mut Self {
        self.0.busy = true;
        self
    }

    /// The method chosen before the restart this start comes after.
    pub fn resume(&mut self, method: Method) -> &mut Self {
        self.0.resume = Some(method);
        self
    }

    /// An account remembered as past the automations' introduction.
    pub fn introduced(&mut self, pk: u64) -> &mut Self {
        self.0.introduced.push(pk.into());
        self
    }
    /// How many accounts every list holds.
    pub fn list_size(&mut self, size: usize) -> &mut Self {
        self.0.list_size = size;
        self
    }

    /// The account `pk` is private, and the viewer does not follow it.
    pub fn private(&mut self, pk: u64) -> &mut Self {
        self.0.private = Some(pk.into());
        self
    }

    /// An automation already kept.
    pub fn automation(&mut self, automation: Automation) -> &mut Self {
        self.0.automations.push(automation);
        self
    }

    /// A run in the history of an automation, wherever it is put.
    pub fn run(&mut self, run: Run) -> &mut Self {
        self.0.runs.push(run);
        self
    }

    /// When the next run of any automation is said to be.
    pub fn next_run(&mut self, next: Next) -> &mut Self {
        self.0.next_run = next;
        self
    }

    /// From now on no automation can be kept, for this reason.
    pub fn save_fails(&mut self, why: &str) -> &mut Self {
        self.0.save_fails = Some(why.to_string());
        self
    }

    /// The posts tab has `pages` pages of twelve past its first, read on
    /// from the cursor each hands out. **None by default**: a first page
    /// hands out no cursor.
    pub fn more_pages(&mut self, pages: u32) -> &mut Self {
        self.0.more_pages = pages;
        self
    }

    /// The next `times` pages past a tab's first fail, leaving their cursor
    /// where it was.
    pub fn more_fails(&mut self, times: u32) -> &mut Self {
        self.0.more_fails = times;
        self
    }

    /// The next pages past a tab's first say they rest until `until` as
    /// soon as they are asked for, and stop saying so when they come.
    pub fn rests_until(&mut self, until: Option<Epoch>) -> &mut Self {
        self.0.rests_until = until;
        self
    }

    /// The accounts a name typed is suggested from, in the order they are
    /// suggested in.
    pub fn known(&mut self, accounts: Vec<Account>) -> &mut Self {
        self.0.known = accounts;
        self
    }

    /// `pk`'s `side` list was read before, `size` accounts long, and is
    /// stored.
    pub fn kept_list(&mut self, pk: u64, side: Side, size: usize) -> &mut Self {
        self.0
            .kept_lists
            .insert((pk.into(), side), kept(people(size)));
        self
    }

    /// The small picture of the row `pk` is downloaded as `picture`.
    pub fn picture(&mut self, pk: u64, picture: PathBuf) -> &mut Self {
        self.0.pictures.insert(pk.into(), picture);
        self
    }
}

/// When every scripted list is said to have been read.
const SCANNED_AT: Epoch = Epoch::new(1_790_000_000);

/// A list stored whole, of `people`.
fn kept(people: Vec<Person>) -> Listed {
    Listed {
        people,
        whole: true,
        stored_at: Some(SCANNED_AT),
        taken_at: SCANNED_AT,
    }
}

/// The invented profile of `who`: an account signed in here says its own
/// name and bio, `someone` (9001) has a story up and three highlights.
pub fn profile_of(who: &Named, accounts: &[Account], private: Option<Pk>) -> Profile {
    let username = who.username.clone().unwrap_or_else(|| who.pk.to_string());
    let here = accounts.iter().find(|account| account.named.pk == who.pk);
    let hidden = private == Some(who.pk);
    let story_up = who.pk == Pk::new(9001);
    let highlights = if hidden {
        Vec::new()
    } else {
        ["Trips", "Food", "35mm"]
            .into_iter()
            .take(if story_up { 3 } else { 2 })
            .enumerate()
            .map(|(at, title)| Highlight {
                id: format!("1790000000000000{at}"),
                title: title.to_string(),
                cover: None,
            })
            .collect()
    };
    Profile {
        named: Named {
            pk: who.pk,
            username: Some(username.clone()),
        },
        full_name: here
            .and_then(|account| account.full_name.clone())
            .or_else(|| Some(format!("Invented {username}"))),
        biography: here
            .and_then(|account| account.biography.clone())
            .or_else(|| Some(format!("the invented bio of {username}"))),
        external_url: None,
        category: None,
        is_private: hidden,
        is_verified: false,
        followers: Some(1_234),
        following: Some(567),
        posts: Some(48),
        story_up,
        picture: here.and_then(|account| account.picture.clone()),
        highlights,
        visible: Some(!hidden),
        you_follow: here.is_none().then_some(!hidden),
        follows_you: here.is_none().then_some(true),
        read: true,
    }
}

/// The invented accounts of a list `size` long.
fn people(size: usize) -> Vec<Person> {
    (0..size)
        .map(|n| Person {
            named: Named {
                pk: Pk::new(1000 + n as u64),
                username: Some(format!("user{n}")),
            },
            full_name: Some(format!("Invented user{n}")),
            is_private: n % 7 == 3,
            is_verified: false,
            picture: None,
            picture_from: Some(format!(
                "https://scontent.cdninstagram.com/v/invented/{n}.jpg"
            )),
        })
        .collect()
}

/// The invented posts `from` to `to` of a tab, numbered from its newest.
fn posts(tab: Tab, from: usize, to: usize) -> Vec<Tile> {
    let kinds = [
        TileKind::Photo,
        TileKind::Carousel(3),
        TileKind::Reel,
        TileKind::Video,
    ];
    (from..to)
        .map(|n| Tile {
            code: format!("C{n:02}invented"),
            kind: kinds[n % kinds.len()],
            thumb: None,
            caption: Some(format!("an invented caption, number {n}")),
            owner: (tab != Tab::Posts).then(|| format!("user{n}")),
            taken_at: Some(Epoch::new(1_790_000_000 - n as i64 * 86_400)),
            likes: Some(100 + n as u64 * 7),
            comments: Some(n as u64),
            pinned: tab == Tab::Posts && n == 0,
        })
        .collect()
}

/// The invented first page of a tab: twelve posts, more past them.
fn tiles(tab: Tab) -> Tiles {
    let count = match tab {
        Tab::Posts => 12,
        Tab::Reposts => 5,
        Tab::Tagged => 0,
    };
    Tiles {
        tiles: posts(tab, 0, count),
        more: count == 12,
        next: None,
    }
}

/// A cursor to `pk`'s `tab` at `page`, held by `state`.
fn hand_out(state: &mut State, pk: Pk, tab: Tab, page: u32) -> Cursor {
    state.cursors_made += 1;
    state.cursors.insert(state.cursors_made, (pk, tab, page));
    Cursor(state.cursors_made)
}

/// What a sign-in that ended well leaves signed in.
fn signed_in(state: &mut State, outcome: &SignIn) {
    if let SignIn::Done(account) = outcome {
        state.expired = None;
        for other in &mut state.accounts {
            other.active = false;
        }
        state.accounts.retain(|a| a.named.pk != account.named.pk);
        state.accounts.insert(0, account.clone());
    }
}

impl Backend for Scripted {
    fn load(&self) -> Answer<Loaded> {
        self.answer(Asked::Load, |state| Loaded {
            found: match (&state.expired, state.accounts.first()) {
                (Some(expired), _) => Found::Expired(expired.clone()),
                (None, Some(active)) => Found::SignedIn(active.named.clone()),
                (None, None) => Found::Nobody,
            },
            accounts: state.accounts.clone(),
            crash: state.crash.clone(),
            resume: state.resume,
            introduced: state.introduced.clone(),
        })
    }

    // Not among what was asked, as the session's check is not.
    fn browsers(&self) -> Answer<Browsers> {
        answered(self.state().browsers.clone())
    }

    fn sign_in_with(&self, session_id: Secret, browser: Option<String>) -> Answer<SignIn> {
        self.state().cursors.clear();
        let expected = session_id.expose() == self.state().session_id;
        self.state().sent_as = browser;
        self.answer(Asked::SignInWith { expected }, |state| {
            let outcome = state.sign_in.clone();
            signed_in(state, &outcome);
            outcome
        })
    }

    fn sign_in_in_a_window(&self, renewing: Option<Pk>, stop: CancelToken) -> Answer<SignIn> {
        self.state().cursors.clear();
        self.state().window = Some(stop.clone());
        self.answer(Asked::SignInInAWindow { renewing }, move |state| {
            if stop.is_canceled() {
                return SignIn::GaveUp;
            }
            let outcome = state.sign_in.clone();
            signed_in(state, &outcome);
            outcome
        })
    }

    fn sign_out(&self, pk: Pk) -> Answer<Vec<Account>> {
        self.state().cursors.clear();
        self.answer(Asked::SignOut(pk), move |state| {
            state.accounts.retain(|account| account.named.pk != pk);
            if let Some(first) = state.accounts.first_mut() {
                first.active = true;
            }
            state.accounts.clone()
        })
    }

    fn switch_to(&self, pk: Pk) -> Answer<Vec<Account>> {
        self.state().cursors.clear();
        self.answer(Asked::SwitchTo(pk), move |state| {
            if state.accounts.iter().any(|account| account.named.pk == pk) {
                for account in &mut state.accounts {
                    account.active = account.named.pk == pk;
                }
                state.accounts.sort_by_key(|account| !account.active);
            }
            state.accounts.clone()
        })
    }

    fn paused_until(&self) -> Answer<Option<Epoch>> {
        self.answer(Asked::PausedUntil, |state| state.paused_until)
    }

    // Not among what was asked: a view may ask it at any time, and a test
    // counts it apart (`session_checks`).
    fn expired_session(&self) -> Answer<Option<Named>> {
        let mut state = self.state();
        state.session_checks += 1;
        let dead = state.dies.take();
        if dead.is_some() {
            state.expired = dead.clone();
        }
        answered(dead)
    }

    fn remember_method(&self, method: Option<Method>) {
        let mut state = self.state();
        state.asked.push(Asked::RememberMethod(method));
        state.resume = method;
    }

    fn remember_window(&self, placement: Placement) {
        let mut state = self.state();
        state.asked.push(Asked::RememberWindow);
        state.placed = Some(placement);
    }

    fn remember_appearance(&self, appearance: Appearance) {
        self.state()
            .asked
            .push(Asked::RememberAppearance(appearance));
    }

    fn remember_introduced(&self, pk: Pk) {
        let mut state = self.state();
        state.asked.push(Asked::RememberIntroduced(pk));
        if !state.introduced.contains(&pk) {
            state.introduced.push(pk);
        }
    }

    fn storage(&self) -> Answer<Storage> {
        self.answer(Asked::Storage, |state| state.storage.clone())
    }

    fn clear_cache(&self, only: Option<Pk>) -> Answer<Cleared> {
        self.answer(Asked::ClearCache(only), move |state| {
            let mut freed = snob_store::disk::Size::default();
            for account in &mut state.storage.accounts {
                if only.is_none_or(|pk| pk == account.pk) {
                    freed += std::mem::take(&mut account.cache);
                }
            }
            for (kind, size) in &mut state.storage.kinds {
                if kind.clearable() {
                    // All of it, the cache no account holds included.
                    if only.is_none() {
                        freed = std::mem::take(size);
                    } else {
                        size.bytes = size.bytes.saturating_sub(freed.bytes);
                    }
                }
            }
            Cleared { freed, whole: true }
        })
    }

    fn forget_history(&self, only: Option<Pk>) -> Answer<Forgotten> {
        self.answer(Asked::ForgetHistory(only), move |state| {
            let mut forgotten = Forgotten::default();
            for account in &mut state.storage.accounts {
                if only.is_some_and(|pk| pk != account.pk) {
                    continue;
                }
                if state.busy {
                    forgotten.busy += 1;
                    continue;
                }
                forgotten.freed += std::mem::take(&mut account.history);
                forgotten.freed += std::mem::take(&mut account.cache);
            }
            for (kind, size) in &mut state.storage.kinds {
                if matches!(
                    kind,
                    snob_engine::storage::Kind::Databases
                        | snob_engine::storage::Kind::Jobs
                        | snob_engine::storage::Kind::Cache
                ) {
                    size.bytes = size.bytes.saturating_sub(forgotten.freed.bytes);
                }
            }
            forgotten
        })
    }

    // Not among what was asked: it reads files alone.
    fn profile_kept(&self, who: Named) -> Answer<Option<Profile>> {
        let state = self.state();
        let kept = state
            .accounts
            .iter()
            .any(|account| account.named.pk == who.pk)
            .then(|| Profile {
                read: false,
                story_up: false,
                highlights: state.kept_trays.get(&who.pk).cloned().unwrap_or_default(),
                visible: None,
                ..profile_of(&who, &state.accounts, state.private)
            });
        answered(kept)
    }

    fn profile(&self, who: Named) -> Answer<Result<Profile, Unread>> {
        self.answer(Asked::Profile(who.pk), move |state| {
            let profile = profile_of(&who, &state.accounts, state.private);
            state.kept_trays.insert(who.pk, profile.highlights.clone());
            Ok(profile)
        })
    }

    fn tiles(&self, who: Named, tab: Tab) -> Answer<Result<Tiles, Unread>> {
        self.answer(Asked::Tiles(who.pk, tab), move |state| {
            if state.private == Some(who.pk) {
                return Err(Unread::Hidden);
            }
            let mut tiles = tiles(tab);
            state.kept_tiles.insert((who.pk, tab), tiles.clone());
            if tab == Tab::Posts && state.more_pages > 0 {
                tiles.next = Some(hand_out(state, who.pk, tab, 2));
            }
            Ok(tiles)
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
        let page = {
            let mut state = self.state();
            state.more_stop = Some(stop.clone());
            if let Some(until) = state.rests_until {
                resting.send_replace(Some(until));
            }
            state
                .cursors
                .get(&after.0)
                .filter(|(pk, on, _)| *pk == who.pk && *on == tab)
                .map(|(_, _, page)| *page)
        };
        // A cursor no longer held is asked as page 0, which reads nothing.
        let asked = Asked::MoreTiles(who.pk, tab, page.unwrap_or(0));
        self.answer(asked, move |state| {
            resting.send_replace(None);
            let Some(page) = page.filter(|_| state.cursors.contains_key(&after.0)) else {
                return Err(Unread::Failed(String::new()));
            };
            if stop.is_canceled() {
                return Err(Unread::Failed(String::new()));
            }
            if state.more_fails > 0 {
                state.more_fails -= 1;
                return Err(Unread::Failed("7K2Q-PAGE".to_string()));
            }
            state.cursors.remove(&after.0);
            let at = (page as usize - 1) * 12;
            let more = page <= state.more_pages;
            Ok(Tiles {
                tiles: posts(tab, at, at + 12),
                more,
                next: more.then(|| hand_out(state, who.pk, tab, page + 1)),
            })
        })
    }

    fn known_accounts(&self, typed: String, limit: usize) -> Answer<Vec<Account>> {
        self.answer(Asked::KnownAccounts(typed.clone()), move |state| {
            let typed = typed.trim();
            let typed = typed.strip_prefix('@').unwrap_or(typed).to_lowercase();
            if typed.is_empty() {
                return Vec::new();
            }
            state
                .known
                .iter()
                .filter(|account| {
                    account
                        .named
                        .username
                        .as_deref()
                        .is_some_and(|name| name.to_lowercase().starts_with(&typed))
                })
                .take(limit)
                .cloned()
                .collect()
        })
    }

    // Not among what was asked: it reads files alone.
    fn list_kept(&self, owner: Named, side: Side) -> Answer<Option<Listed>> {
        answered(self.state().kept_lists.get(&(owner.pk, side)).cloned())
    }

    fn pictures(
        &self,
        wanted: Vec<(Pk, String)>,
        came: mpsc::UnboundedSender<(Pk, PathBuf, bool)>,
        stop: CancelToken,
    ) -> Answer<()> {
        let rows = wanted.iter().map(|(pk, _)| *pk).collect();
        self.answer(Asked::Pictures(rows), move |state| {
            for (pk, _) in wanted.into_iter().take(MOST_PICTURES) {
                if stop.is_canceled() {
                    return;
                }
                if let Some(picture) = state.pictures.get(&pk)
                    && came.send((pk, picture.clone(), true)).is_err()
                {
                    return;
                }
            }
        })
    }

    // Not among what was asked: it reads files alone.
    fn tiles_kept(&self, who: Named, tab: Tab) -> Answer<Option<Tiles>> {
        answered(self.state().kept_tiles.get(&(who.pk, tab)).cloned())
    }

    fn full_picture(&self, who: Named) -> Answer<Option<PathBuf>> {
        self.answer(Asked::FullPicture(who.pk), move |state| {
            state
                .accounts
                .iter()
                .find(|account| account.named.pk == who.pk)
                .and_then(|account| account.picture.clone())
        })
    }

    fn picture_bytes(&self, who: Named) -> Answer<Option<Vec<u8>>> {
        self.answer(Asked::PictureBytes(who.pk), |_| {
            Some(b"\x89PNG invented".to_vec())
        })
    }

    fn save_picture(&self, who: Named, folder: PathBuf) -> Answer<Option<PathBuf>> {
        let name = format!("{}.jpg", who.username.as_deref().unwrap_or("picture"));
        self.answer(Asked::SavePicture(who.pk), move |_| Some(folder.join(name)))
    }

    fn list(
        &self,
        owner: Named,
        side: Side,
        walked: watch::Sender<Walked>,
        stop: CancelToken,
    ) -> Answer<Result<Listed, Unread>> {
        let all = people(self.state().list_size);
        let expected = Some(all.len() as u64);
        // The first page at once; the rest, and the end, when the answer is
        // let go.
        let first: Vec<Person> = all.iter().take(12).cloned().collect();
        let _ = walked.send(Walked {
            people: first,
            expected,
        });
        self.answer(Asked::List(owner.pk, side), move |state| {
            if state.private == Some(owner.pk) {
                return Err(Unread::Hidden);
            }
            let whole = !stop.is_canceled();
            let _ = walked.send(Walked {
                people: all.clone(),
                expected,
            });
            if whole {
                state.kept_lists.insert((owner.pk, side), kept(all.clone()));
            }
            Ok(Listed {
                people: all,
                whole,
                stored_at: None,
                taken_at: SCANNED_AT,
            })
        })
    }

    fn check_for_update(&self) -> Answer<Update> {
        {
            let mut state = self.state();
            if let Some(stalled) = &mut state.stalled {
                let (answer, answered) = oneshot::channel();
                stalled.push(answer);
                state.asked.push(Asked::CheckForUpdate);
                return answered;
            }
        }
        self.answer(Asked::CheckForUpdate, |state| state.update)
    }

    fn fetch_update(&self, progress: watch::Sender<Option<u8>>) -> Answer<bool> {
        self.answer(Asked::FetchUpdate, move |state| {
            for percent in &state.fetch.0 {
                let _ = progress.send(*percent);
            }
            state.fetch.1
        })
    }

    fn detailed_until(&self) -> Option<Epoch> {
        self.state().detailed_until
    }

    fn set_detailed(&self, on: bool) {
        let mut state = self.state();
        state.asked.push(Asked::SetDetailed(on));
        state.detailed_until = on.then(|| Epoch::new(snob_core::clock::now().get() + 24 * 60 * 60));
    }

    fn logs_dir(&self) -> PathBuf {
        PathBuf::from("logs")
    }

    fn diagnostic_report(&self) -> Answer<String> {
        self.answer(Asked::DiagnosticReport, |_| {
            "snob desktop: diagnostic report\n(a scripted one)\n".to_string()
        })
    }

    fn crash_shown(&self) {
        let mut state = self.state();
        state.asked.push(Asked::CrashShown);
        state.crash = None;
    }

    fn automations(&self) -> Answer<Vec<Automation>> {
        self.answer(Asked::Automations, |state| state.automations.clone())
    }

    fn automation_problems(&self, automation: Automation) -> Answer<Vec<Problem>> {
        answered(automation.problems())
    }

    fn save_automation(&self, automation: Automation) -> Answer<Result<(), String>> {
        self.answer(Asked::SaveAutomation(automation.clone()), move |state| {
            if let Some(why) = &state.save_fails {
                return Err(why.clone());
            }
            match state
                .automations
                .iter_mut()
                .find(|kept| kept.id == automation.id)
            {
                Some(kept) => *kept = automation,
                None => state.automations.push(automation),
            }
            Ok(())
        })
    }

    fn remove_automation(&self, id: String) -> Answer<bool> {
        self.answer(Asked::RemoveAutomation(id.clone()), move |state| {
            let before = state.automations.len();
            state.automations.retain(|kept| kept.id != id);
            before != state.automations.len()
        })
    }

    fn new_automation_id(&self) -> Answer<String> {
        self.answer(Asked::NewAutomationId, |state| {
            state.ids_made += 1;
            format!("{:08x}", 0xa0_0000 + state.ids_made)
        })
    }

    fn automation_runs(&self, id: String) -> Answer<Vec<Run>> {
        self.answer(Asked::AutomationRuns(id.clone()), move |state| {
            state
                .runs
                .iter()
                .filter(|run| run.automation == id)
                .cloned()
                .collect()
        })
    }

    fn next_automation_run(&self, _: Automation) -> Answer<Next> {
        answered(self.state().next_run)
    }

    fn run_automation(&self, id: String) -> Answer<Result<(), String>> {
        self.answer(Asked::RunAutomation(id), |_| Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_engine::login::Failure;

    #[test]
    fn it_answers_what_it_was_told_to_and_remembers_what_it_was_asked() {
        let backend = Scripted::with(Vec::new());
        assert_eq!(backend.load().blocking_recv().unwrap().found, Found::Nobody);

        backend.script(|script| {
            script.sign_in(SignIn::Failed {
                why: Failure::Rejected,
                id: "7K2Q-9FMX".into(),
                until: None,
            });
        });
        let failed = backend
            .sign_in_with(Secret::new("not the one".to_string()), None)
            .blocking_recv()
            .unwrap();
        assert!(matches!(failed, SignIn::Failed { .. }));

        backend.script(|script| {
            script.sign_in(SignIn::Done(account(42, "me", true)));
        });
        let done = backend
            .sign_in_with(Secret::new("42%3AAbCdEfGhIjKl%3A20".to_string()), None)
            .blocking_recv()
            .unwrap();
        assert_eq!(done, SignIn::Done(account(42, "me", true)));
        assert_eq!(
            backend.load().blocking_recv().unwrap().found,
            Found::SignedIn(account(42, "me", true).named)
        );

        assert_eq!(
            backend.asked(),
            [
                Asked::Load,
                Asked::SignInWith { expected: false },
                Asked::SignInWith { expected: true },
                Asked::Load,
            ]
        );
    }

    #[test]
    fn an_answer_held_arrives_only_once_it_is_let_go() {
        let backend = Scripted::with(vec![account(42, "me", true)]);
        backend.hold();
        let mut answer = backend.check_for_update();
        assert!(answer.try_recv().is_err(), "still held");
        backend.release();
        assert_eq!(answer.try_recv().unwrap(), Update::None);
    }

    #[test]
    fn a_window_login_stopped_ends_as_given_up() {
        let backend = Scripted::with(Vec::new());
        backend.hold();
        let stop = CancelToken::default();
        let mut answer = backend.sign_in_in_a_window(None, stop.clone());
        stop.cancel();
        assert!(backend.window_was_stopped());
        backend.release();
        assert_eq!(answer.try_recv().unwrap(), SignIn::GaveUp);
    }

    fn someone() -> Named {
        Named {
            pk: 9001.into(),
            username: Some("someone".into()),
        }
    }

    fn more(backend: &Scripted, after: Cursor) -> Result<Tiles, Unread> {
        let (resting, _) = watch::channel(None);
        backend
            .more_tiles(
                someone(),
                Tab::Posts,
                after,
                resting,
                CancelToken::default(),
            )
            .blocking_recv()
            .unwrap()
    }

    #[test]
    fn a_first_page_hands_out_no_cursor_unless_a_test_says_there_are_more() {
        let backend = Scripted::with(vec![account(42, "me", true)]);
        let first = backend
            .tiles(someone(), Tab::Posts)
            .blocking_recv()
            .unwrap();
        assert_eq!(first.unwrap().next, None);
    }

    #[test]
    fn pages_past_the_first_come_from_each_cursor_and_a_failed_one_keeps_it() {
        let backend = Scripted::with(vec![account(42, "me", true)]);
        backend.script(|script| {
            script.more_pages(2).more_fails(1);
        });
        let first = backend
            .tiles(someone(), Tab::Posts)
            .blocking_recv()
            .unwrap();
        let after = first.unwrap().next.expect("a second page");

        assert!(more(&backend, after).is_err(), "the page fails once");
        let second = more(&backend, after).expect("asked again from the same place");
        assert_eq!(second.tiles[0].code, "C12invented");
        let third = more(&backend, second.next.expect("a third page")).unwrap();
        assert_eq!(third.tiles[11].code, "C35invented");
        assert_eq!(third.next, None, "the tab has ended");
        assert!(more(&backend, after).is_err(), "a cursor is spent once");

        let pages: Vec<u32> = backend
            .asked()
            .into_iter()
            .filter_map(|asked| match asked {
                Asked::MoreTiles(_, _, page) => Some(page),
                _ => None,
            })
            .collect();
        assert_eq!(pages, [2, 2, 3, 0]);
    }

    #[test]
    fn a_list_read_whole_is_kept_and_answered_with_nothing_asked() {
        let backend = Scripted::with(vec![account(42, "me", true)]);
        let me = account(42, "me", true).named;
        assert_eq!(
            backend
                .list_kept(me.clone(), Side::Followers)
                .blocking_recv()
                .unwrap(),
            None
        );
        let (walked, _) = watch::channel(Walked::default());
        let read = backend
            .list(me.clone(), Side::Followers, walked, CancelToken::default())
            .blocking_recv()
            .unwrap()
            .unwrap();
        let asked = backend.asked();

        let kept = backend
            .list_kept(me, Side::Followers)
            .blocking_recv()
            .unwrap()
            .expect("kept");
        assert_eq!(kept.people, read.people);
        assert_eq!(kept.taken_at, read.taken_at);
        assert_eq!(backend.asked(), asked, "nothing more was asked");
    }

    #[test]
    fn known_accounts_are_the_ones_whose_name_begins_with_what_was_typed() {
        let backend = Scripted::with(vec![account(42, "me", true)]);
        backend.script(|script| {
            script.known(vec![
                account(1, "anna", false),
                account(2, "Annabel", false),
                account(3, "bob", false),
            ]);
        });
        let named = |typed: &str, limit: usize| -> Vec<String> {
            backend
                .known_accounts(typed.to_string(), limit)
                .blocking_recv()
                .unwrap()
                .into_iter()
                .filter_map(|account| account.named.username)
                .collect()
        };
        assert_eq!(named("@ann", 6), ["anna", "Annabel"]);
        assert_eq!(named("ANN", 1), ["anna"]);
        assert!(named("", 6).is_empty());
    }

    #[test]
    fn signing_out_and_switching_keep_one_account_in_use() {
        let backend = Scripted::with(vec![
            account(42, "me", true),
            account(9001, "someone", false),
        ]);
        let switched = backend.switch_to(9001.into()).blocking_recv().unwrap();
        assert_eq!(switched[0], account(9001, "someone", true));
        assert!(!switched[1].active);

        let left = backend.sign_out(9001.into()).blocking_recv().unwrap();
        assert_eq!(left, [account(42, "me", true)]);
    }
}
