//! Which screen is shown, and what moves the app from one to the next: the
//! start, the two ways to sign in, the update that may come between, the
//! error, the welcome. **A function of what happened, with no clock, no
//! window and no engine in it**, so every path of the flow is a test here
//! before it is a screen.

use std::time::Duration;

use snob_engine::login::Failure;

/// How a person chose to sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// In a window of snob's own browser.
    Browser,
    /// By pasting a `sessionid`.
    Paste,
}

/// An account as a screen names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub pk: snob_core::Pk,
    pub username: Option<String>,
}

/// What the app found on this computer when it started: the accounts the
/// command line signed in as are the app's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// Nobody is signed in.
    Nobody,
    /// The account in use, with a session stored.
    SignedIn(Named),
    /// The account in use, whose session is gone or no longer works.
    Expired(Named),
}

/// What is known about a newer version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Update {
    /// The check has not answered yet.
    Unknown,
    /// There is none, or the check could not be made (no network, a failure).
    None,
    /// There is one and the app replaces itself with it.
    Ready,
    /// There is one and a package manager installed the app: said in the
    /// band, never applied by the app.
    ByItsManager,
}

/// Where the update between choosing a method and using it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Updating {
    Checking,
    /// A whole number from 0 to 100, when the size is known.
    Downloading(Option<u8>),
    Restarting,
}

/// A screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Splash,
    /// "Let's get started." With an account when its session expired; with
    /// `back` when it was reached from a signed-in app to add an account.
    Start {
        expired: Option<Named>,
        back: bool,
    },
    Updating {
        then: Method,
        at: Updating,
    },
    Browser,
    Paste,
    Failed {
        why: Failure,
        /// The code the failure was logged under.
        id: String,
        /// What "Try again" goes back to.
        retry: Method,
    },
    Welcome(Named),
    Home,
    Settings,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The app finished loading: what it found, and the method chosen before
    /// a restart for an update, when there was one.
    Loaded {
        found: Found,
        resume: Option<Method>,
    },
    Chose(Method),
    /// The update check answered, or the app stopped waiting for it.
    Checked(Update),
    Downloaded(Option<u8>),
    /// The download is whole and checked: the app restarts.
    Staged,
    /// The update could not be fetched: the app goes on as it is.
    UpdateFailed,
    SignedIn(Named),
    /// The person closed the login window, or canceled.
    GaveUp,
    Failed {
        why: Failure,
        id: String,
    },
    Retry,
    Back,
    /// The welcome has been shown for as long as it stays.
    Welcomed,
    OpenSettings,
    AddAccount,
    /// The account in use was signed out; `next` is the account now in use,
    /// when another is signed in.
    SignedOut {
        next: Option<Named>,
    },
    /// The session of the account in use stopped working.
    Expired(Named),
}

/// Something to do beside showing a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Give the check this long, then go on without it.
    WaitForTheCheck(Duration),
    /// Fetch the update, remembering the method for after the restart.
    Download {
        then: Method,
    },
    Restart,
    /// Open snob's browser window on the login page.
    OpenBrowser,
    /// Close it, when it is open.
    CloseBrowser,
    /// Say once that the update failed and the app goes on as it is.
    SayUpdateFailed,
    /// Forget the method remembered for after a restart.
    ForgetResume,
}

/// How long the check is waited for once a method is chosen.
pub const CHECK_PATIENCE: Duration = Duration::from_secs(8);

/// The flow: the screen shown and what is known about an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flow {
    screen: Screen,
    update: Update,
    /// Whether an account is signed in and in use: what "back" from the
    /// start screen and from settings returns to.
    signed_in: bool,
}

impl Default for Flow {
    fn default() -> Self {
        Self {
            screen: Screen::Splash,
            update: Update::Unknown,
            signed_in: false,
        }
    }
}

impl Flow {
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// What is known about a newer version, for the band.
    pub fn update(&self) -> Update {
        self.update
    }

    fn start(&self, expired: Option<Named>) -> Screen {
        Screen::Start {
            expired,
            back: self.signed_in,
        }
    }

    fn method(method: Method) -> (Screen, Vec<Effect>) {
        match method {
            Method::Browser => (Screen::Browser, vec![Effect::OpenBrowser]),
            Method::Paste => (Screen::Paste, Vec::new()),
        }
    }

    /// The screen after a method was chosen, given what the check said.
    fn after_choosing(&self, then: Method) -> (Screen, Vec<Effect>) {
        match self.update {
            Update::Unknown => (
                Screen::Updating {
                    then,
                    at: Updating::Checking,
                },
                vec![Effect::WaitForTheCheck(CHECK_PATIENCE)],
            ),
            Update::Ready => (
                Screen::Updating {
                    then,
                    at: Updating::Downloading(None),
                },
                vec![Effect::Download { then }],
            ),
            Update::None | Update::ByItsManager => Self::method(then),
        }
    }

