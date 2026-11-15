//! The app as one view: the flow's screen inside the frame, what the band
//! says, the accounts, and everything that happens to them. **A screen never
//! calls the engine or the system**: it says what the person did
//! ([`Shell::happened`] and the few methods beside it), and what follows is
//! decided by the flow (`state::flow`) and done here, off the window's
//! thread.

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, FocusHandle, Pixels, ScrollHandle, SharedString, Subscription,
    Window, point, px,
};
use snob_core::secret::Secret;
use snob_core::{Epoch, Pk};
use snob_engine::login::Failure;
use snob_ig::pace::CancelToken;
use tokio::sync::watch;

use super::look::Look;
use super::{frame, screens};
use crate::backend::{Account, Backend, Loaded, SignIn};
use crate::state::flow::{Effect, Event, Flow, Found, Method, Screen, Update};
use crate::state::paste::{self, Field};
use crate::state::status::{self, Band, Status};
use crate::system::{self, System};
use crate::theme::motion;

/// How long the window rests before where it is gets remembered: a drag
/// says it moved many times a second.
const SETTLED_AFTER: Duration = Duration::from_millis(500);

/// Where the screen shown is in its coming and going.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phase {
    /// From 0, not there, to 1, there.
    pub presence: f32,
    /// How far under its place it still is, in logical pixels.
    pub rise: f32,
    /// Its scale beside 1.
    pub scale: f32,
}

impl Phase {
    pub const THERE: Self = Self {
        presence: 1.0,
        rise: 0.0,
        scale: 1.0,
    };
}

/// The field a session ID is pasted into, for as long as its screen is
/// shown. **Gone with the screen, and what was in it with it.**
pub(crate) struct PasteForm {
    pub input: Entity<InputState>,
    /// Whether the person asked to see what is in the field.
    pub revealed: bool,
    /// What the field was judged to hold: nothing yet while the person is
    /// still typing.
    pub judged: Option<Field>,
    /// Whether Instagram is being asked about it.
    pub checking: bool,
    /// How many times the field changed.
    edits: usize,
    /// Whether the change under way is a paste, which is judged at once.
    pasting: bool,
    _watching: Subscription,
}

/// A screen as the log names it: which one, and nothing of what is on it.
fn name(screen: &Screen) -> &'static str {
    match screen {
        Screen::Splash => "splash",
        Screen::Start { .. } => "start",
        Screen::Updating { .. } => "updating",
        Screen::Browser => "browser",
        Screen::Paste => "paste",
        Screen::Failed { .. } => "failed",
        Screen::Welcome(_) => "welcome",
        Screen::Home => "home",
        Screen::Settings => "settings",
    }
}

/// What happened, as the log names it.
fn event_name(event: &Event) -> &'static str {
    match event {
        Event::Loaded { .. } => "loaded",
        Event::Chose(Method::Browser) => "chose_browser",
        Event::Chose(Method::Paste) => "chose_paste",
        Event::Checked(_) => "update_checked",
        Event::Downloaded(_) => "update_progress",
        Event::Staged => "update_staged",
        Event::UpdateFailed => "update_failed",
        Event::SignedIn(_) => "signed_in",
        Event::GaveUp => "gave_up",
        Event::Failed { .. } => "sign_in_failed",
        Event::Retry => "retry",
        Event::Back => "back",
        Event::Welcomed => "welcomed",
        Event::OpenSettings => "open_settings",
        Event::AddAccount => "add_account",
        Event::SignedOut { .. } => "signed_out",
        Event::Expired(_) => "session_expired",
    }
}

/// The app.
pub struct Shell {
    backend: Arc<dyn Backend>,
    system: Rc<dyn System>,
    flow: Flow,
    band: Band,
    accounts: Vec<Account>,
    /// The screen drawn: the flow's, once the one before it has left.
    shown: Screen,
    entered: Instant,
    leaving: Option<Instant>,
    /// Where focus goes when a screen comes in: its title.
    pub(crate) title: FocusHandle,
    pub(crate) scroll: ScrollHandle,
    /// What stops the login window that is open.
    browser: Option<CancelToken>,
    /// Which sign-in is the one whose answer counts: an answer to one the
    /// person left is not acted on.
    attempt: usize,
    /// How many times the window was moved or resized.
    moved: usize,
    /// The account whose expired session the sign-in under way renews.
    renewing: Option<Pk>,
    /// Until when Instagram asked to wait, when the failure shown said so.
    pub(crate) failed_until: Option<Epoch>,
    pub(crate) details_open: bool,
    /// What a screen reader was last told.
    pub(crate) announced: SharedString,
    /// Something said once, under the screen's title: an update that failed.
    pub(crate) notice: Option<SharedString>,
    /// The report of a crash nobody has been shown yet.
    pub(crate) crash: Option<String>,
    pub(crate) paste: Option<PasteForm>,
}

