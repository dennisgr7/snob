//! The app as one view: the flow's screen inside the frame, what the band
//! says, the accounts, and everything that happens to them. **A screen never
//! calls the engine or the system**: it says what the person did
//! ([`Shell::happened`] and the few methods beside it), and what follows is
//! decided by the flow (`state::flow`) and done here, off the window's
//! thread.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, ImgResourceLoader, Pixels, Resource,
    ScrollHandle, SharedString, Subscription, Window, point, px,
};
use snob_core::secret::Secret;
use snob_core::{Epoch, Pk};
use snob_engine::login::Failure;
use snob_engine::storage::Kind;
use snob_ig::pace::CancelToken;
use tokio::sync::watch;

use super::look::Look;
use super::{frame, screens};
use crate::backend::{Account, Backend, Cleared, Forgotten, Loaded, SignIn, Storage};
use crate::state::commands::{self, Command};
use crate::state::flow::{Effect, Event, Flow, Found, Method, Named, Place, Screen, Update};
use crate::state::frame::{self as shape, Icon, Morph, Pill, SECTIONS};
use crate::state::paste::{self, Field};
use crate::state::prefs::{Appearance, ThemeChoice};
use crate::state::rest::{self, Rest};
use crate::state::status::{self, Band, Status};
use crate::system::{self, System};
use crate::theme::motion;

mod pages;

pub use pages::PeopleList;
pub(crate) use pages::{Pages, PopupKind, TabRead, eased};

/// How long the window rests before where it is gets remembered: a drag
/// says it moved many times a second.
const SETTLED_AFTER: Duration = Duration::from_millis(500);

/// How often an app left open asks the backend about the session, which
/// asks Instagram once a day at most (`prefs::CHECKED_FOR_SECS`).
const SESSION_ASKED_EVERY: Duration = Duration::from_secs(60 * 60);

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

/// The frame as it is drawn on this frame: how open, how far each of the
/// rail's icons has come in, and where the selection is.
pub(crate) struct FrameNow {
    pub openness: f32,
    pub icons: [Icon; SECTIONS],
    pub pill: Option<Pill>,
    /// The section the app is in, whose button is the one chosen.
    pub selected: Option<usize>,
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
    /// The browsers the session can be said to come from: not known until
    /// the backend has looked.
    pub browsers: Option<Vec<String>>,
    /// Which of them it is said to come from.
    pub browser: usize,
    /// Whether the list of them is open.
    pub choosing: bool,
    /// The field that opens the list, where focus goes back when it closes.
    pub browser_field: FocusHandle,
    _watching: Subscription,
}

/// The settings while they are shown: what is typed in their search, where
/// their list is scrolled, and whether the languages are listed. Goes with
/// the screen.
pub(crate) struct SettingsPage {
    pub search: Entity<InputState>,
    pub scroll: super::scroll::OutlinedScroll,
    pub choosing_language: bool,
    /// The language field, where focus goes back when its list closes, and
    /// where it was last drawn, which the list is put under.
    pub language_field: FocusHandle,
    pub language_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Each language listed, the one chosen focused when the list opens.
    pub language_options: Vec<FocusHandle>,
    /// Whether the outline of the sections is coming or going, and since
    /// when, by the executor's clock: none before the thumb was first held.
    pub outline: Option<(bool, Instant)>,
    /// What is kept, once measured.
    pub storage: Option<Storage>,
    /// The question deleting history asks first, while it is asked.
    pub forgetting: Option<Forgetting>,
    _watching: Subscription,
}

/// What clearing or deleting what is kept did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Done {
    Cleared(Cleared),
    Forgot(Forgotten),
}

/// The question deleting history asks first.
pub(crate) struct Forgetting {
    /// Whose: one account, or with `None` every one here.
    pub only: Option<Pk>,
    /// The account's name as the settings say it.
    pub name: String,
    pub focus: FocusHandle,
    /// The answer focus starts on: the one that changes nothing.
    pub cancel: FocusHandle,
    /// What had focus when it was asked, where it goes back.
    pub from: Option<FocusHandle>,
}

/// The command palette, while it is open.
pub(crate) struct Palette {
    pub input: Entity<InputState>,
    /// Which of the commands listed Enter does.
    pub selected: usize,
    /// What keeps focus inside it.
    pub focus: FocusHandle,
    /// Where focus was, and returns when it closes.
    back_to: Option<FocusHandle>,
    _watching: Subscription,
}

/// The account's menu, while it is open.
pub(crate) struct AccountMenu {
    /// What keeps focus inside it.
    pub focus: FocusHandle,
    /// Since when the other accounts are listed, once they are.
    pub switching: Option<Instant>,
}

