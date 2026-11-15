//! A backend that answers what a test told it to, at once or when the test
//! lets it: for the tests that drive the views with no engine under them.
//! What it was asked is kept, so a test reads back that a screen called what
//! it should and nothing else. **Only a `testing` build has it.**

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use snob_core::secret::Secret;
use snob_core::{Epoch, Pk};
use snob_ig::pace::CancelToken;
use tokio::sync::{oneshot, watch};

use super::{Account, Answer, Backend, Loaded, SignIn};
use crate::state::flow::{Found, Method, Named, Update};
use crate::state::prefs::Placement;

/// What the backend was asked, in order. **Never the session ID itself**:
/// only that one was handed over, and whether it was the one expected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    Load,
    SignInWith { expected: bool },
    SignInInAWindow { renewing: Option<Pk> },
    SignOut(Pk),
    SwitchTo(Pk),
    PausedUntil,
    CheckForUpdate,
    FetchUpdate,
    SetDetailed(bool),
    RememberMethod(Option<Method>),
    RememberWindow,
    DiagnosticReport,
    CrashShown,
}

type Held = Box<dyn FnOnce() + Send>;

struct State {
    accounts: Vec<Account>,
    expired: Option<Named>,
    crash: Option<String>,
    /// The session ID a paste is expected to carry.
    session_id: String,
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
    held: Vec<Held>,
    /// The token of the window login in flight, to see it stopped.
    window: Option<CancelToken>,
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
        active,
    }
}

impl Scripted {
    /// A backend with `accounts` signed in, the active one first.
    pub fn with(accounts: Vec<Account>) -> Self {
        Self(Arc::new(Mutex::new(State {
            accounts,
            expired: None,
            crash: None,
            session_id: "42%3AAbCdEfGhIjKl%3A20".to_string(),
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
            held: Vec::new(),
            window: None,
        })))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
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

    /// Lets every answer held go, and stops holding.
    pub fn release(&self) {
        let held = {
            let mut state = self.state();
            state.holding = false;
            std::mem::take(&mut state.held)
        };
        for answer in held {
            answer();
        }
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
            state.asked.push(asked);
            if state.holding {
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

    /// The method chosen before the restart this start comes after.
    pub fn resume(&mut self, method: Method) -> &mut Self {
        self.0.resume = Some(method);
        self
    }
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
        })
    }

    fn sign_in_with(&self, session_id: Secret) -> Answer<SignIn> {
        let expected = session_id.expose() == self.state().session_id;
        self.answer(Asked::SignInWith { expected }, |state| {
            let outcome = state.sign_in.clone();
            signed_in(state, &outcome);
            outcome
        })
    }

    fn sign_in_in_a_window(&self, renewing: Option<Pk>, stop: CancelToken) -> Answer<SignIn> {
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
        self.answer(Asked::SignOut(pk), move |state| {
            state.accounts.retain(|account| account.named.pk != pk);
            if let Some(first) = state.accounts.first_mut() {
                first.active = true;
            }
            state.accounts.clone()
        })
    }

    fn switch_to(&self, pk: Pk) -> Answer<Vec<Account>> {
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
            .sign_in_with(Secret::new("not the one".to_string()))
            .blocking_recv()
            .unwrap();
        assert!(matches!(failed, SignIn::Failed { .. }));

        backend.script(|script| {
            script.sign_in(SignIn::Done(account(42, "me", true)));
        });
        let done = backend
            .sign_in_with(Secret::new("42%3AAbCdEfGhIjKl%3A20".to_string()))
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