    /// Takes `event` and answers what to do beside showing the new screen.
    /// **An event that means nothing on the screen shown changes nothing**:
    /// a late answer to a screen already left is not an error.
    pub fn on(&mut self, event: Event) -> Vec<Effect> {
        let (screen, effects) = match (&self.screen, event) {
            (Screen::Splash, Event::Loaded { found, resume }) => match (found, resume) {
                (Found::SignedIn(_), _) => {
                    self.signed_in = true;
                    (Screen::Home, vec![Effect::ForgetResume])
                }
                // Back from the restart an update asked for: straight to the
                // method chosen before it, with no second check.
                (Found::Nobody | Found::Expired(_), Some(method)) => {
                    let (screen, mut effects) = Self::method(method);
                    effects.push(Effect::ForgetResume);
                    (screen, effects)
                }
                (Found::Nobody, None) => (self.start(None), Vec::new()),
                (Found::Expired(account), None) => (self.start(Some(account)), Vec::new()),
            },

            // The check answers whenever it does, on whichever screen.
            (
                Screen::Updating {
                    then,
                    at: Updating::Checking,
                },
                Event::Checked(update),
            ) => {
                let then = *then;
                match update {
                    // Waited for and not answered: on without it, and a later
                    // answer is still taken for the band.
                    Update::Unknown => Self::method(then),
                    known => {
                        self.update = known;
                        self.after_choosing(then)
                    }
                }
            }
            (_, Event::Checked(update)) => {
                if update != Update::Unknown {
                    self.update = update;
                }
                return Vec::new();
            }

            (Screen::Start { .. }, Event::Chose(method)) => self.after_choosing(method),
            (Screen::Start { back: true, .. }, Event::Back) => (Screen::Home, Vec::new()),

            (
                Screen::Updating {
                    then,
                    at: Updating::Downloading(_),
                },
                Event::Downloaded(percent),
            ) => (
                Screen::Updating {
                    then: *then,
                    at: Updating::Downloading(percent),
                },
                Vec::new(),
            ),
            (
                Screen::Updating {
                    then,
                    at: Updating::Downloading(_),
                },
                Event::Staged,
            ) => (
                Screen::Updating {
                    then: *then,
                    at: Updating::Restarting,
                },
                vec![Effect::Restart],
            ),
            (Screen::Updating { then, .. }, Event::UpdateFailed) => {
                let then = *then;
                self.update = Update::None;
                let (screen, mut effects) = Self::method(then);
                effects.push(Effect::SayUpdateFailed);
                effects.push(Effect::ForgetResume);
                (screen, effects)
            }

            (Screen::Browser | Screen::Paste, Event::SignedIn(account)) => {
                self.signed_in = true;
                (Screen::Welcome(account), Vec::new())
            }
            // Signed in from somewhere else while this screen waited: the
            // command line did it.
            (Screen::Start { back: false, .. }, Event::SignedIn(account)) => {
                self.signed_in = true;
                (Screen::Welcome(account), Vec::new())
            }
            // A window closed or a login canceled is a change of mind, not a
            // failure: back to where the method was chosen.
            (Screen::Browser, Event::GaveUp | Event::Back) => {
                (self.start(None), vec![Effect::CloseBrowser])
            }
            (Screen::Paste, Event::Back) => (self.start(None), Vec::new()),
            (Screen::Browser, Event::Failed { why, id }) => (
                Screen::Failed {
                    why,
                    id,
                    retry: Method::Browser,
                },
                vec![Effect::CloseBrowser],
            ),
            (Screen::Paste, Event::Failed { why, id }) => (
                Screen::Failed {
                    why,
                    id,
                    retry: Method::Paste,
                },
                Vec::new(),
            ),

            (Screen::Failed { retry, .. }, Event::Retry) => Self::method(*retry),
            (Screen::Failed { .. }, Event::Back) => (self.start(None), Vec::new()),

            (Screen::Welcome(_), Event::Welcomed) => (Screen::Home, Vec::new()),

            (Screen::Home, Event::OpenSettings) => (Screen::Settings, Vec::new()),
            (Screen::Settings, Event::Back) => (Screen::Home, Vec::new()),
            (Screen::Home | Screen::Settings, Event::AddAccount) => (self.start(None), Vec::new()),

            (Screen::Home | Screen::Settings, Event::SignedOut { next: Some(_) }) => {
                (Screen::Home, Vec::new())
            }
            (Screen::Home | Screen::Settings, Event::SignedOut { next: None }) => {
                self.signed_in = false;
                (self.start(None), Vec::new())
            }
            (Screen::Home | Screen::Settings, Event::Expired(account)) => {
                self.signed_in = false;
                (self.start(Some(account)), Vec::new())
            }

            _ => return Vec::new(),
        };
        self.screen = screen;
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Named {
        Named {
            pk: 42.into(),
            username: Some("me".into()),
        }
    }

    fn loaded(found: Found) -> Event {
        Event::Loaded {
            found,
            resume: None,
        }
    }

    fn start() -> Screen {
        Screen::Start {
            expired: None,
            back: false,
        }
    }

    /// A flow that has taken `events`, and what the last one asked for.
    fn after(events: impl IntoIterator<Item = Event>) -> (Flow, Vec<Effect>) {
        let mut flow = Flow::default();
        let mut effects = Vec::new();
        for event in events {
            effects = flow.on(event);
        }
        (flow, effects)
    }

    #[test]
    fn an_account_the_command_line_signed_in_goes_straight_home() {
        let (flow, _) = after([loaded(Found::SignedIn(me()))]);
        assert_eq!(flow.screen(), &Screen::Home);
    }

    #[test]
    fn nobody_signed_in_is_asked_how_to_start() {
        let (flow, effects) = after([loaded(Found::Nobody)]);
        assert_eq!(flow.screen(), &start());
        assert!(effects.is_empty());
    }

    #[test]
    fn an_expired_session_is_not_a_new_person() {
        let (flow, _) = after([loaded(Found::Expired(me()))]);
        assert_eq!(
            flow.screen(),
            &Screen::Start {
                expired: Some(me()),
                back: false
            }
        );
    }

    #[test]
    fn a_check_already_answered_is_never_waited_for() {
        let (flow, effects) = after([
            Event::Checked(Update::None),
            loaded(Found::Nobody),
            Event::Chose(Method::Browser),
        ]);
        assert_eq!(flow.screen(), &Screen::Browser);
        assert_eq!(effects, [Effect::OpenBrowser]);

        let (flow, effects) = after([
            loaded(Found::Nobody),
            Event::Checked(Update::None),
            Event::Chose(Method::Paste),
        ]);
        assert_eq!(flow.screen(), &Screen::Paste);
        assert!(effects.is_empty());
    }

    #[test]
    fn a_check_still_out_is_waited_for_and_then_given_up_on() {
        let (mut flow, effects) = after([loaded(Found::Nobody), Event::Chose(Method::Paste)]);
        assert_eq!(
            flow.screen(),
            &Screen::Updating {
                then: Method::Paste,
                at: Updating::Checking
            }
        );
        assert_eq!(effects, [Effect::WaitForTheCheck(CHECK_PATIENCE)]);

        // The wait ran out with no answer: on to the method, and the answer
        // that comes later still reaches the band.
        assert!(flow.on(Event::Checked(Update::Unknown)).is_empty());
        assert_eq!(flow.screen(), &Screen::Paste);
        assert_eq!(flow.update(), Update::Unknown);
        flow.on(Event::Checked(Update::ByItsManager));
        assert_eq!(flow.screen(), &Screen::Paste);
        assert_eq!(flow.update(), Update::ByItsManager);
    }

    #[test]
    fn an_update_is_fetched_then_the_app_restarts_into_the_method_chosen() {
        let (mut flow, effects) = after([
            loaded(Found::Nobody),
            Event::Chose(Method::Browser),
            Event::Checked(Update::Ready),
        ]);
        assert_eq!(
            flow.screen(),
            &Screen::Updating {
                then: Method::Browser,
                at: Updating::Downloading(None)
            }
        );
        assert_eq!(
            effects,
            [Effect::Download {
                then: Method::Browser
            }]
        );

        flow.on(Event::Downloaded(Some(63)));
        assert_eq!(
            flow.screen(),
            &Screen::Updating {
                then: Method::Browser,
                at: Updating::Downloading(Some(63))
            }
        );
        assert_eq!(flow.on(Event::Staged), [Effect::Restart]);
        assert_eq!(
            flow.screen(),
            &Screen::Updating {
                then: Method::Browser,
                at: Updating::Restarting
            }
        );

        // The app that starts after it: no start screen, no second check.
        let (flow, effects) = after([Event::Loaded {
            found: Found::Nobody,
            resume: Some(Method::Browser),
        }]);
        assert_eq!(flow.screen(), &Screen::Browser);
        assert_eq!(effects, [Effect::OpenBrowser, Effect::ForgetResume]);
    }

    #[test]
    fn an_update_a_package_manager_owns_is_never_fetched() {
        let (flow, effects) = after([
            loaded(Found::Nobody),
            Event::Checked(Update::ByItsManager),
            Event::Chose(Method::Paste),
        ]);
        assert_eq!(flow.screen(), &Screen::Paste);
        assert!(effects.is_empty());
        assert_eq!(flow.update(), Update::ByItsManager);
    }

    #[test]
    fn an_update_that_fails_is_said_once_and_the_app_goes_on() {
        let (mut flow, _) = after([
            loaded(Found::Nobody),
            Event::Checked(Update::Ready),
            Event::Chose(Method::Paste),
        ]);
        let effects = flow.on(Event::UpdateFailed);
        assert_eq!(flow.screen(), &Screen::Paste);
        assert_eq!(effects, [Effect::SayUpdateFailed, Effect::ForgetResume]);
        // Not offered again in this run.
        assert_eq!(flow.update(), Update::None);
    }

    #[test]
    fn a_sign_in_from_elsewhere_is_welcomed_unless_an_account_is_being_added() {
        let (flow, effects) = after([loaded(Found::Nobody), Event::SignedIn(me())]);
        assert_eq!(flow.screen(), &Screen::Welcome(me()));
        assert!(effects.is_empty());

        let (flow, _) = after([
            loaded(Found::SignedIn(me())),
            Event::AddAccount,
            Event::SignedIn(me()),
        ]);
        assert!(
            matches!(flow.screen(), Screen::Start { back: true, .. }),
            "{:?}",
            flow.screen()
        );
    }

    #[test]
    fn closing_the_login_window_is_no_error() {
        let (mut flow, _) = after([
            Event::Checked(Update::None),
            loaded(Found::Nobody),
            Event::Chose(Method::Browser),
        ]);
        assert_eq!(flow.on(Event::GaveUp), [Effect::CloseBrowser]);
        assert_eq!(flow.screen(), &start());
    }

    #[test]
    fn a_failure_offers_the_same_method_again_or_the_way_back() {
        for (method, opens) in [
            (Method::Browser, vec![Effect::OpenBrowser]),
            (Method::Paste, Vec::new()),
        ] {
            let failed = || {
                after([
                    Event::Checked(Update::None),
                    loaded(Found::Nobody),
                    Event::Chose(method),
                    Event::Failed {
                        why: Failure::Rejected,
                        id: "7K2Q-9FMX".into(),
                    },
                ])
                .0
            };
            let mut flow = failed();
            assert_eq!(
                flow.screen(),
                &Screen::Failed {
                    why: Failure::Rejected,
                    id: "7K2Q-9FMX".into(),
                    retry: method
                }
            );
            assert_eq!(flow.on(Event::Retry), opens);
            assert_eq!(
                flow.screen(),
                &match method {
                    Method::Browser => Screen::Browser,
                    Method::Paste => Screen::Paste,
                }
            );

            let mut flow = failed();
            flow.on(Event::Back);
            assert_eq!(flow.screen(), &start());
        }
    }

    #[test]
    fn signing_in_is_welcomed_and_then_home() {
        let (mut flow, _) = after([
            Event::Checked(Update::None),
            loaded(Found::Nobody),
            Event::Chose(Method::Paste),
            Event::SignedIn(me()),
        ]);
        assert_eq!(flow.screen(), &Screen::Welcome(me()));
        flow.on(Event::Welcomed);
        assert_eq!(flow.screen(), &Screen::Home);
    }

    #[test]
    fn adding_an_account_can_be_backed_out_of() {
        let (mut flow, _) = after([loaded(Found::SignedIn(me())), Event::AddAccount]);
        assert_eq!(
            flow.screen(),
            &Screen::Start {
                expired: None,
                back: true
            }
        );
        flow.on(Event::Back);
        assert_eq!(flow.screen(), &Screen::Home);
    }

    #[test]
    fn signing_out_goes_to_the_next_account_or_to_the_start() {
        let other = Named {
            pk: 9001.into(),
            username: Some("someone".into()),
        };
        let (flow, _) = after([
            loaded(Found::SignedIn(me())),
            Event::SignedOut { next: Some(other) },
        ]);
        assert_eq!(flow.screen(), &Screen::Home);

        let (flow, _) = after([
            loaded(Found::SignedIn(me())),
            Event::OpenSettings,
            Event::SignedOut { next: None },
        ]);
        assert_eq!(flow.screen(), &start());
    }

    #[test]
    fn a_session_that_stops_working_asks_that_account_to_sign_in_again() {
        let (flow, _) = after([loaded(Found::SignedIn(me())), Event::Expired(me())]);
        assert_eq!(
            flow.screen(),
            &Screen::Start {
                expired: Some(me()),
                back: false
            }
        );
    }

    #[test]
    fn an_answer_to_a_screen_already_left_changes_nothing() {
        let (mut flow, _) = after([loaded(Found::SignedIn(me()))]);
        for late in [
            Event::SignedIn(me()),
            Event::GaveUp,
            Event::Retry,
            Event::Welcomed,
            Event::Staged,
            Event::Failed {
                why: Failure::Network,
                id: String::new(),
            },
        ] {
            assert!(flow.on(late).is_empty());
            assert_eq!(flow.screen(), &Screen::Home);
        }
    }
}