/// The question signing out asks first, while it is asked.
pub(crate) struct SigningOut {
    pub account: Named,
    pub focus: FocusHandle,
    /// The answer focus starts on: the one that changes nothing.
    pub cancel: FocusHandle,
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
        Screen::Search => "search",
        Screen::Automations => "automations",
        Screen::Settings => "settings",
        Screen::Profile(_) => "profile",
        Screen::People { .. } => "people",
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
        Event::Go(_) => "go",
        Event::OpenSettings => "open_settings",
        Event::AddAccount => "add_account",
        Event::SignedOut { .. } => "signed_out",
        Event::Expired(_) => "session_expired",
        Event::OpenProfile(_) => "open_profile",
        Event::OpenList { .. } => "open_list",
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
    pub announced: SharedString,
    /// Something said once, under the screen's title: an update that failed.
    pub(crate) notice: Option<SharedString>,
    /// The report of a crash nobody has been shown yet.
    pub(crate) crash: Option<String>,
    pub(crate) paste: Option<PasteForm>,
    pub(crate) settings: Option<SettingsPage>,
    /// What is being cleared or deleted, for one account or with `None` for
    /// all of them. **The shell's, not the settings'**: it runs on in the
    /// backend whatever screen is shown, and the settings opened again show
    /// it running.
    pub(crate) storage_work: Option<Option<Pk>>,
    /// What the last of it did, and for whom.
    pub(crate) storage_done: Option<(Option<Pk>, Done)>,
    pub(crate) menu: Option<AccountMenu>,
    pub(crate) palette: Option<Palette>,
    /// What opens the menu, and where focus returns when it closes.
    pub(crate) account_button: FocusHandle,
    pub(crate) signing_out: Option<SigningOut>,
    /// Whether the report of the last crash is unfolded.
    pub(crate) crash_open: bool,
    /// Whether the diagnostic report was copied on this visit to the
    /// settings: said until the screen is left.
    pub(crate) report_copied: bool,
    /// Whether the app rests (`state::rest`), and when anything last moved:
    /// a frame drawn, the window looked at or away from. By the executor's
    /// clock, which a test moves.
    rest: Rest,
    stirred: Instant,
    /// Whether a look at the rest is already on its way.
    looking: bool,
    /// Every account picture asked for, and how far each has come in on
    /// this frame.
    pictures: HashMap<PathBuf, Picture>,
    pictures_in: crate::ui::parts::Shown,
    /// The frame's shape and its way between them (`state::frame`), since
    /// when, by the executor's clock.
    frame: Morph,
    frame_since: Instant,
    /// The rail's selection: the section it left, the one it is going to,
    /// and since when.
    pill: (Option<usize>, Option<usize>, Instant),
    /// The profiles and lists the way back holds, and what is open over
    /// them (`pages`).
    pub(crate) pages: Pages,
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

        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(SESSION_ASKED_EVERY).await;
                let asked = this.update_in(cx, |this, window, cx| {
                    this.check_the_session(window, cx);
                });
                if asked.is_err() {
                    break;
                }
            }
        })
        .detach();

        // Looked at again: the command line may have signed somebody in or
        // out meanwhile.
        cx.observe_window_activation(window, |this, window, cx| {
            this.stirred = cx.background_executor().now();
            this.judge_the_rest(window, cx);
            if window.is_window_active() {
                this.look_again(window, cx);
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
            settings: None,
            storage_work: None,
            storage_done: None,
            menu: None,
            palette: None,
            account_button: cx.focus_handle(),
            signing_out: None,
            crash_open: false,
            report_copied: false,
            rest: Rest::default(),
            stirred: cx.background_executor().now(),
            looking: false,
            pictures: HashMap::new(),
            pictures_in: crate::ui::parts::Shown::new(),
            frame: Morph::still(shape::Shape::Closed),
            frame_since: cx.background_executor().now(),
            pill: (None, None, cx.background_executor().now()),
            pages: Pages::default(),
        }
    }

    /// Sends the frame toward the shape `screen` is drawn in, from wherever
    /// it is: nothing when it is going there already.
    fn aim_the_frame(&mut self, screen: &Screen, cx: &App) {
        let shape = shape::shape_of(screen);
        if self.frame.to() == shape {
            return;
        }
        let now = cx.background_executor().now();
        let elapsed = now.saturating_duration_since(self.frame_since);
        let reduce = Look::now(cx).settings.reduce_motion;
        self.frame = self.frame.toward(shape, elapsed, reduce);
        self.frame_since = now;
    }

    /// The frame on this frame, asking for the next one while anything of
    /// it moves.
    pub(crate) fn frame_now(&mut self, window: &mut Window, cx: &App) -> FrameNow {
        let now = cx.background_executor().now();
        let reduce = Look::now(cx).settings.reduce_motion;
        let elapsed = now.saturating_duration_since(self.frame_since);
        let selected = self
            .flow
            .section()
            .and_then(|place| Place::ALL.iter().position(|each| *each == place));
        if selected != self.pill.1 {
            self.pill = (self.pill.1, selected, now);
        }
        let (from, to, since) = self.pill;
        let sliding = now.saturating_duration_since(since);
        let pill = shape::pill(from, to, sliding, reduce);
        let settled = shape::pill(from, to, motion::INTERACTION, reduce);
        if self.frame.moving(elapsed) || pill != settled {
            window.request_animation_frame();
        }
        FrameNow {
            openness: self.frame.openness(elapsed),
            icons: std::array::from_fn(|at| self.frame.icon(at, elapsed)),
            pill,
            selected,
        }
    }

    /// Starts reading and decoding each account's picture kept on disk, so
    /// a screen that shows one draws it in its first frame, from the cache,
    /// rather than an empty circle first. **A picture written again since it
    /// was read is read again**, and comes in. Nothing is fetched: a picture
    /// is a file of the account's, or nothing.
    fn decode_the_pictures(&mut self, cx: &mut App) {
        // A picture no account names any more (the cache cleared, an account
        // gone) is forgotten, and so is what was decoded of it.
        let named: Vec<&PathBuf> = self
            .accounts
            .iter()
            .filter_map(|a| a.picture.as_ref())
            .collect();
        self.pictures.retain(|path, _| {
            let kept = named.contains(&path);
            if !kept {
                cx.remove_asset::<ImgResourceLoader>(&Resource::from(path.clone()));
            }
            kept
        });
        for account in &self.accounts {
            let Some(path) = account.picture.clone() else {
                continue;
            };
            let resource = Resource::from(path.clone());
            match self.pictures.get_mut(&path) {
                Some(known) if known.kept_at != account.picture_kept_at => {
                    cx.remove_asset::<ImgResourceLoader>(&resource);
                    *known = Picture {
                        kept_at: account.picture_kept_at,
                        arrival: Arrival::Reading,
                    };
                }
                Some(_) => {}
                None => {
                    self.pictures.insert(
                        path,
                        Picture {
                            kept_at: account.picture_kept_at,
                            arrival: Arrival::Unseen,
                        },
                    );
                }
            }
            let _ = cx.fetch_asset::<ImgResourceLoader>(&resource);
        }
    }

    /// How far each account's picture has come in on this frame, asking for
    /// another frame while one is coming in.
    fn see_the_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let lasts = if Look::now(cx).settings.reduce_motion {
            motion::FADE
        } else {
            motion::PICTURE_ARRIVES
        };
        // The splash draws no picture: one read under it is there at once.
        let drawn = self.shown != Screen::Splash;
        let mut coming = false;
        self.pictures_in.clear();
        for account in &self.accounts {
            let Some(path) = &account.picture else {
                continue;
            };
            let read = window
                .use_asset::<ImgResourceLoader>(&Resource::from(path.clone()), cx)
                .is_some();
            let picture = self.pictures.entry(path.clone()).or_insert(Picture {
                kept_at: account.picture_kept_at,
                arrival: Arrival::Unseen,
            });
            picture.arrival = match (picture.arrival, read) {
                (Arrival::Unseen, true) => Arrival::AtOnce,
                (Arrival::Unseen, false) if !drawn => Arrival::Unseen,
                (Arrival::Unseen | Arrival::Reading, false) => Arrival::Reading,
                (Arrival::Reading, true) => Arrival::At(now),
                (arrival, _) => arrival,
            };
            let shown = match picture.arrival {
                Arrival::Unseen | Arrival::Reading => 0.0,
                Arrival::AtOnce => 1.0,
                Arrival::At(since) => {
                    let at =
                        now.saturating_duration_since(since).as_secs_f32() / lasts.as_secs_f32();
                    coming |= at < 1.0;
                    if at < 1.0 { motion::ease_out(at) } else { 1.0 }
                }
            };
            self.pictures_in.insert(account.named.pk, shown);
        }
        if coming {
            window.request_animation_frame();
        }
    }

    /// How far each account's picture has come in on this frame.
    pub fn pictures_shown(&self) -> &crate::ui::parts::Shown {
        &self.pictures_in
    }

    /// Rests the app, or wakes it, as `state::rest` judges what is seen now.
    fn judge_the_rest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seen = rest::Seen {
            looked_at: window.is_window_active(),
            waited_on: self.browser.is_some(),
            still_for: cx
                .background_executor()
                .now()
                .saturating_duration_since(self.stirred),
        };
        loop {
            match self.rest.judge(seen) {
                rest::Next::Rest => self.system.rest(true),
                rest::Next::Wake => self.system.rest(false),
                rest::Next::LookIn(after) => {
                    self.look_at_the_rest_in(after, window, cx);
                    break;
                }
                rest::Next::Stay => break,
            }
        }
    }

    /// Judges the rest again `after` this long, unless a look is already on
    /// its way: that one asks for the next.
    fn look_at_the_rest_in(
        &mut self,
        after: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.looking {
            return;
        }
        self.looking = true;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(after).await;
            this.update_in(cx, |this, window, cx| {
                this.looking = false;
                this.judge_the_rest(window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Whether the app rests now.
    pub fn is_resting(&self) -> bool {
        self.rest.is_resting()
    }

    /// The screen drawn.
    pub fn shown(&self) -> &Screen {
        &self.shown
    }

    /// The screen drawn, as the log names it.
    pub fn screen_name(&self) -> &'static str {
        name(&self.shown)
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

    pub(crate) fn backend(&self) -> &Arc<dyn Backend> {
        &self.backend
    }

    /// Whether the account's menu can be opened now: on the screens of an
    /// app somebody is signed in to, and not on the way to signing in.
    pub(crate) fn has_a_menu(&self) -> bool {
        self.shown.signed_in() && self.flow.screen().signed_in()
    }

    /// Whether the app is where somebody signed in is, whatever is still
    /// drawn on the way there.
    fn is_signed_in(&self) -> bool {
        self.flow.screen().signed_in()
    }

    /// Opens the account's menu, or closes it.
    pub(crate) fn toggle_the_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.close_the_menu(window, cx);
            return;
        }
        if !self.has_a_menu() {
            return;
        }
        tracing::info!(action = "open_account_menu", "action");
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.menu = Some(AccountMenu {
            focus,
            switching: None,
        });
        cx.notify();
    }

    /// Closes the menu and hands focus back to what opened it.
    pub(crate) fn close_the_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            window.focus(&self.account_button, cx);
            cx.notify();
        }
    }

    /// Goes to the next item of the open menu, or the one before, around
    /// its ends: what the arrows do.
    pub(crate) fn move_in_the_menu(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(inside) = self.menu.as_ref().map(|menu| menu.focus.clone()) else {
            return;
        };
        // The tab order is the window's: walked until it is back inside.
        let mut seen = Vec::new();
        loop {
            if forward {
                window.focus_next(cx);
            } else {
                window.focus_prev(cx);
            }
            if inside.contains_focused(window, cx) {
                break;
            }
            let focused = window.focused(cx);
            if seen.contains(&focused) {
                window.focus(&inside, cx);
                break;
            }
            seen.push(focused);
        }
    }

    /// The commands the palette lists: what the screen offers that is what
    /// was typed, each with what it is called.
    pub(crate) fn listed(&self, cx: &App) -> Vec<(Command, String)> {
        let Some(palette) = &self.palette else {
            return Vec::new();
        };
        let typed = palette.input.read(cx).value();
        let look = Look::now(cx);
        let others: Vec<Pk> = self
            .accounts
            .iter()
            .filter(|account| !account.active)
            .map(|account| account.named.pk)
            .collect();
        commands::offered(self.flow.screen(), &others)
            .into_iter()
            .map(|command| (command, super::palette::label(command, self, look)))
            .filter(|(_, label)| commands::matches(label, &typed))
            .collect()
    }

    /// Opens the command palette, or closes it. Not over the question
    /// signing out asks, and not where there is nothing to offer.
    pub(crate) fn toggle_the_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_the_palette(window, cx);
            return;
        }
        let settled = name(&self.shown) == name(self.flow.screen());
        let asking = self
            .settings
            .as_ref()
            .is_some_and(|page| page.forgetting.is_some());
        if !settled
            || self.signing_out.is_some()
            || asking
            || self.a_page_overlay_is_open()
            || commands::offered(self.flow.screen(), &[]).is_empty()
        {
            return;
        }
        tracing::info!(action = "open_command_palette", "action");
        self.close_the_menu(window, cx);
        let placeholder = Look::now(cx).tr("palette-placeholder");
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let watching = cx.subscribe_in(
            &input,
            window,
            |shell, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if let Some(palette) = &mut shell.palette {
                        palette.selected = 0;
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => shell.run_the_selected(window, cx),
                _ => {}
            },
        );
        let back_to = window.focused(cx);
        input.update(cx, |input, cx| input.focus(window, cx));
        self.palette = Some(Palette {
            input,
            selected: 0,
            focus: cx.focus_handle(),
            back_to,
            _watching: watching,
        });
        cx.notify();
    }

    /// Closes the palette and hands focus back to where it was.
    pub(crate) fn close_the_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(palette) = self.palette.take() {
            window.focus(palette.back_to.as_ref().unwrap_or(&self.title), cx);
            cx.notify();
        }
    }

    /// Chooses the command `by` rows further down, around the ends.
    pub(crate) fn palette_moves(&mut self, by: isize, cx: &mut Context<Self>) {
        let listed = self.listed(cx).len() as isize;
        if let Some(palette) = &mut self.palette
            && listed > 0
        {
            palette.selected = (palette.selected as isize + by).rem_euclid(listed) as usize;
            cx.notify();
        }
    }

    /// Enter in the palette: does the command chosen.
    fn run_the_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = self.palette.as_ref().map(|palette| palette.selected);
        let command = chosen.and_then(|at| self.listed(cx).get(at).map(|(command, _)| *command));
        if let Some(command) = command {
            self.run_from_the_palette(command, window, cx);
        }
    }

    /// Closes the palette and does `command`, as its button would.
    pub(crate) fn run_from_the_palette(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_the_palette(window, cx);
        match command {
            Command::SignInBrowser => self.chose(Method::Browser, window, cx),
            Command::SignInPaste => self.chose(Method::Paste, window, cx),
            Command::Go(place) => self.go(place, window, cx),
            Command::Settings => self.open_settings(window, cx),
            Command::AddAccount => self.add_an_account(window, cx),
            Command::SwitchTo(pk) => self.switch_to(pk, window, cx),
            Command::SignOut => self.ask_to_sign_out(window, cx),
            Command::OpenLogs => self.open_the_logs(cx),
            Command::CopyReport => self.copy_the_report(window, cx),
            Command::Profile => self.open_own_profile(window, cx),
        }
    }

    /// Closes whatever is open over the screen: the palette or the question
    /// first, then the menu.
    pub(crate) fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_none() && self.dismiss_a_page_overlay(window, cx) {
            return;
        }
        if self.palette.is_some() {
            self.close_the_palette(window, cx);
        } else if self.paste.as_ref().is_some_and(|form| form.choosing) {
            self.toggle_the_browsers(window, cx);
        } else if self
            .settings
            .as_ref()
            .is_some_and(|page| page.forgetting.is_some())
        {
            self.keep_the_history(window, cx);
        } else if self
            .settings
            .as_ref()
            .is_some_and(|page| page.choosing_language)
        {
            self.toggle_the_languages(window, cx);
        } else if self.signing_out.is_some() {
            self.keep_signed_in(window, cx);
        } else {
            self.close_the_menu(window, cx);
        }
    }

    /// Lists the other accounts under "Switch account".
    pub(crate) fn list_the_other_accounts(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.switching = match menu.switching {
                Some(_) => None,
                None => Some(Instant::now()),
            };
            cx.notify();
        }
    }

    /// Makes `pk` the account in use.
    pub(crate) fn switch_to(&mut self, pk: Pk, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!(action = "switch_account", "action");
        self.close_the_menu(window, cx);
        let switching = self.backend.switch_to(pk);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(accounts) = switching.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.accounts = accounts;
                // The pause is the account's: the band says the new one's.
                this.quiet(Band::clear_pause, cx);
                this.read_the_pause(window, cx);
                this.check_the_session(window, cx);
                if let Some(active) = this.active() {
                    let said = Look::now(cx).tr_with(
                        "account-switched",
                        &[("account", &screens::handle(&active.named))],
                    );
                    this.announce(said, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// On to signing another account in.
    pub(crate) fn add_an_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!(action = "add_account", "action");
        self.close_the_menu(window, cx);
        self.happened(Event::AddAccount, window, cx);
    }

    /// To a section of the app: what its button on the rail does.
    pub(crate) fn go(&mut self, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_signed_in() {
            return;
        }
        tracing::info!(action = "go", "action");
        self.menu = None;
        self.happened(Event::Go(place), window, cx);
    }

    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_a_menu() {
            return;
        }
        tracing::info!(action = "open_settings", "action");
        self.menu = None;
        self.report_copied = false;
        self.happened(Event::OpenSettings, window, cx);
    }

    /// Asks before signing the account in use out.
    pub(crate) fn ask_to_sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.active().map(|account| account.named.clone()) else {
            return;
        };
        self.menu = None;
        let (focus, cancel) = (cx.focus_handle(), cx.focus_handle());
        window.focus(&cancel, cx);
        self.signing_out = Some(SigningOut {
            account,
            focus,
            cancel,
        });
        cx.notify();
    }

    /// The answer was no.
    pub(crate) fn keep_signed_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.signing_out.take().is_some() {
            window.focus(&self.account_button, cx);
            cx.notify();
        }
    }

    /// The answer was yes: the account in use is signed out of this
    /// computer.
    pub(crate) fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(asked) = self.signing_out.take() else {
            return;
        };
        tracing::info!(action = "sign_out", "action");
        let signing_out = self.backend.sign_out(asked.account.pk);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(accounts) = signing_out.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.accounts = accounts;
                this.quiet(Band::clear_pause, cx);
                let next = this.active().map(|account| account.named.clone());
                if next.is_some() {
                    this.read_the_pause(window, cx);
                }
                this.happened(Event::SignedOut { next }, window, cx);
                window.focus(&this.title, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The settings' page, new: nothing typed, at the top.
    fn settings_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> SettingsPage {
        let placeholder = Look::now(cx).tr("settings-search");
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let watching = cx.subscribe_in(&search, window, |shell, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event
                && let Some(page) = &shell.settings
            {
                // What is found is read from the top.
                page.scroll.handle.set_offset(point(px(0.), px(0.)));
                cx.notify();
            }
        });
        self.measure(window, cx);
        SettingsPage {
            search,
            scroll: super::scroll::OutlinedScroll::new(),
            choosing_language: false,
            language_field: cx.focus_handle(),
            language_bounds: Rc::default(),
            language_options: (0..super::screens::settings::LANGUAGES.len())
                .map(|_| cx.focus_handle())
                .collect(),
            outline: None,
            storage: None,
            forgetting: None,
            _watching: watching,
        }
    }

    /// Measures what is kept, for the settings shown.
    fn measure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let measuring = self.backend.storage();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(storage) = measuring.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(page) = &mut this.settings {
                    page.storage = Some(storage);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Removes the cache of `only`, or of every account with `None`, says
    /// what that freed, and measures again. **Asks nothing**: what a view
    /// showed at once is fetched again.
    pub(crate) fn clear_the_cache(
        &mut self,
        only: Option<Pk>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.storage_work.is_some() {
            return;
        }
        tracing::info!(action = "clear_cache", all = only.is_none(), "action");
        self.storage_work = Some(only);
        self.storage_done = None;
        let clearing = self.backend.clear_cache(only);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let cleared = clearing.await.ok();
            this.update_in(cx, |this, window, cx| {
                this.after_working(only, cleared.map(Done::Cleared), window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Asks before deleting the history of `only`, or of every account
    /// with `None`, named `name`.
    pub(crate) fn ask_to_forget(
        &mut self,
        only: Option<Pk>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from = window.focused(cx);
        if self.storage_work.is_some() {
            return;
        }
        let Some(page) = &mut self.settings else {
            return;
        };
        page.choosing_language = false;
        let (focus, cancel) = (cx.focus_handle(), cx.focus_handle());
        window.focus(&cancel, cx);
        page.forgetting = Some(Forgetting {
            only,
            name,
            focus,
            cancel,
            from,
        });
        cx.notify();
    }

    /// The answer was no: nothing is deleted.
    pub(crate) fn keep_the_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(asked) = self
            .settings
            .as_mut()
            .and_then(|page| page.forgetting.take())
        else {
            return;
        };
        if let Some(from) = asked.from {
            window.focus(&from, cx);
        }
        cx.notify();
    }

    /// The answer was yes: the history asked about is deleted, what that
    /// freed is said, and what is kept is measured again.
    pub(crate) fn forget_the_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = &mut self.settings else {
            return;
        };
        let Some(asked) = page.forgetting.take() else {
            return;
        };
        if let Some(from) = &asked.from {
            window.focus(from, cx);
        }
        let only = asked.only;
        if self.storage_work.is_some() {
            return;
        }
        tracing::info!(action = "forget_history", all = only.is_none(), "action");
        self.storage_work = Some(only);
        self.storage_done = None;
        let forgetting = self.backend.forget_history(only);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let forgot = forgetting.await.ok();
            this.update_in(cx, |this, window, cx| {
                this.after_working(only, forgot.map(Done::Forgot), window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// What follows a clearing or a deletion, whatever screen is shown: the
    /// accounts read again, since their pictures were in the cache, and
    /// what it did said; what is kept measured again when the settings are
    /// shown.
    fn after_working(
        &mut self,
        only: Option<Pk>,
        done: Option<Done>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.look_again(window, cx);
        if let Some(done) = done {
            let look = Look::now(cx).clone();
            let said = super::screens::settings::done_words(done, &look);
            self.announce(said, cx);
        }
        self.storage_work = None;
        self.storage_done = done.map(|done| (only, done));
        cx.notify();
        let Some(page) = &mut self.settings else {
            return;
        };
        if let (Some(done), Some(storage)) = (done, &mut page.storage) {
            // Gone until measured again, so nothing is offered on what was
            // just removed.
            let forgot = matches!(done, Done::Forgot(_));
            for account in &mut storage.accounts {
                if only.is_none_or(|pk| pk == account.pk) {
                    account.cache = Default::default();
                    if forgot {
                        account.history = Default::default();
                    }
                }
            }
            if only.is_none() {
                for (kind, size) in &mut storage.kinds {
                    if *kind == Kind::Cache {
                        *size = Default::default();
                    }
                }
            }
        }
        self.measure(window, cx);
    }

    /// Whether the screen shown scrolls on its own, under a header that
    /// stays: the panel then holds still.
    pub(crate) fn scrolls_itself(&self) -> bool {
        (self.settings.is_some() && self.shown == Screen::Settings) || self.a_page_scrolls_itself()
    }

    /// Draws the app in `theme` from now on, whatever the system says, or
    /// with [`ThemeChoice::System`] as the system says.
    pub(crate) fn choose_theme(&mut self, theme: ThemeChoice, cx: &mut Context<Self>) {
        let look = Look::now(cx).clone();
        if look.chosen.theme == theme {
            return;
        }
        tracing::info!(action = "choose_theme", ?theme, "action");
        self.choose(
            Appearance {
                theme,
                ..look.chosen.clone()
            },
            &look,
            cx,
        );
    }

    /// Speaks `language` from now on, or with none the system's.
    pub(crate) fn choose_language(
        &mut self,
        language: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(page) = &mut self.settings {
            page.choosing_language = false;
            window.focus(&page.language_field, cx);
        }
        let look = Look::now(cx).clone();
        if look.chosen.language == language {
            cx.notify();
            return;
        }
        tracing::info!(
            action = "choose_language",
            language = language.as_deref().unwrap_or("system"),
            "action"
        );
        self.choose(
            Appearance {
                language,
                ..look.chosen.clone()
            },
            &look,
            cx,
        );
        let now = Look::now(cx).clone();
        window.set_window_title(&now.tr("window-title"));
        if let Some(page) = &self.settings {
            let placeholder = now.tr("settings-search");
            page.search.update(cx, |search, cx| {
                search.set_placeholder(placeholder, window, cx);
            });
        }
    }

    /// Wears `chosen` over what the system says, and remembers it.
    fn choose(&mut self, chosen: Appearance, look: &Look, cx: &mut Context<Self>) {
        self.backend.remember_appearance(chosen.clone());
        super::look::wear(
            Look::chosen(look.settings.clone(), chosen, look.testing),
            cx,
        );
        cx.notify();
    }

    /// Lists the languages under their field, the one chosen focused, or
    /// stops listing them, focus back on the field.
    pub(crate) fn toggle_the_languages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = Look::now(cx).chosen.language.clone();
        let Some(page) = &mut self.settings else {
            return;
        };
        page.choosing_language = !page.choosing_language;
        if page.choosing_language {
            let at = super::screens::settings::LANGUAGES
                .iter()
                .position(|(tag, _)| *tag == chosen.as_deref())
                .unwrap_or(0);
            window.focus(&page.language_options[at], cx);
        } else {
            window.focus(&page.language_field, cx);
        }
        cx.notify();
    }

    /// The languages stop being listed, as a press outside them does: focus
    /// stays where the press put it.
    pub(crate) fn close_the_languages(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = &mut self.settings
            && page.choosing_language
        {
            page.choosing_language = false;
            cx.notify();
        }
    }

    /// Turns the log's detailed level on or off.
    pub(crate) fn set_detailed(&mut self, on: bool, cx: &mut Context<Self>) {
        tracing::info!(action = "set_detailed_logging", on, "action");
        self.backend.set_detailed(on);
        cx.notify();
    }

    pub(crate) fn open_the_logs(&mut self, cx: &mut Context<Self>) {
        tracing::info!(action = "open_logs_folder", "action");
        let folder = self.backend.logs_dir();
        self.system.open_folder(&folder, cx);
    }

    /// Copies the diagnostic report, and says so.
    pub(crate) fn copy_the_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!(action = "copy_diagnostic_report", "action");
        let writing = self.backend.diagnostic_report();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(report) = writing.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.system.copy(&report, cx);
                this.report_copied = true;
                let said = Look::now(cx).tr("settings-report-copied");
                this.announce(said, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Copies the report of the last crash.
    pub(crate) fn copy_the_crash(&mut self, cx: &mut Context<Self>) {
        tracing::info!(action = "copy_crash_report", "action");
        if let Some(report) = self.crash.clone() {
            self.system.copy(&report, cx);
            let said = Look::now(cx).tr("crash-copied");
            self.announce(said, cx);
        }
    }

    /// The crash was seen: it is not offered again.
    pub(crate) fn dismiss_the_crash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!(action = "dismiss_crash_report", "action");
        self.crash = None;
        self.crash_open = false;
        self.backend.crash_shown();
        window.focus(&self.title, cx);
        cx.notify();
    }

    fn loaded(&mut self, loaded: Loaded, window: &mut Window, cx: &mut Context<Self>) {
        self.accounts = loaded.accounts;
        // Under the splash's way out, so home's first frame has them.
        self.decode_the_pictures(cx);
        self.crash = loaded.crash;
        let signed_in = matches!(loaded.found, Found::SignedIn(_));
        if signed_in {
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
        if signed_in {
            self.check_the_session(window, cx);
        }
    }

    /// Reads again who is signed in on this computer, **from files alone**,
    /// where the answer can change what is shown: at home, in the settings,
    /// and where nobody is signed in yet.
    fn look_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let waiting = matches!(self.flow.screen(), Screen::Start { back: false, .. });
        let settled = name(&self.shown) == name(self.flow.screen());
        if !settled || !(self.is_signed_in() || waiting) {
            return;
        }
        let loading = self.backend.load();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(loaded) = loading.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.found_again(loaded, window, cx))
                .ok();
        })
        .detach();
    }

    /// What a second look found: another account in use, nobody left, or
    /// somebody signed in from the command line.
    fn found_again(&mut self, loaded: Loaded, window: &mut Window, cx: &mut Context<Self>) {
        let waiting = matches!(self.flow.screen(), Screen::Start { back: false, .. });
        match (self.is_signed_in(), loaded.found) {
            (true, Found::SignedIn(now)) => {
                let before = self.active().map(|account| account.named.pk);
                self.accounts = loaded.accounts;
                self.decode_the_pictures(cx);
                if before != Some(now.pk) {
                    tracing::info!("the account in use was changed from outside the app");
                    self.quiet(Band::clear_pause, cx);
                    self.read_the_pause(window, cx);
                    let said = Look::now(cx)
                        .tr_with("account-switched", &[("account", &screens::handle(&now))]);
                    self.announce(said, cx);
                }
                cx.notify();
            }
            (true, Found::Nobody) => {
                tracing::info!("everybody was signed out from outside the app");
                self.accounts = loaded.accounts;
                (self.menu, self.signing_out, self.palette) = (None, None, None);
                self.quiet(Band::clear_pause, cx);
                self.happened(Event::SignedOut { next: None }, window, cx);
            }
            (true, Found::Expired(account)) => self.session_expired(account, window, cx),
            (false, Found::SignedIn(now)) if waiting => {
                tracing::info!("somebody signed in from outside the app");
                self.accounts = loaded.accounts;
                self.decode_the_pictures(cx);
                self.read_the_pause(window, cx);
                self.happened(Event::SignedIn(now), window, cx);
            }
            _ => {}
        }
    }

    /// Asks whether the session of the account in use still works, behind
    /// what is shown: the backend asks Instagram once a day at most, and
    /// here nothing is asked while the system says there is no network.
    fn check_the_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_signed_in() || self.system.online() == Some(false) {
            return;
        }
        let checking = self.backend.expired_session();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(expired) = checking.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| match expired {
                Some(account) => this.session_expired(account, window, cx),
                // A check that went out kept what the account's tab said
                // of it, a new picture included: read from the files.
                None => this.look_again(window, cx),
            })
            .ok();
        })
        .detach();
    }

    /// The session of the account in use stopped working: it is asked back,
    /// and somebody not looking at the window is told by the system.
    fn session_expired(&mut self, account: Named, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_signed_in() {
            return;
        }
        self.menu = None;
        self.signing_out = None;
        self.palette = None;
        if !window.is_window_active() {
            let look = Look::now(cx);
            let body = match &account.username {
                Some(_) => look.tr_with(
                    "notify-expired-body",
                    &[("account", &screens::handle(&account))],
                ),
                None => look.tr("notify-expired-body-unnamed"),
            };
            let notification = system::Notification {
                about: system::Notice::SessionExpired,
                title: look.tr("notify-expired-title"),
                body,
            };
            self.system.notify(notification, cx);
        }
        self.happened(Event::Expired(account), window, cx);
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
            if !window.is_window_active() {
                let look = Look::now(cx);
                let notification = system::Notification {
                    about: system::Notice::UpdateReady,
                    title: look.tr("notify-update-title"),
                    body: look.tr("notify-update-body"),
                };
                self.system.notify(notification, cx);
            }
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
        let looking = self.backend.browsers();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(browsers) = looking.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(form) = &mut this.paste {
                    form.browser = browsers
                        .last
                        .and_then(|last| browsers.found.iter().position(|name| *name == last))
                        .unwrap_or(0);
                    form.browsers = Some(browsers.found);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        PasteForm {
            input,
            revealed: false,
            judged: None,
            checking: false,
            edits: 0,
            pasting: false,
            browsers: None,
            browser: 0,
            choosing: false,
            browser_field: cx.focus_handle(),
            _watching: watching,
        }
    }

    /// Opens the list of browsers under the field, or closes it, focus
    /// back on the field.
    pub(crate) fn toggle_the_browsers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.paste {
            form.choosing = !form.choosing;
            if !form.choosing {
                window.focus(&form.browser_field, cx);
            }
            cx.notify();
        }
    }

    /// The session is said to come from the browser listed at `at`.
    pub(crate) fn choose_the_browser(
        &mut self,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = &mut self.paste {
            tracing::info!(action = "choose_browser", "action");
            form.browser = at;
            form.choosing = false;
            window.focus(&form.browser_field, cx);
            cx.notify();
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
        form.choosing = false;
        let browser = form
            .browsers
            .as_ref()
            .and_then(|found| found.get(form.browser).cloned());
        let session_id = Secret::new(value.trim().to_string());
        form.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.announce(Look::now(cx).tr("paste-checking"), cx);

        self.attempt += 1;
        let attempt = self.attempt;
        let signing_in = self.backend.sign_in_with(session_id, browser);
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
                self.decode_the_pictures(cx);
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
        // A page of another account is another page: it comes in.
        if name(next) == name(&self.shown) && self.leaving.is_none() && !next.is_a_page() {
            self.shown = next.clone();
            cx.notify();
            return;
        }
        // The frame changes shape as the screen shown leaves; under "reduce
        // motion" at once, once it has (`enter`). **The first screen after
        // the splash is drawn in its shape**: nothing opens at a start.
        let next = next.clone();
        if self.shown != Screen::Splash && !Look::now(cx).settings.reduce_motion {
            self.aim_the_frame(&next, cx);
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
        let before = std::mem::replace(&mut self.shown, self.flow.screen().clone());
        if before == Screen::Splash {
            self.frame = Morph::still(shape::shape_of(&self.shown));
            self.frame_since = cx.background_executor().now();
        } else {
            let shown = self.shown.clone();
            self.aim_the_frame(&shown, cx);
        }
        self.entered = Instant::now();
        self.scroll.set_offset(point(px(0.), px(0.)));
        tracing::info!(screen = name(&self.shown), "screen");
        window.focus(&self.title, cx);
        self.paste = match self.shown {
            Screen::Paste => Some(self.paste_form(window, cx)),
            _ => None,
        };
        self.settings = match self.shown {
            Screen::Settings => Some(self.settings_page(window, cx)),
            _ => None,
        };
        self.menu = None;
        self.signing_out = None;
        self.settle_the_pages(window, cx);

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
                let enter = if matches!(self.shown, Screen::Welcome(_)) {
                    motion::WELCOME_ENTER
                } else {
                    motion::ENTER
                };
                let moved = look.moved(enter, if splash { 0.0 } else { 14.0 }, 1.0);
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
                // **What was chosen over the system holds**: a theme chosen
                // is not moved by the system's.
                let now = Look::now(cx);
                let (chosen, testing) = (now.chosen.clone(), now.testing);
                super::look::wear(Look::chosen(settings, chosen, testing), cx);
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
                self.check_the_session(window, cx);
            }
            system::Event::ShuttingDown => tracing::info!("the session is ending"),
            system::Event::Online(online) => {
                tracing::info!(online, "the network changed");
                if online {
                    self.quiet(Band::clear_offline, cx);
                    self.check_the_session(window, cx);
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

/// How a picture came to be drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Arrival {
    /// Not asked for in a frame yet.
    Unseen,
    /// Asked for, and still being read.
    Reading,
    /// Read before the first frame that showed it: drawn at once.
    AtOnce,
    /// Read while a frame waited for it, at this moment: it comes in.
    At(Instant),
}

/// A picture the app draws: when its file was written, and how it came.
#[derive(Debug, Clone, Copy)]
struct Picture {
    kept_at: Option<SystemTime>,
    arrival: Arrival,
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A frame is something moving: the app wakes before it is drawn.
        self.stirred = cx.background_executor().now();
        self.judge_the_rest(window, cx);
        self.see_the_pictures(window, cx);
        let phase = self.phase(window, cx);
        let screen: AnyElement = screens::render(self, phase, window, cx);
        frame::render(self, screen, window, cx)
    }
}