impl Shell {
    pub fn new(
        backend: Arc<dyn Backend>,
        system: Rc<dyn System>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let started = Instant::now();
        let title = cx.focus_handle();
        window.focus(&title, cx);

        let mut band = Band::default();
        if system.online() == Some(false) {
            band.set(Status::Offline);
        }

        let loading = backend.load();
        cx.spawn_in(window, async move |this, cx| {
            let loaded = loading.await;
            let left = motion::SPLASH_AT_LEAST.saturating_sub(started.elapsed());
            cx.background_executor().timer(left).await;
            this.update_in(cx, |this, window, cx| match loaded {
                Ok(loaded) => this.loaded(loaded, window, cx),
                Err(_) => tracing::error!("the app could not read what is on this computer"),
            })
            .ok();
        })
        .detach();

        // The check starts under the splash, so a method chosen later finds
        // it answered. **No network is said, no check is made.**
        if system.online() == Some(false) {
            tracing::info!(why = "offline", "the update check was skipped");
        }
        let checking = (system.online() != Some(false)).then(|| backend.check_for_update());
        cx.spawn_in(window, async move |this, cx| {
            let update = match checking {
                Some(checking) => checking.await.unwrap_or(Update::None),
                None => Update::None,
            };
            this.update_in(cx, |this, window, cx| {
                tracing::info!(?update, "the update check answered");
                this.happened(Event::Checked(update), window, cx);
            })
            .ok();
        })
        .detach();

        let mut heard = system.events();
        cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = heard.recv().await {
                let told = this.update_in(cx, |this, window, cx| {
                    this.system_said(event, window, cx);
                });
                if told.is_err() {
                    break;
                }
            }
        })
        .detach();

        // Where the window is, remembered once it has stopped moving.
        cx.observe_window_bounds(window, |this, window, cx| {
            this.moved += 1;
            let moved = this.moved;
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(SETTLED_AFTER).await;
                this.update_in(cx, |this, window, _| {
                    if this.moved == moved {
                        this.backend
                            .remember_window(super::window::placement(window));
                    }
                })
                .ok();
            })
            .detach();
        })
        .detach();

        tracing::info!(screen = "splash", "screen");
        Self {
            backend,
            system,
            flow: Flow::default(),
            band,
            accounts: Vec::new(),
            shown: Screen::Splash,
            entered: started,
            leaving: None,
            title,
            scroll: ScrollHandle::new(),
            browser: None,
            attempt: 0,
            moved: 0,
            renewing: None,
            failed_until: None,
            details_open: false,
            announced: SharedString::default(),
            notice: None,
            crash: None,
            paste: None,
        }
    }

    /// The screen drawn.
    pub fn shown(&self) -> &Screen {
        &self.shown
    }

    /// Every account signed in here, the one in use first.
    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    /// The account in use, once somebody is signed in.
    pub fn active(&self) -> Option<&Account> {
        self.accounts.iter().find(|account| account.active)
    }

    /// What the band says.
    pub fn status(&self) -> Option<&Status> {
        self.band.shown()
    }

    fn loaded(&mut self, loaded: Loaded, window: &mut Window, cx: &mut Context<Self>) {
        self.accounts = loaded.accounts;
        self.crash = loaded.crash;
        if matches!(loaded.found, Found::SignedIn(_)) {
            self.read_the_pause(window, cx);
        }
        self.happened(
            Event::Loaded {
                found: loaded.found,
                resume: loaded.resume,
            },
            window,
            cx,
        );
    }

    /// Takes what happened: the flow decides what is shown, and what it asks
    /// for beside that is done.
    pub fn happened(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!(event = event_name(&event), "event");
        let before = self.flow.update();
        for effect in self.flow.on(event) {
            self.run(effect, window, cx);
        }
        if self.flow.update() != before && self.flow.update() == Update::ByItsManager {
            self.say(Status::UpdateAvailable, cx);
        }
        self.follow(window, cx);
    }

    /// A method chosen on the start screen.
    pub fn chose(&mut self, method: Method, window: &mut Window, cx: &mut Context<Self>) {
        self.renewing = match self.flow.screen() {
            Screen::Start {
                expired: Some(account),
                ..
            } => Some(account.pk),
            _ => None,
        };
        self.notice = None;
        self.happened(Event::Chose(method), window, cx);
    }

    fn run(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        match effect {
            Effect::WaitForTheCheck(patience) => {
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(patience).await;
                    this.update_in(cx, |this, window, cx| {
                        this.happened(Event::Checked(Update::Unknown), window, cx);
                    })
                    .ok();
                })
                .detach();
            }
            Effect::Download { then } => self.download(then, window, cx),
            Effect::Restart => {
                tracing::info!("restarting into the update");
                self.system.restart(cx);
            }
            Effect::OpenBrowser => self.open_browser(window, cx),
            Effect::CloseBrowser => {
                if let Some(stop) = self.browser.take() {
                    stop.cancel();
                }
            }
            Effect::SayUpdateFailed => {
                let said = Look::now(cx).tr("update-failed");
                self.announce(said.clone(), cx);
                self.notice = Some(said.into());
            }
            Effect::ForgetResume => self.backend.remember_method(None),
        }
    }

    fn download(&mut self, then: Method, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.remember_method(Some(then));
        let (progress, mut watched) = watch::channel(None);
        let fetching = self.backend.fetch_update(progress);
        self.say(Status::Downloading { percent: None }, cx);
        cx.spawn_in(window, async move |this, cx| {
            while watched.changed().await.is_ok() {
                let percent = *watched.borrow_and_update();
                let told = this.update_in(cx, |this, window, cx| {
                    this.say(Status::Downloading { percent }, cx);
                    this.happened(Event::Downloaded(percent), window, cx);
                });
                if told.is_err() {
                    return;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let ready = fetching.await.unwrap_or(false);
            this.update_in(cx, |this, window, cx| {
                tracing::info!(ready, "the update was fetched");
                this.quiet(|band| band.clear_download(), cx);
                let event = if ready {
                    Event::Staged
                } else {
                    Event::UpdateFailed
                };
                this.happened(event, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn open_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stop = CancelToken::default();
        self.browser = Some(stop.clone());
        self.attempt += 1;
        let attempt = self.attempt;
        let signing_in = self.backend.sign_in_in_a_window(self.renewing, stop);
        cx.spawn_in(window, async move |this, cx| {
            let outcome = signing_in.await;
            this.update_in(cx, |this, window, cx| {
                if this.attempt == attempt {
                    this.browser = None;
                    this.signed(outcome.ok(), window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The field of the paste screen, new and empty.
    fn paste_form(&mut self, window: &mut Window, cx: &mut Context<Self>) -> PasteForm {
        let input = cx.new(|cx| InputState::new(window, cx).masked(true));
        let watching = cx.subscribe_in(
            &input,
            window,
            |shell, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => shell.the_field_changed(window, cx),
                InputEvent::PressEnter { .. } => shell.submit_the_paste(window, cx),
                _ => {}
            },
        );
        PasteForm {
            input,
            revealed: false,
            judged: None,
            checking: false,
            edits: 0,
            pasting: false,
            _watching: watching,
        }
    }

    /// What is in the field is about to change by a paste.
    pub(crate) fn pasted_into_the_field(&mut self) {
        if let Some(form) = &mut self.paste {
            form.pasting = true;
        }
    }

    /// The field changed: judged at once after a paste, and otherwise once
    /// the person has stopped typing.
    fn the_field_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.paste else {
            return;
        };
        form.edits += 1;
        form.judged = None;
        let edits = form.edits;
        if std::mem::take(&mut form.pasting) {
            self.judge_the_field(cx);
            return;
        }
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(paste::JUDGED_AFTER).await;
            this.update(cx, |shell, cx| {
                if shell.paste.as_ref().is_some_and(|form| form.edits == edits) {
                    shell.judge_the_field(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn judge_the_field(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.paste else {
            return;
        };
        let judged = paste::judge(&form.input.read(cx).value());
        form.judged = Some(judged);
        if judged == Field::Invalid {
            let said = Look::now(cx).tr("paste-hint-invalid");
            self.announce(said, cx);
        }
        cx.notify();
    }

    /// Shows or hides what is in the field.
    pub(crate) fn reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.paste else {
            return;
        };
        form.revealed = !form.revealed;
        let masked = !form.revealed;
        form.input
            .update(cx, |input, cx| input.set_masked(masked, window, cx));
        cx.notify();
    }

    /// Puts what is on the clipboard in the field, in place of what was.
    pub(crate) fn paste_from_the_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.system.pasted(cx) else {
            return;
        };
        let Some(form) = &mut self.paste else {
            return;
        };
        form.edits += 1;
        form.input
            .update(cx, |input, cx| input.set_value(text.trim(), window, cx));
        self.judge_the_field(cx);
    }

    /// Signs in with what is in the field, when it has the shape of a
    /// session ID. **The secret goes to the engine and nowhere else**, and
    /// the field is emptied as it goes.
    pub(crate) fn submit_the_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.paste else {
            return;
        };
        let value = form.input.read(cx).value();
        if form.checking || paste::judge(&value) != Field::Valid {
            return;
        }
        form.checking = true;
        form.judged = Some(Field::Valid);
        let session_id = Secret::new(value.trim().to_string());
        form.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.announce(Look::now(cx).tr("paste-checking"), cx);

        self.attempt += 1;
        let attempt = self.attempt;
        let signing_in = self.backend.sign_in_with(session_id);
        cx.spawn_in(window, async move |this, cx| {
            let outcome = signing_in.await;
            this.update_in(cx, |this, window, cx| {
                if this.attempt == attempt {
                    this.signed(outcome.ok(), window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Leaves the sign-in under way: its answer no longer counts.
    pub(crate) fn leave_the_sign_in(&mut self) {
        self.attempt += 1;
    }

    fn signed(&mut self, outcome: Option<SignIn>, window: &mut Window, cx: &mut Context<Self>) {
        match outcome {
            Some(SignIn::Done(account)) => {
                self.renewing = None;
                let named = account.named.clone();
                for other in &mut self.accounts {
                    other.active = false;
                }
                self.accounts.retain(|other| other.named.pk != named.pk);
                self.accounts.insert(0, account);
                self.read_the_pause(window, cx);
                self.happened(Event::SignedIn(named), window, cx);
            }
            Some(SignIn::GaveUp) => self.happened(Event::GaveUp, window, cx),
            Some(SignIn::Failed { why, id, until }) => {
                self.failed_until = until;
                self.details_open = false;
                self.happened(Event::Failed { why, id }, window, cx);
            }
            // The engine went away without answering.
            None => self.happened(
                Event::Failed {
                    why: Failure::Other,
                    id: String::new(),
                },
                window,
                cx,
            ),
        }
    }

    /// Shows the flow's screen: at once when it is the one shown with other
    /// numbers, and otherwise once the one shown has left.
    fn follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.flow.screen();
        if *next == self.shown {
            return;
        }
        if name(next) == name(&self.shown) && self.leaving.is_none() {
            self.shown = next.clone();
            cx.notify();
            return;
        }
        if self.leaving.is_some() {
            // One is on its way out already: what comes in is the newest.
            return;
        }
        let exit = Look::now(cx).moved(motion::EXIT, 0.0, 1.0).lasts;
        self.leaving = Some(Instant::now());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(exit).await;
            this.update_in(cx, |this, window, cx| this.enter(window, cx))
                .ok();
        })
        .detach();
    }

    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leaving = None;
        self.shown = self.flow.screen().clone();
        self.entered = Instant::now();
        self.scroll.set_offset(point(px(0.), px(0.)));
        tracing::info!(screen = name(&self.shown), "screen");
        window.focus(&self.title, cx);
        self.paste = match self.shown {
            Screen::Paste => Some(self.paste_form(window, cx)),
            _ => None,
        };

        let look = Look::now(cx).clone();
        match &self.shown {
            Screen::Welcome(account) => {
                // Said before it moves on by itself.
                let who = screens::handle(account);
                self.announce(look.tr_with("welcome", &[("account", &who)]), cx);
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(motion::WELCOME_STAYS).await;
                    this.update_in(cx, |this, window, cx| {
                        this.happened(Event::Welcomed, window, cx);
                    })
                    .ok();
                })
                .detach();
            }
            Screen::Failed { why, retry, .. } => {
                let said = screens::failure(*why, *retry, self.failed_until, &look);
                self.announce(said, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Where the screen shown is in its coming and going, now. Asks for the
    /// next frame for as long as it moves.
    pub(crate) fn phase(&self, window: &mut Window, cx: &Context<Self>) -> Phase {
        let look = Look::now(cx);
        let splash = self.shown == Screen::Splash;
        let (through, moved, leaving) = match self.leaving {
            Some(since) => {
                let moved = look.moved(motion::EXIT, 0.0, if splash { 0.94 } else { 1.0 });
                (since.elapsed(), moved, true)
            }
            None => {
                let moved = look.moved(motion::ENTER, if splash { 0.0 } else { 14.0 }, 1.0);
                (self.entered.elapsed(), moved, false)
            }
        };
        let at = (through.as_secs_f32() / moved.lasts.as_secs_f32()).min(1.0);
        if at < 1.0 {
            window.request_animation_frame();
        }
        let eased = motion::ease_out(at);
        if leaving {
            Phase {
                presence: 1.0 - eased,
                rise: 0.0,
                scale: 1.0 + (moved.scales - 1.0) * eased,
            }
        } else if splash {
            // The splash is what the window opens on: it does not come in.
            Phase::THERE
        } else {
            Phase {
                presence: eased,
                rise: moved.travels * (1.0 - eased),
                scale: 1.0,
            }
        }
    }

    /// Tells a screen reader `said`.
    pub(crate) fn announce(&mut self, said: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.announced = said.into();
        cx.notify();
    }

    /// Turns `status` on in the band, telling a screen reader when what the
    /// band says changed enough to be worth saying.
    fn say(&mut self, status: Status, cx: &mut Context<Self>) {
        self.quiet(|band| band.set(status), cx);
    }

    /// Changes what is on in the band.
    fn quiet(&mut self, change: impl FnOnce(&mut Band), cx: &mut Context<Self>) {
        let before = self.band.shown().cloned();
        change(&mut self.band);
        let now = self.band.shown().cloned();
        if status::announced(before.as_ref(), now.as_ref())
            && let Some(now) = &now
        {
            let said = frame::status_line(now, Look::now(cx)).1;
            self.announce(said, cx);
        }
        cx.notify();
    }

    /// Reads until when Instagram asked the account in use to wait: from its
    /// budget, with nothing sent. The band says so, and stops saying so when
    /// the time comes.
    fn read_the_pause(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reading = self.backend.paused_until();
        cx.spawn_in(window, async move |this, cx| {
            let until = reading.await.ok().flatten();
            let now = snob_core::clock::now();
            let wait = this
                .update_in(cx, |this, _, cx| match until {
                    Some(until) if until.get() > now.get() => {
                        this.say(Status::Paused { until }, cx);
                        Some(Duration::from_secs(
                            (until.get() - now.get()).unsigned_abs(),
                        ))
                    }
                    _ => {
                        this.quiet(Band::clear_pause, cx);
                        None
                    }
                })
                .ok()
                .flatten();
            if let Some(wait) = wait {
                cx.background_executor().timer(wait).await;
                this.update_in(cx, |this, _, cx| {
                    this.quiet(
                        |band| {
                            band.lift_pause_at(snob_core::clock::now());
                        },
                        cx,
                    );
                })
                .ok();
            }
        })
        .detach();
    }

    fn system_said(&mut self, event: system::Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            system::Event::Settings(settings) => {
                tracing::info!(
                    dark = settings.dark,
                    more_contrast = settings.more_contrast,
                    reduce_motion = settings.reduce_motion,
                    reduce_transparency = settings.reduce_transparency,
                    text_scale = settings.text_scale,
                    "the system's settings changed"
                );
                let testing = Look::now(cx).testing;
                super::look::wear(Look::of(settings, testing), cx);
            }
            system::Event::Sleeping => tracing::info!("the system is going to sleep"),
            system::Event::Woke { slept } => {
                tracing::info!(slept_secs = slept.as_secs(), "the system woke");
                // What a clock was counted against is read again, not
                // counted on.
                self.quiet(
                    |band| {
                        band.lift_pause_at(snob_core::clock::now());
                    },
                    cx,
                );
                if self.active().is_some() {
                    self.read_the_pause(window, cx);
                }
            }
            system::Event::ShuttingDown => tracing::info!("the session is ending"),
            system::Event::Online(online) => {
                tracing::info!(online, "the network changed");
                if online {
                    self.quiet(Band::clear_offline, cx);
                } else {
                    self.say(Status::Offline, cx);
                }
            }
            system::Event::Activate => {
                tracing::info!("brought to the front by a second start");
                window.activate_window();
                cx.activate(true);
            }
            system::Event::Opened(notice) => {
                tracing::info!(?notice, "a notification was opened");
                window.activate_window();
                cx.activate(true);
            }
        }
        cx.notify();
    }

    /// How far the content is scrolled under the band.
    pub(crate) fn scrolled(&self) -> Pixels {
        -self.scroll.offset().y
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let phase = self.phase(window, cx);
        let screen: AnyElement = screens::render(self, phase, window, cx);
        frame::render(self, screen, window, cx)
    }
}
