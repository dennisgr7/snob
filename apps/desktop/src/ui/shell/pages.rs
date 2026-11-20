//! The pages a section opens: an account's profile, its lists, the picture
//! large over them, and what each does. **A list of another account is
//! asked about first**, in the app's own question, and only a yes walks it
//! (`state::profile::asks_first`). **Nothing is read twice on the way
//! back**: a page keeps what it read for as long as the way back holds it.
//! What the cache kept of a profile, its highlights and its tabs' first
//! pages is shown at once, and the band says it is loading until the reads
//! it makes anyway answer: **the cache never decides whether to read**. A
//! tab's next page is asked for one at a time, from the cursor the backend
//! handed out, and stopped when the page is left; **the engine sets the
//! pace**, which the page only shows.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, App, Bounds, Context, Entity, FocusHandle, Pixels, Point, ScrollHandle,
    SharedString, UniformListScrollHandle, WeakEntity, Window,
};
use snob_core::{Epoch, Pk};
use snob_ig::pace::CancelToken;
use tokio::sync::{mpsc, watch};

use super::Shell;
use crate::backend::{
    Backend, Cursor, Listed, MOST_PICTURES, Person, Profile, Tiles, Unread, Walked,
};
use crate::state::flow::{Event, Named, Screen};
use crate::state::people;
use crate::state::profile::{self, Layout, PictureClick, Rect, Run, Showing, Side, Tab, Way, Zoom};
use crate::state::status::Status;
use crate::theme::motion;
use crate::theme::tokens::frame;
use crate::ui::look::Look;

/// How a tab's first page stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TabRead {
    Reading,
    Read(Tiles),
    Failed(Unread),
}

/// What a frame's sizes were measured from: a page asks for no more from
/// sizes of something else (`profile::ReadOn::measured`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Measured {
    pub tab: Tab,
    pub layout: Layout,
    pub showing: Showing,
    pub posts: usize,
    pub window: gpui_kit::Size<Pixels>,
    pub text_scale: f32,
}

/// One page past a tab's first asked for: which ask it is, so an answer
/// finds the page that asked, and what stops it.
#[derive(Clone)]
pub(crate) struct Reading {
    id: u64,
    stop: CancelToken,
}

/// Every page past a tab's first asked for this run, for its id.
static READS: AtomicU64 = AtomicU64::new(0);

/// How a tab reads on past its first page. **The cursor stays in the tab's
/// [`Tiles::next`]** until a page comes, so a page failed is asked again
/// from where it was.
#[derive(Clone, Default)]
pub(crate) struct Further {
    /// The page on its way, until it answers: **a page stopped stays on its
    /// way**, since the engine may have spent its cursor and still answer
    /// with it, and is shown when it comes.
    pub reading: Option<Reading>,
    /// Until when the engine said it rests before reading it.
    pub resting_until: Option<Epoch>,
    /// Why the last page asked for was not read, until somebody asks again.
    pub failed: Option<Unread>,
    /// When each page past the first came: the posts from that one on.
    pub came: Vec<(usize, Instant)>,
    /// What the last frame drawn was.
    pub measured: Option<Measured>,
}

impl Further {
    /// The pages shown, the first included.
    pub fn pages(&self) -> u32 {
        u32::try_from(self.came.len()).map_or(u32::MAX, |came| came.saturating_add(1))
    }

    /// Stops the page on its way, which stays on its way until it answers,
    /// and forgets what was measured: the next frame measures again.
    fn stop(&mut self) {
        if let Some(reading) = &self.reading {
            reading.stop.cancel();
        }
        self.resting_until = None;
        self.measured = None;
    }

    /// Stops the page on its way and lets go of it: what it answers is not
    /// shown.
    fn let_go(&mut self) {
        self.stop();
        self.reading = None;
    }

    fn is_reading(&self, id: u64) -> bool {
        self.reading
            .as_ref()
            .is_some_and(|reading| reading.id == id)
    }
}

/// A profile while the way back holds it: what was kept of it, then what
/// was read, and each tab shown.
pub(crate) struct ProfilePage {
    pub who: Named,
    pub profile: Option<Profile>,
    /// Since when the read came: what it changed comes in.
    pub read_at: Option<Instant>,
    pub failed: Option<Unread>,
    pub tab: Tab,
    /// The tab the underline leaves, and since when.
    pub tab_from: (Tab, Instant),
    pub tabs: HashMap<Tab, TabRead>,
    /// How each tab read reads on.
    pub further: HashMap<Tab, Further>,
    /// The first page of each tab the cache kept, shown until the tab is
    /// read.
    pub kept_tabs: HashMap<Tab, Tiles>,
    /// Since when the posts shown are the tab's they are: they come in
    /// again, one after the other, when that changes.
    pub shown_since: Instant,
    /// Since when the highlights are the ones shown: they come in when a
    /// read changes them, not when it says them again.
    pub highlights_since: Option<Instant>,
    /// The posts' own scroll in the split layout.
    pub scroll: ScrollHandle,
    pub highlights: ScrollHandle,
    /// What opens the picture and what opens the options: where focus goes
    /// back when what they opened closes.
    pub picture_button: FocusHandle,
    pub options_button: FocusHandle,
    /// Where each was last drawn: what its menu opens under.
    pub picture_drawn: Rc<Cell<Bounds<Pixels>>>,
    pub options_drawn: Rc<Cell<Bounds<Pixels>>>,
    /// What the page's scrolling left in sight where the picture was last
    /// drawn.
    pub picture_clip: Rc<Cell<Bounds<Pixels>>>,
}

impl ProfilePage {
    /// Stops every page on its way: what leaving the page does.
    fn stop_reading_on(&mut self) {
        for further in self.further.values_mut() {
            further.stop();
        }
    }
}

impl Drop for ProfilePage {
    fn drop(&mut self) {
        self.stop_reading_on();
    }
}

/// One of an account's lists: what was stored of it, shown at once, then
/// the rows its read brings. **Read on until nothing shows it any more**:
/// its page left (kept in memory, `state::people::Parked`) or its window
/// closed, it stops, and what it read is kept for the next time.
pub struct PeopleList {
    pub owner: Named,
    pub side: Side,
    /// The account in use when it was opened, whose list it is shown to.
    pub viewer: Pk,
    /// The rows shown.
    pub people: Vec<Person>,
    /// What the rows are the answer of, once they are a whole one: stored,
    /// or read to its end. **None while they come in as a first read
    /// reads them.**
    pub answered: Option<people::Answered>,
    /// The read going, as far as it is.
    pub reading: Option<Walked>,
    pub failed: Option<Unread>,
    /// When each run of rows came: the first `n` by then.
    pub arrived: Vec<(usize, Instant)>,
    /// Its own window, once it was moved out to one.
    pub window: Option<AnyWindowHandle>,
    pub scroll: UniformListScrollHandle,
    /// The rows drawn on the last frame.
    pub drawn: Rc<Cell<Range<usize>>>,
    /// Since when each picture downloaded while it is shown came in.
    pub came: HashMap<Pk, Instant>,
    pub selection: people::Selection,
    /// How many usernames the last copy put on the clipboard, until the
    /// rows chosen change.
    pub copied: Option<usize>,
    /// Its options, while they are open.
    pub menu: Option<ListMenu>,
    pub options_button: FocusHandle,
    pub options_drawn: Rc<Cell<Bounds<Pixels>>>,
    /// Left, and kept in memory: it reads nothing.
    parked: bool,
    /// The address each row's picture was asked for at, this showing.
    tried: HashMap<Pk, String>,
    /// The rows the batch of pictures on its way asked for that have not
    /// come yet: **a batch stopped gives them back to be asked for again**.
    asking: HashSet<Pk>,
    /// The rows drawn when that batch was asked for.
    asked_for: Range<usize>,
    /// Which batch of pictures is the last asked for.
    batch: u64,
    /// Whether to look for pictures again once the batch on its way ends.
    look_after: bool,
    /// Every picture known this run, which a row read again takes.
    pictures: HashMap<Pk, PathBuf>,
    /// Which read the rows heard are of: an answer of one let go is not
    /// shown.
    read_id: u64,
    /// Which look for pictures is the last: one the rows moved after asks
    /// nothing.
    looked: u64,
    stop: CancelToken,
    picture_stop: Option<CancelToken>,
    backend: Arc<dyn Backend>,
    shell: WeakEntity<Shell>,
}

/// A list's options, open under their button.
pub struct ListMenu {
    pub focus: FocusHandle,
    pub since: Instant,
}

impl PeopleList {
    fn new(
        owner: Named,
        side: Side,
        viewer: Pk,
        backend: Arc<dyn Backend>,
        shell: WeakEntity<Shell>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            owner,
            side,
            viewer,
            people: Vec::new(),
            answered: None,
            reading: None,
            failed: None,
            arrived: Vec::new(),
            window: None,
            scroll: UniformListScrollHandle::new(),
            drawn: Rc::default(),
            came: HashMap::new(),
            selection: people::Selection::default(),
            copied: None,
            menu: None,
            options_button: cx.focus_handle(),
            options_drawn: Rc::default(),
            parked: false,
            tried: HashMap::new(),
            asking: HashSet::new(),
            asked_for: 0..0,
            batch: 0,
            look_after: false,
            pictures: HashMap::new(),
            read_id: 0,
            looked: 0,
            stop: CancelToken::default(),
            picture_stop: None,
            backend,
            shell,
        }
    }

    /// When the row at `at` came.
    pub fn arrived_at(&self, at: usize) -> Option<Instant> {
        self.arrived
            .iter()
            .find(|(until, _)| at < *until)
            .map(|(_, when)| *when)
    }

    /// The rows drawn on the last frame.
    fn drawn(&self) -> Range<usize> {
        let drawn = self.drawn.take();
        self.drawn.set(drawn.clone());
        drawn
    }

    /// What the line under its title says.
    pub fn said(&self) -> people::Said {
        let going = self.reading.as_ref().map(|walked| people::Going {
            read: walked.people.len(),
            expected: walked.expected,
        });
        people::said(self.answered, going, self.failed.is_some())
    }

    /// What was stored of it, shown at once while nothing else is.
    fn show_kept(&mut self, listed: Listed, cx: &mut Context<Self>) {
        if !self.people.is_empty() || self.answered.is_some() {
            return;
        }
        self.answered = Some(answered_of(&listed));
        self.people = listed.people;
        self.take_the_pictures();
        cx.notify();
        self.look_for_pictures(cx);
    }

    /// A read starts, under `stop`: what it hears is told by its id.
    fn start_reading(&mut self, stop: CancelToken) -> u64 {
        self.stop.cancel();
        self.stop = stop;
        self.read_id += 1;
        self.reading = Some(Walked::default());
        self.failed = None;
        self.read_id
    }

    /// What the read `id` has read so far. **Rows a whole answer shows stay
    /// until the read ends**; a first read's come in as they are read.
    fn heard(&mut self, id: u64, walked: Walked, cx: &mut Context<Self>) {
        if id != self.read_id || self.reading.is_none() {
            return;
        }
        // A walk shown again after it was left starts over: the rows it
        // had read stay until it has read as many.
        if self.answered.is_none() && walked.people.len() >= self.people.len() {
            if walked.people.len() > self.people.len() {
                self.arrived.push((walked.people.len(), Instant::now()));
            }
            self.people.clone_from(&walked.people);
            self.take_the_pictures();
            self.look_for_pictures(cx);
        }
        self.reading = Some(walked);
        cx.notify();
    }

    /// The read `id` ended: its rows replace those shown, the row at the
    /// top staying there and what was chosen staying chosen. One that failed
    /// leaves the rows shown and says why. Whether it was this list's read.
    fn read_ended(
        &mut self,
        id: u64,
        ended: Result<Listed, Unread>,
        cx: &mut Context<Self>,
    ) -> bool {
        if id != self.read_id || self.reading.is_none() {
            return false;
        }
        self.reading = None;
        match ended {
            Ok(listed) => {
                let before: Vec<Pk> = self.people.iter().map(|person| person.named.pk).collect();
                let after: Vec<Pk> = listed.people.iter().map(|person| person.named.pk).collect();
                if self.answered.is_some()
                    && let Some(by) = people::moved_by(self.drawn().start, &before, &after)
                {
                    self.scroll_rows(by, before.len());
                }
                if self.answered.is_none() && listed.people.len() > self.people.len() {
                    self.arrived.push((listed.people.len(), Instant::now()));
                }
                self.answered = Some(answered_of(&listed));
                self.people = listed.people;
                self.selection.keep_among(after);
                self.take_the_pictures();
                self.look_for_pictures(cx);
            }
            Err(why) => self.failed = Some(why),
        }
        cx.notify();
        true
    }

    /// Moves the scroll by `rows` rows of the `count` laid out last, as far
    /// into the row at the top as it was.
    fn scroll_rows(&self, rows: isize, count: usize) {
        let state = self.scroll.0.borrow();
        // `item` is the list's own size; `contents`, all its rows'.
        let Some(size) = state.last_item_size.filter(|_| count > 0) else {
            return;
        };
        let offset = state.base_handle.offset();
        let by = size.contents.height / count as f32 * rows as f32;
        state
            .base_handle
            .set_offset(gpui_kit::point(offset.x, offset.y - by));
    }

    /// Gives each row with no picture the one known of it.
    fn take_the_pictures(&mut self) {
        for person in &mut self.people {
            if person.picture.is_none()
                && let Some(picture) = self.pictures.get(&person.named.pk)
            {
                person.picture = Some(picture.clone());
            }
        }
    }

    /// Asks for the pictures of the rows drawn once they have stayed the
    /// same for `state::people::SETTLES_AFTER`: a scroll going on asks for
    /// nothing.
    pub fn look_for_pictures(&mut self, cx: &mut Context<Self>) {
        if self.parked {
            return;
        }
        self.looked += 1;
        let looked = self.looked;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(people::SETTLES_AFTER).await;
            this.update(cx, |list, cx| {
                if list.looked == looked {
                    list.ask_for_pictures(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks the backend for the small pictures of the rows drawn and
    /// `state::people::AROUND` either side: **one download each from the
    /// CDN, never the API, and never while the app rests**, which looks
    /// again once it wakes. A picture that did not come keeps the row's
    /// initial and is not asked for again this showing. **One batch at a
    /// time**, so no more than the engine's own few downloads run at once:
    /// one wanted meanwhile waits for it to end, and stops it first when
    /// the rows drawn moved.
    fn ask_for_pictures(&mut self, cx: &mut Context<Self>) {
        let resting = self
            .shell
            .upgrade()
            .is_some_and(|shell| shell.read(cx).is_resting());
        if self.parked || resting {
            return;
        }
        let drawn = self.drawn();
        let mut wanted =
            people::wanted_pictures(&self.people, drawn.clone(), &self.tried, |person| {
                let from = person
                    .picture
                    .is_none()
                    .then_some(person.picture_from.as_deref());
                (person.named.pk, from.flatten())
            });
        if wanted.is_empty() {
            return;
        }
        if self.picture_stop.is_some() {
            if drawn != self.asked_for {
                self.let_go_of_the_pictures();
            }
            self.look_after = true;
            return;
        }
        // The backend reads no more of a batch: the rest is asked next.
        wanted.truncate(MOST_PICTURES);
        for (pk, from) in &wanted {
            self.tried.insert(*pk, from.clone());
        }
        self.asking = wanted.iter().map(|(pk, _)| *pk).collect();
        self.asked_for = drawn;
        self.batch += 1;
        let batch = self.batch;
        let stop = CancelToken::default();
        self.picture_stop = Some(stop.clone());
        let (came, mut heard) = mpsc::unbounded_channel();
        let answer = self.backend.pictures(wanted, came, stop);
        cx.spawn(async move |this, cx| {
            while let Some((pk, picture, fetched)) = heard.recv().await {
                let told = this.update(cx, |list, cx| list.picture_came(pk, picture, fetched, cx));
                if told.is_err() {
                    return;
                }
            }
            let _ = answer.await;
            this.update(cx, |list, cx| list.pictures_ended(batch, cx))
                .ok();
        })
        .detach();
    }

    /// The batch `batch` ended: what it did not bring could not be had, and
    /// a look waiting for it asks now.
    fn pictures_ended(&mut self, batch: u64, cx: &mut Context<Self>) {
        if batch != self.batch {
            return;
        }
        self.picture_stop = None;
        self.asking.clear();
        if std::mem::take(&mut self.look_after) {
            self.ask_for_pictures(cx);
        }
    }

    /// Stops the batch on its way, which ends once its downloads going end:
    /// the rows it has not brought are asked for again.
    fn let_go_of_the_pictures(&mut self) {
        if let Some(stop) = &self.picture_stop {
            stop.cancel();
        }
        for pk in self.asking.drain() {
            self.tried.remove(&pk);
        }
    }

    /// A row's picture is on disk: it comes in when it was downloaded now,
    /// and is there at once when it was kept.
    fn picture_came(&mut self, pk: Pk, picture: PathBuf, fetched: bool, cx: &mut Context<Self>) {
        self.asking.remove(&pk);
        self.pictures.insert(pk, picture.clone());
        if let Some(person) = self.people.iter_mut().find(|person| person.named.pk == pk) {
            person.picture = Some(picture);
        }
        let now = Instant::now();
        self.came
            .retain(|_, since| now.saturating_duration_since(*since) < Duration::from_secs(1));
        if fetched {
            self.came.insert(pk, now);
        }
        cx.notify();
    }

    /// Left, and kept in memory: its read and its pictures stop, its menu
    /// closes; its rows, scroll and choice stay.
    fn park(&mut self) {
        self.parked = true;
        self.stop.cancel();
        self.reading = None;
        self.let_go_of_the_pictures();
        self.look_after = false;
        self.menu = None;
    }

    /// Shown again: a picture that did not come is asked for again.
    fn unpark(&mut self, cx: &mut Context<Self>) {
        self.parked = false;
        self.tried.clear();
        self.came.clear();
        self.look_for_pictures(cx);
    }

    /// Opens its options, or closes them.
    pub fn toggle_the_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.close_the_menu(window, cx);
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.menu = Some(ListMenu {
            focus,
            since: Instant::now(),
        });
        cx.notify();
    }

    /// Closes its options, focus back on their button.
    pub fn close_the_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            window.focus(&self.options_button, cx);
            cx.notify();
        }
    }

    /// Goes to the next of its options, or the one before, around the ends.
    pub fn move_in_the_menu(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(inside) = self.menu.as_ref().map(|menu| menu.focus.clone()) else {
            return;
        };
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

    /// Starts choosing rows, from its options: **nothing is sent for it**.
    pub fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!(action = "choose_in_list", "action");
        self.close_the_menu(window, cx);
        self.selection.start();
        self.copied = None;
        cx.notify();
    }

    /// Chooses the row `pk`, or leaves it.
    pub fn toggle(&mut self, pk: Pk, cx: &mut Context<Self>) {
        self.selection.toggle(pk);
        self.copied = None;
        cx.notify();
    }

    /// Chooses every row shown.
    pub fn choose_all(&mut self, cx: &mut Context<Self>) {
        self.selection
            .all(self.people.iter().map(|person| person.named.pk));
        self.copied = None;
        cx.notify();
    }

    /// Stops choosing, focus back on the options' button.
    pub fn stop_choosing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.stop();
        self.copied = None;
        window.focus(&self.options_button, cx);
        cx.notify();
    }

    /// Closes what of it is open, its options first, then the choosing:
    /// whether anything was.
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.menu.is_some() {
            self.close_the_menu(window, cx);
        } else if self.selection.choosing() {
            self.stop_choosing(window, cx);
        } else {
            return false;
        }
        true
    }

    /// The usernames of the rows chosen, one a line, and how many.
    pub fn chosen_usernames(&self) -> (String, usize) {
        self.selection.usernames(
            self.people
                .iter()
                .map(|person| (person.named.pk, person.named.username.as_deref())),
        )
    }
}

impl Drop for PeopleList {
    fn drop(&mut self) {
        // What it read is saved page by page: stopping keeps it.
        self.stop.cancel();
        if let Some(stop) = &self.picture_stop {
            stop.cancel();
        }
    }
}

/// What a list answered whole says of itself.
fn answered_of(listed: &Listed) -> people::Answered {
    people::Answered {
        count: listed.people.len(),
        whole: listed.whole,
        taken_at: listed.taken_at,
    }
}

/// The picture of an account, large over the screen.
pub(crate) struct PictureViewer {
    pub who: Named,
    /// The small picture, shown until the large one is drawn.
    pub small: Option<PathBuf>,
    pub large: Option<PathBuf>,
    /// When the large picture starts coming in over the small one: once it
    /// is decoded, and not before the picture has arrived.
    pub large_since: Option<Instant>,
    pub zoom: Zoom,
    /// How it comes from the page, and since when.
    pub way: Way,
    pub run: Run,
    pub since: Instant,
    /// Where the page draws the picture, as it is drawn now, and what of
    /// that its scrolling leaves in sight.
    pub on_the_page: Rc<Cell<Bounds<Pixels>>>,
    pub clip: Rc<Cell<Bounds<Pixels>>>,
    pub focus: FocusHandle,
    /// Where a drag that moves the picture started, while one does.
    pub dragging: Option<Point<Pixels>>,
    /// Where the menu of a right click is, while it is open.
    pub menu: Option<Point<Pixels>>,
    pub menu_focus: FocusHandle,
    /// What was last done, said under the buttons.
    pub said: Option<SharedString>,
    /// What had focus when it opened, where it goes back.
    pub back_to: Option<FocusHandle>,
}

/// The picture on its way back to the page once its viewer closed: drawn
/// alone, with nothing to act on and nothing a screen reader is told, while
/// focus is already back on the page.
pub(crate) struct LeavingPicture {
    pub who: Named,
    pub small: Option<PathBuf>,
    pub large: Option<PathBuf>,
    /// How far the large picture had come in over the small one.
    pub large_shown: f32,
    /// How far it was zoomed and moved: what it turns back to if opened
    /// again on its way.
    pub zoom: Zoom,
    pub way: Way,
    pub run: Run,
    pub since: Instant,
    /// The viewer's end of its way: where it was drawn there as it closed,
    /// zoomed or not, and how round.
    pub end: Rect,
    pub end_corner: f32,
    /// How much of it was shown as it closed: what a picture that fades
    /// fades from.
    pub shown: f32,
    pub on_the_page: Rc<Cell<Bounds<Pixels>>>,
    /// Drawn at its end once: the next frame draws the page's own.
    pub done: bool,
}

/// Which small menu is open over a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopupKind {
    /// A story is up: the story or the picture.
    Picture,
    /// What can be done with the account, or known of it.
    Options,
}

/// A small menu over a profile, while it is open.
pub(crate) struct Popup {
    pub kind: PopupKind,
    pub focus: FocusHandle,
    pub since: Instant,
}

/// The question reading another account's list asks first.
pub(crate) struct ListQuestion {
    pub owner: Named,
    pub side: Side,
    pub focus: FocusHandle,
    /// The answer focus starts on: the one that reads nothing.
    pub cancel: FocusHandle,
    pub from: Option<FocusHandle>,
}

/// "About this account", open over a profile: its picture, its username and
/// what is known of it, **none of which is read yet** (`profile::About`).
pub(crate) struct AboutAccount {
    pub who: Named,
    pub full_name: Option<String>,
    pub picture: Option<PathBuf>,
    pub known: profile::About,
    pub focus: FocusHandle,
    /// Its button that closes it, where focus starts.
    pub close: FocusHandle,
}

/// Everything the pages hold, apart from the rest of the shell.
#[derive(Default)]
pub(crate) struct Pages {
    pub profiles: HashMap<Pk, ProfilePage>,
    pub lists: HashMap<(Pk, Side), Entity<PeopleList>>,
    pub viewer: Option<PictureViewer>,
    pub leaving: Option<LeavingPicture>,
    pub popup: Option<Popup>,
    pub asking: Option<ListQuestion>,
    pub about: Option<AboutAccount>,
    /// How profiles are laid out and their posts shown, the same for every
    /// one this run.
    pub layout: Layout,
    pub showing: Showing,
    /// The way posts were shown before, and since when they are shown as
    /// they are: the words naming it cross, and the posts come in again.
    pub showing_from: Option<(Showing, Instant)>,
    /// Since when the layout is what it is: the page comes in again.
    pub laid_out_at: Option<Instant>,
    /// The lists moved out to windows of their own: **one walk per list**,
    /// so opening one of them again shows the one going.
    pub detached: Vec<WeakEntity<PeopleList>>,
    /// The lists left, kept in memory to be shown again at once: of the
    /// account in use only.
    pub parked: people::Parked<(Pk, Side)>,
    pub parked_lists: HashMap<(Pk, Side), Entity<PeopleList>>,
    /// The list another account's yes was just given for: **the one list
    /// of another account that may be read when it is shown next**.
    pub consented: Option<(Pk, Side)>,
}

impl Shell {
    /// To the profile of the account in use.
    pub(crate) fn open_own_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.active().map(|account| account.named.clone()) else {
            return;
        };
        self.close_the_menu(window, cx);
        self.open_profile(account, window, cx);
    }

    /// To `who`'s profile.
    pub(crate) fn open_profile(&mut self, who: Named, window: &mut Window, cx: &mut Context<Self>) {
        if !self.flow.screen().signed_in() {
            return;
        }
        tracing::info!(action = "open_profile", "action");
        self.pages.popup = None;
        self.happened(Event::OpenProfile(who), window, cx);
    }

    /// Whether there is a step to go back to: what the band's arrow says.
    pub(crate) fn can_go_back(&self) -> bool {
        self.has_a_menu()
            && (self.is_editing() || self.has_an_automation_open() || self.flow.can_go_back())
    }

    /// One step back: out of the editor (which asks first about what is
    /// not finished), out of an automation's page, or to the screen before.
    pub(crate) fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_a_menu() {
            return;
        }
        if self.is_editing() {
            self.leave_the_editor(window, cx);
        } else if self.has_an_automation_open() {
            self.close_the_automation(window, cx);
        } else if self.flow.can_go_back() {
            tracing::info!(action = "back", "action");
            self.menu = None;
            self.happened(Event::Back, window, cx);
        }
    }

    /// The page of the screen just shown, made when it is new, and every
    /// page the way back no longer holds let go: **a list let go stops**,
    /// and is kept in memory to be shown again at once
    /// (`state::people::Parked`).
    pub(super) fn settle_the_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let held: Vec<Screen> = self
            .flow
            .trail()
            .iter()
            .chain(std::iter::once(self.flow.screen()))
            .cloned()
            .collect();
        self.pages.profiles.retain(|pk, page| {
            // A page held but not shown reads no further: its cursor stays.
            if !matches!(&self.shown, Screen::Profile(who) if who.pk == *pk) {
                page.stop_reading_on();
            }
            held.iter()
                .any(|screen| matches!(screen, Screen::Profile(who) if who.pk == *pk))
        });
        let mut left = Vec::new();
        self.pages.lists.retain(|(pk, side), list| {
            let kept = held.iter().any(|screen| {
                matches!(screen, Screen::People { owner, side: shown } if owner.pk == *pk && shown == side)
            });
            if !kept {
                left.push(((*pk, *side), list.clone()));
            }
            kept
        });
        self.park_the_lists(left, cx);
        for list in self.pages.lists.values() {
            list.update(cx, |list, _| list.menu = None);
        }
        let consented = self.pages.consented.take();
        self.pages.popup = None;
        self.pages.viewer = None;
        self.pages.leaving = None;
        self.pages.about = None;
        match self.shown.clone() {
            Screen::Profile(who) if !self.pages.profiles.contains_key(&who.pk) => {
                self.read_the_profile(who, window, cx);
            }
            Screen::People { owner, side } if !self.pages.lists.contains_key(&(owner.pk, side)) => {
                // **Another account's list is read only right after its
                // yes**: shown again any other way, it shows what is kept.
                let may_read = consented == Some((owner.pk, side))
                    || self
                        .active()
                        .is_some_and(|viewer| !profile::asks_first(owner.pk, viewer.named.pk));
                self.pages.detached.retain(|list| list.upgrade().is_some());
                let going = self.pages.detached.iter().find_map(|list| {
                    let list = list.upgrade()?;
                    let same = {
                        let read = list.read(cx);
                        read.owner.pk == owner.pk && read.side == side
                    };
                    same.then_some(list)
                });
                let key = (owner.pk, side);
                let parked = going
                    .is_none()
                    .then(|| {
                        self.pages.parked.take(&key);
                        self.pages.parked_lists.remove(&key)
                    })
                    .flatten();
                match (going, parked) {
                    (Some(list), _) => {
                        self.pages.lists.insert(key, list);
                    }
                    (None, Some(list)) => {
                        self.pages.lists.insert(key, list.clone());
                        list.update(cx, |list, cx| list.unpark(cx));
                        if may_read {
                            self.read_the_rows(&list, window, cx);
                        }
                    }
                    (None, None) => self.open_the_list(owner, side, may_read, window, cx),
                }
            }
            _ => {}
        }
        self.tell_the_band_about_the_page(cx);
    }

    /// Whether the band says the profile shown is loading: while it, or the
    /// tab shown of it, is being read.
    pub(crate) fn tell_the_band_about_the_page(&mut self, cx: &mut Context<Self>) {
        let loading = self.profile_page().is_some_and(|page| {
            (page.failed.is_none() && page.profile.as_ref().is_none_or(|profile| !profile.read))
                || matches!(page.tabs.get(&page.tab), Some(TabRead::Reading))
        });
        match (loading, self.band.holds(&Status::Loading)) {
            (true, false) => self.say(Status::Loading, cx),
            (false, true) => self.quiet(|band| band.clear_kind(&Status::Loading), cx),
            _ => {}
        }
    }

    /// Shows what was kept of `who` at once, and reads it now.
    fn read_the_profile(&mut self, who: Named, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.pages.profiles.insert(
            who.pk,
            ProfilePage {
                who: who.clone(),
                profile: None,
                read_at: None,
                failed: None,
                tab: Tab::Posts,
                tab_from: (Tab::Posts, now),
                tabs: HashMap::new(),
                further: HashMap::new(),
                kept_tabs: HashMap::new(),
                shown_since: now,
                highlights_since: None,
                scroll: ScrollHandle::new(),
                highlights: ScrollHandle::new(),
                picture_button: cx.focus_handle(),
                options_button: cx.focus_handle(),
                picture_drawn: Rc::default(),
                options_drawn: Rc::default(),
                picture_clip: Rc::default(),
            },
        );
        let pk = who.pk;
        let kept = self.backend.profile_kept(who.clone());
        let kept_tabs: Vec<_> = Tab::ALL
            .into_iter()
            .map(|tab| (tab, self.backend.tiles_kept(who.clone(), tab)))
            .collect();
        let reading = self.backend.profile(who);
        self.engine_asked(cx);
        self.tell_the_band_about_the_page(cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Some(kept)) = kept.await {
                this.update(cx, |this, cx| {
                    if let Some(page) = this.pages.profiles.get_mut(&pk)
                        && page.profile.as_ref().is_none_or(|shown| !shown.read)
                    {
                        page.profile = Some(kept);
                        cx.notify();
                    }
                })
                .ok();
            }
            for (tab, kept) in kept_tabs {
                let Ok(Some(kept)) = kept.await else {
                    continue;
                };
                this.update(cx, |this, cx| {
                    if let Some(page) = this.pages.profiles.get_mut(&pk) {
                        // The posts kept come in, unless the read came first.
                        if page.tab == tab && !matches!(page.tabs.get(&tab), Some(TabRead::Read(_)))
                        {
                            page.shown_since = Instant::now();
                        }
                        page.kept_tabs.insert(tab, kept);
                        cx.notify();
                    }
                })
                .ok();
            }
            let read = reading.await;
            this.update_in(cx, |this, window, cx| {
                this.engine_answered(cx);
                this.profile_read(pk, read.ok(), window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn profile_read(
        &mut self,
        pk: Pk,
        read: Option<Result<Profile, Unread>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.pages.profiles.get_mut(&pk) else {
            return;
        };
        match read {
            Some(Ok(profile)) => {
                // An account's own page read now is what its menu shows too.
                if let Some(account) = self.accounts.iter_mut().find(|a| a.named.pk == pk) {
                    account.full_name = profile.full_name.clone().or(account.full_name.take());
                    account.biography = profile.biography.clone().or(account.biography.take());
                    if account.picture.is_none() {
                        account.picture = profile.picture.clone();
                    }
                }
                let visible = profile.visible == Some(true);
                let ids = |profile: &Profile| -> Vec<String> {
                    profile.highlights.iter().map(|h| h.id.clone()).collect()
                };
                let now = Instant::now();
                if page
                    .profile
                    .as_ref()
                    .is_none_or(|shown| ids(shown) != ids(&profile))
                {
                    page.highlights_since = Some(now);
                }
                page.profile = Some(profile);
                page.read_at = Some(now);
                page.failed = None;
                if visible {
                    let tab = page.tab;
                    self.read_the_tab(pk, tab, window, cx);
                }
            }
            Some(Err(why)) => {
                let said = super::super::screens::profile::unread_words(&why, Look::now(cx));
                page.failed = Some(why);
                self.announce(said, cx);
            }
            None => page.failed = Some(Unread::Failed(String::new())),
        }
        self.tell_the_band_about_the_page(cx);
        cx.notify();
    }

    /// Reads the first page of `tab` of the profile `pk`, once.
    fn read_the_tab(&mut self, pk: Pk, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.pages.profiles.get_mut(&pk) else {
            return;
        };
        if page.tabs.contains_key(&tab) {
            return;
        }
        let who = page
            .profile
            .as_ref()
            .map_or_else(|| page.who.clone(), |profile| profile.named.clone());
        page.tabs.insert(tab, TabRead::Reading);
        let reading = self.backend.tiles(who, tab);
        self.engine_asked(cx);
        self.tell_the_band_about_the_page(cx);
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            this.update(cx, |this, cx| {
                this.engine_answered(cx);
                if let Some(page) = this.pages.profiles.get_mut(&pk) {
                    let read = match read {
                        Ok(Ok(tiles)) => TabRead::Read(tiles),
                        Ok(Err(why)) => TabRead::Failed(why),
                        Err(_) => TabRead::Failed(Unread::Failed(String::new())),
                    };
                    // What was kept is replaced in its place: only posts
                    // that were not shown come in.
                    let kept = page
                        .kept_tabs
                        .get(&tab)
                        .is_some_and(|kept| !kept.tiles.is_empty());
                    if !kept {
                        page.shown_since = Instant::now();
                    }
                    page.tabs.insert(tab, read);
                    this.tell_the_band_about_the_page(cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The profile page shown, when one is.
    pub(crate) fn profile_page(&self) -> Option<&ProfilePage> {
        match &self.shown {
            Screen::Profile(who) => self.pages.profiles.get(&who.pk),
            _ => None,
        }
    }

    /// On a frame of the profile shown, from what the frame before
    /// measured: asks for the tab's next page when `state::profile::reads_on`
    /// says it is due, right after this frame. Sizes of something other
    /// than what is drawn now ask for one more frame instead.
    pub(crate) fn read_on_when_due(
        &mut self,
        sizes: (f32, f32, f32),
        measured: Measured,
        settled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let held_back = self.reads_held_back();
        let at_rest = self.is_resting();
        let Screen::Profile(who) = &self.shown else {
            return;
        };
        let Some(page) = self.pages.profiles.get_mut(&who.pk) else {
            return;
        };
        let (pk, tab) = (who.pk, page.tab);
        let visible = page
            .profile
            .as_ref()
            .is_some_and(|profile| profile.visible == Some(true));
        let Some(TabRead::Read(tiles)) = page.tabs.get(&tab) else {
            return;
        };
        let next = tiles.next;
        let further = page.further.entry(tab).or_default();
        let fresh = further.measured == Some(measured);
        if !fresh {
            further.measured = Some(measured);
            crate::ui::frames::ask(window);
        }
        let (scrolled, content, seen) = sizes;
        let due = profile::reads_on(&profile::ReadOn {
            scrolled,
            content,
            seen,
            measured: fresh && settled,
            pending: further.reading.is_some(),
            next: next.is_some(),
            resting_until: further.resting_until,
            now: snob_core::clock::now(),
            failed: further.failed.is_some(),
            looked_at: window.is_window_active(),
            at_rest,
            held_back,
            visible,
        });
        if let (true, Some(after)) = (due, next) {
            // Marked as on its way at once, so no other frame asks too.
            let reading = Reading {
                id: READS.fetch_add(1, Ordering::Relaxed),
                stop: CancelToken::default(),
            };
            further.reading = Some(reading.clone());
            cx.defer_in(window, move |this, window, cx| {
                this.read_more(pk, tab, after, reading, window, cx);
            });
        }
    }

    /// The band says Instagram asked for a pause, or that there is no
    /// network: no page past a tab's first is asked for.
    pub(crate) fn reads_held_back(&self) -> bool {
        self.band.holds(&Status::Offline)
            || self.band.holds(&Status::Paused {
                until: Epoch::new(0),
            })
    }

    /// Reads the page of `tab` of the profile `pk` that `after` points at,
    /// until `stop`. **The engine sets the pace**: the page says that it
    /// reads, and until when it rests when the engine says so.
    fn read_more(
        &mut self,
        pk: Pk,
        tab: Tab,
        after: Cursor,
        asked: Reading,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (id, listening) = (asked.id, asked.stop.clone());
        if listening.is_canceled() {
            // Stopped before it was asked: nothing is on its way.
            if let Some(further) = self.further_of(pk, tab)
                && further.is_reading(id)
            {
                further.reading = None;
            }
            return;
        }
        let Some(page) = self.pages.profiles.get(&pk) else {
            return;
        };
        let who = page
            .profile
            .as_ref()
            .map_or_else(|| page.who.clone(), |profile| profile.named.clone());
        tracing::debug!(?tab, "a profile's tab reads on");
        let (resting, mut heard) = watch::channel(None);
        let reading = self
            .backend
            .more_tiles(who, tab, after, resting, asked.stop.clone());
        self.engine_asked(cx);
        cx.spawn_in(window, async move |this, cx| {
            while heard.changed().await.is_ok() {
                let until = *heard.borrow_and_update();
                let told = this.update(cx, |this, cx| {
                    if listening.is_canceled() {
                        return;
                    }
                    let Some(further) = this.further_of(pk, tab).filter(|f| f.is_reading(id))
                    else {
                        return;
                    };
                    further.resting_until = until;
                    cx.notify();
                    // The words change when the rest ends, the app resting
                    // or not.
                    let left = until.map_or(0, |until| {
                        until.get().saturating_sub(snob_core::clock::now().get())
                    });
                    if let Ok(left @ 1..) = u64::try_from(left) {
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(Duration::from_secs(left))
                                .await;
                            this.update(cx, |_, cx| cx.notify()).ok();
                        })
                        .detach();
                    }
                });
                if told.is_err() || listening.is_canceled() {
                    return;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            this.update_in(cx, |this, window, cx| {
                this.engine_answered(cx);
                this.more_read(pk, tab, &asked, read.ok(), window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn further_of(&mut self, pk: Pk, tab: Tab) -> Option<&mut Further> {
        self.pages.profiles.get_mut(&pk)?.further.get_mut(&tab)
    }

    /// A page past a tab's first came, or did not: its posts go after the
    /// others, or the page says it was not read and waits for a button.
    /// **A page that came is shown though it was stopped**, as the engine
    /// spent its cursor for it; one let go of (`Further::let_go`, a page
    /// made again) is not. A page stopped that did not come says nothing:
    /// its cursor was put back, and it is asked again when due.
    fn more_read(
        &mut self,
        pk: Pk,
        tab: Tab,
        asked: &Reading,
        read: Option<Result<Tiles, Unread>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.pages.profiles.get_mut(&pk) else {
            return;
        };
        let Some(further) = page
            .further
            .get_mut(&tab)
            .filter(|f| f.is_reading(asked.id))
        else {
            return;
        };
        let stopped = asked.stop.is_canceled();
        further.reading = None;
        further.resting_until = None;
        match read {
            Some(Ok(more)) => {
                if let Some(TabRead::Read(tiles)) = page.tabs.get_mut(&tab) {
                    further.came.push((tiles.tiles.len(), Instant::now()));
                    tiles.tiles.extend(more.tiles);
                    tiles.more = more.more;
                    tiles.next = more.next;
                }
            }
            Some(Err(_)) | None if stopped => {}
            Some(Err(why)) => {
                let paused = matches!(why, Unread::Paused(_));
                let look = Look::now(cx).clone();
                let said = match &why {
                    Unread::Failed(_) => look.tr("profile-more-failed"),
                    other => super::super::screens::profile::unread_words(other, &look),
                };
                further.failed = Some(why);
                self.announce(said, cx);
                // The band says the pause the engine keeps, nothing sent.
                if paused {
                    self.read_the_pause(window, cx);
                }
            }
            None => further.failed = Some(Unread::Failed(String::new())),
        }
        cx.notify();
    }

    /// The button under a page that could not be read: the same page is
    /// asked for again, from where it was, once it is due.
    pub(crate) fn read_more_again(&mut self, cx: &mut Context<Self>) {
        let Screen::Profile(who) = &self.shown else {
            return;
        };
        let (pk, Some(tab)) = (who.pk, self.profile_page().map(|page| page.tab)) else {
            return;
        };
        tracing::info!(action = "read_more_posts_again", "action");
        // In a pause or with no network the failure stays, and its button
        // waits.
        if self.reads_held_back() {
            return;
        }
        if let Some(further) = self.further_of(pk, tab) {
            further.failed = None;
        }
        cx.notify();
    }

    /// Stops every profile's pages on their way and lets go of where they
    /// read on: **a cursor is the account's that it was handed to**.
    pub(crate) fn forget_reading_on(&mut self) {
        for page in self.pages.profiles.values_mut() {
            for further in page.further.values_mut() {
                further.let_go();
            }
            for read in page.tabs.values_mut() {
                if let TabRead::Read(tiles) = read {
                    tiles.next = None;
                }
            }
        }
    }

    /// To tab `tab` of the profile shown.
    pub(crate) fn choose_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Profile(who) = self.shown.clone() else {
            return;
        };
        let Some(page) = self.pages.profiles.get_mut(&who.pk) else {
            return;
        };
        if page.tab == tab {
            return;
        }
        tracing::info!(action = "choose_profile_tab", "action");
        if let Some(left) = page.further.get_mut(&page.tab) {
            left.stop();
        }
        page.tab_from = (page.tab, Instant::now());
        page.tab = tab;
        page.shown_since = Instant::now();
        let visible = page
            .profile
            .as_ref()
            .is_some_and(|profile| profile.visible == Some(true));
        if visible {
            self.read_the_tab(who.pk, tab, window, cx);
        }
        self.tell_the_band_about_the_page(cx);
        cx.notify();
    }

    /// Shows the posts as a grid or as a list.
    pub(crate) fn show_posts_as(&mut self, showing: Showing, cx: &mut Context<Self>) {
        if self.pages.showing == showing {
            return;
        }
        tracing::info!(action = "show_posts_as", ?showing, "action");
        self.pages.showing_from = Some((self.pages.showing, Instant::now()));
        self.pages.showing = showing;
        cx.notify();
    }

    /// The other layout.
    pub(crate) fn change_the_layout(&mut self, cx: &mut Context<Self>) {
        self.pages.layout = self.pages.layout.other();
        tracing::info!(action = "change_profile_layout", layout = ?self.pages.layout, "action");
        self.pages.laid_out_at = Some(Instant::now());
        cx.notify();
    }

    /// The profile shown scrolls its posts on its own: in the split layout.
    pub(super) fn a_page_scrolls_itself(&self) -> bool {
        matches!(self.shown, Screen::People { .. })
            || (matches!(self.shown, Screen::Profile(_)) && self.pages.layout == Layout::Split)
    }

    /// A click on the picture of the profile shown: the picture large, or,
    /// with a story up, the choice of which.
    pub(crate) fn click_the_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.profile_page().and_then(|page| page.profile.clone()) else {
            return;
        };
        match profile::picture_click(profile.story_up) {
            PictureClick::Open => self.open_the_picture(window, cx),
            PictureClick::Choose => self.open_a_popup(PopupKind::Picture, window, cx),
        }
    }

    /// Opens a small menu over the profile, or closes the one open.
    pub(crate) fn open_a_popup(
        &mut self,
        kind: PopupKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .pages
            .popup
            .as_ref()
            .is_some_and(|popup| popup.kind == kind)
        {
            self.close_the_popup(window, cx);
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.pages.popup = Some(Popup {
            kind,
            focus,
            since: Instant::now(),
        });
        cx.notify();
    }

    /// Closes the small menu, focus back on what opened it.
    pub(crate) fn close_the_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = self.pages.popup.take() else {
            return;
        };
        if let Some(page) = self.profile_page() {
            let back = match popup.kind {
                PopupKind::Picture => page.picture_button.clone(),
                PopupKind::Options => page.options_button.clone(),
            };
            window.focus(&back, cx);
        }
        cx.notify();
    }

    /// The story's way in: **not opened yet, and nothing of it is read or
    /// marked as seen**. Says so.
    pub(crate) fn open_the_story(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!(action = "open_story", "action");
        self.close_the_popup(window, cx);
        let said = Look::now(cx).tr("profile-story-later");
        self.announce(said, cx);
    }

    /// Copies something the options offer: the username, the address of the
    /// profile, the account's number.
    pub(crate) fn copy_of_the_profile(
        &mut self,
        what: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.profile_page().and_then(|page| page.profile.clone()) else {
            return;
        };
        let username = profile.named.username.clone().unwrap_or_default();
        let (text, said) = match what {
            "username" => (username, "profile-copied-username"),
            "link" => {
                let Some(address) = crate::system::profile_address(&username) else {
                    self.close_the_popup(window, cx);
                    return;
                };
                (address, "profile-copied-link")
            }
            _ => (profile.named.pk.to_string(), "profile-copied-id"),
        };
        tracing::info!(action = "copy_from_profile", what, "action");
        self.system.copy(&text, cx);
        self.close_the_popup(window, cx);
        let said = Look::now(cx).tr(said);
        self.announce(said, cx);
    }

    /// Hands the person's own browser the profile's address on Instagram,
    /// as any link would: **nothing else, and nothing is read from it**. A
    /// name that cannot be a username opens nothing.
    pub(crate) fn open_in_the_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.profile_page().and_then(|page| page.profile.clone()) else {
            return;
        };
        self.close_the_popup(window, cx);
        let Some(address) = profile
            .named
            .username
            .as_deref()
            .and_then(crate::system::profile_address)
        else {
            tracing::warn!("a name that cannot be a username was not opened in the browser");
            return;
        };
        tracing::info!(action = "open_profile_in_browser", "action");
        self.system.open_url(&address, cx);
        let said = Look::now(cx).tr("profile-opened-browser");
        self.announce(said, cx);
    }

    /// "About this account" over the profile shown, focus on its button
    /// that closes it. **Nothing is read for it.**
    pub(crate) fn open_about_the_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.profile_page().and_then(|page| page.profile.clone()) else {
            return;
        };
        tracing::info!(action = "open_about_account", "action");
        self.pages.popup = None;
        let (focus, close) = (cx.focus_handle(), cx.focus_handle());
        window.focus(&close, cx);
        self.pages.about = Some(AboutAccount {
            who: profile.named,
            full_name: profile.full_name,
            picture: profile.picture,
            known: profile::About::default(),
            focus,
            close,
        });
        cx.notify();
    }

    /// Closes "About this account", focus back on the options.
    pub(crate) fn close_about_the_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.about.take().is_some() {
            if let Some(page) = self.profile_page() {
                window.focus(&page.options_button, cx);
            }
            cx.notify();
        }
    }

    /// The picture of the profile shown, large over the screen: it grows
    /// from where the page draws it, or, on its way back there, turns
    /// around from where it is.
    pub(crate) fn open_the_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.profile_page() else {
            return;
        };
        let Some(profile) = page.profile.clone() else {
            return;
        };
        tracing::info!(action = "open_profile_picture", "action");
        let back_to = Some(page.picture_button.clone());
        let on_the_page = page.picture_drawn.clone();
        let clip = page.picture_clip.clone();
        self.pages.popup = None;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let who = profile.named.clone();
        let look = Look::now(cx).clone();
        let mut way = way_now(&on_the_page, &clip, window, &look);
        let (mut run, mut zoom, mut large, mut large_since) = (Run::OPENING, Zoom::FIT, None, None);
        if let Some(leaving) = self
            .pages
            .leaving
            .take()
            .filter(|leaving| leaving.who.pk == who.pk && !leaving.done)
        {
            let at = leaving.run.at(
                leaving.since.elapsed(),
                whole_way(false, &look),
                motion::STANDARD,
            );
            run = leaving.run.turned(at);
            if leaving.way == Way::Fades {
                way = Way::Fades;
            }
            zoom = leaving.zoom;
            large = leaving.large;
            if leaving.large_shown >= 1.0 {
                let now = Instant::now();
                large_since = Some(now.checked_sub(motion::FADE).unwrap_or(now));
            }
        }
        self.pages.viewer = Some(PictureViewer {
            who: who.clone(),
            small: profile.picture.clone(),
            large,
            large_since,
            zoom,
            way,
            run,
            since: Instant::now(),
            on_the_page,
            clip,
            focus,
            dragging: None,
            menu: None,
            menu_focus: cx.focus_handle(),
            said: None,
            back_to,
        });
        let fetching = self.backend.full_picture(who.clone());
        self.engine_asked(cx);
        cx.spawn_in(window, async move |this, cx| {
            let large = fetching.await;
            this.update(cx, |this, cx| {
                this.engine_answered(cx);
                let Ok(large) = large else {
                    return;
                };
                if let Some(viewer) = &mut this.pages.viewer
                    && viewer.who.pk == who.pk
                    && large.is_some()
                    && viewer.large != large
                {
                    // A large picture already all in stays under the new
                    // one until it is decoded, never the small one again.
                    let all_in = viewer
                        .large_since
                        .is_some_and(|since| since.elapsed() >= motion::FADE);
                    let before = std::mem::replace(&mut viewer.large, large);
                    if all_in {
                        viewer.small = before;
                    }
                    viewer.large_since = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Closes the picture, focus back where it was: **the viewer is gone at
    /// once**, and the picture goes back to where the page draws it now,
    /// from where it is, or fades where it is when that place is out of
    /// sight.
    pub(crate) fn close_the_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(viewer) = self.pages.viewer.take() else {
            return;
        };
        if let Some(back) = &viewer.back_to {
            window.focus(back, cx);
        }
        let look = Look::now(cx).clone();
        let at = viewer.run.at(
            viewer.since.elapsed(),
            whole_way(true, &look),
            motion::STANDARD,
        );
        let fit = viewer_fit(window);
        let drawn = viewer.drawn(at, fit);
        let way = match viewer.way {
            Way::Moves => way_now(&viewer.on_the_page, &viewer.clip, window, &look),
            Way::Fades => Way::Fades,
        };
        let (end, end_corner) = match way {
            Way::Moves => (viewer.end(fit), viewer.end_corner()),
            Way::Fades => (drawn.rect, drawn.corner),
        };
        let large_shown = viewer.large_since.map_or(0.0, |since| {
            motion::STANDARD.at(motion::progress(since.elapsed(), motion::FADE))
        });
        self.pages.leaving = Some(LeavingPicture {
            who: viewer.who,
            small: viewer.small,
            large: viewer.large,
            large_shown,
            zoom: viewer.zoom,
            way,
            run: viewer.run.turned(at),
            since: Instant::now(),
            end,
            end_corner,
            shown: drawn.shown,
            on_the_page: viewer.on_the_page,
            done: false,
        });
        cx.notify();
    }

    /// Whether the page leaves out its own picture of `pk`: while it is on
    /// its way to the viewer or back, **so it is never seen twice**.
    pub fn picture_is_away(&self, pk: Pk) -> bool {
        let viewer = self
            .pages
            .viewer
            .as_ref()
            .is_some_and(|viewer| viewer.who.pk == pk && viewer.way == Way::Moves);
        let leaving = self.pages.leaving.as_ref().is_some_and(|leaving| {
            leaving.who.pk == pk && leaving.way == Way::Moves && !leaving.done
        });
        viewer || leaving
    }

    /// The wheel over the picture: zoomed about the pointer.
    pub(crate) fn zoom_the_picture(
        &mut self,
        notches: f32,
        at: (f32, f32),
        size: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        if let Some(viewer) = &mut self.pages.viewer {
            viewer.zoom = viewer.zoom.wheeled(notches, at, size);
            cx.notify();
        }
    }

    /// A drag over the picture, zoomed in: it follows.
    pub(crate) fn drag_the_picture(
        &mut self,
        to: Point<Pixels>,
        size: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        if let Some(viewer) = &mut self.pages.viewer
            && let Some(from) = viewer.dragging
        {
            let by = (f32::from(to.x - from.x), f32::from(to.y - from.y));
            viewer.zoom = viewer.zoom.dragged(by, size);
            viewer.dragging = Some(to);
            cx.notify();
        }
    }

    /// A drag over the picture starts at `at`, or, with none, ends.
    pub(crate) fn hold_the_picture(&mut self, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        if let Some(viewer) = &mut self.pages.viewer {
            viewer.dragging = at.filter(|_| viewer.zoom.zoomed());
            cx.notify();
        }
    }

    /// The picture back as it opened.
    pub(crate) fn fit_the_picture(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = &mut self.pages.viewer {
            viewer.zoom = Zoom::FIT;
            cx.notify();
        }
    }

    /// Opens the menu a right click on the picture opens, at `at`.
    pub(crate) fn picture_menu(
        &mut self,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(viewer) = &mut self.pages.viewer {
            viewer.menu = at;
            let focus = if at.is_some() {
                viewer.menu_focus.clone()
            } else {
                viewer.focus.clone()
            };
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    /// Saves the picture shown in the system's downloads, or with `ask` in
    /// a folder the person chooses, and says where.
    pub(crate) fn save_the_picture(
        &mut self,
        ask: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(viewer) = &mut self.pages.viewer else {
            return;
        };
        viewer.menu = None;
        let who = viewer.who.clone();
        window.focus(&viewer.focus.clone(), cx);
        tracing::info!(action = "save_profile_picture", ask, "action");
        let folder = if ask {
            self.system.choose_folder(cx)
        } else {
            crate::backend::answered(self.system.downloads())
        };
        let backend = self.backend.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Some(folder)) = folder.await else {
                return;
            };
            let Ok(saved) = backend.save_picture(who, folder).await else {
                return;
            };
            this.update(cx, |this, cx| {
                let look = Look::now(cx).clone();
                let said = match &saved {
                    Some(path) => look.tr_with(
                        "viewer-saved",
                        &[(
                            "file",
                            &path.file_name().unwrap_or_default().to_string_lossy(),
                        )],
                    ),
                    None => look.tr("viewer-not-saved"),
                };
                if let Some(viewer) = &mut this.pages.viewer {
                    viewer.said = Some(said.clone().into());
                }
                this.announce(said, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Puts the picture shown on the clipboard, and says so.
    pub(crate) fn copy_the_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(viewer) = &mut self.pages.viewer else {
            return;
        };
        viewer.menu = None;
        window.focus(&viewer.focus.clone(), cx);
        tracing::info!(action = "copy_profile_picture", "action");
        let reading = self.backend.picture_bytes(viewer.who.clone());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(bytes) = reading.await else {
                return;
            };
            this.update(cx, |this, cx| {
                let look = Look::now(cx).clone();
                let said = match bytes {
                    Some(bytes) => {
                        this.system.copy_image(bytes, cx);
                        look.tr("viewer-copied")
                    }
                    None => look.tr("viewer-not-copied"),
                };
                if let Some(viewer) = &mut this.pages.viewer {
                    viewer.said = Some(said.clone().into());
                }
                this.announce(said, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// One of the profile's lists: at once for the account's own, and
    /// **another's only after the question is answered yes**.
    pub(crate) fn open_a_list(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.profile_page().and_then(|page| page.profile.clone()) else {
            return;
        };
        let Some(viewer) = self.active().map(|account| account.named.pk) else {
            return;
        };
        if profile.visible == Some(false) {
            return;
        }
        if profile::asks_first(profile.named.pk, viewer) {
            let from = window.focused(cx);
            let (focus, cancel) = (cx.focus_handle(), cx.focus_handle());
            window.focus(&cancel, cx);
            self.pages.asking = Some(ListQuestion {
                owner: profile.named,
                side,
                focus,
                cancel,
                from,
            });
            cx.notify();
        } else {
            self.happened(
                Event::OpenList {
                    owner: profile.named,
                    side,
                },
                window,
                cx,
            );
        }
    }

    /// The answer was no: nothing is read.
    pub(crate) fn keep_the_list_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pages.consented = None;
        if let Some(asked) = self.pages.asking.take() {
            tracing::info!(action = "decline_list", "action");
            if let Some(from) = asked.from {
                window.focus(&from, cx);
            }
            cx.notify();
        }
    }

    /// The answer was yes: the list opens and is read.
    pub(crate) fn read_the_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(asked) = self.pages.asking.take() {
            tracing::info!(action = "consent_to_list", "action");
            self.pages.consented = Some((asked.owner.pk, asked.side));
            self.happened(
                Event::OpenList {
                    owner: asked.owner,
                    side: asked.side,
                },
                window,
                cx,
            );
        }
    }

    /// Opens a list: what was stored of it shown at once (**nothing sent**
    /// for it), then read again behind it when `may_read`, its rows coming
    /// in as they are read when nothing was stored.
    fn open_the_list(
        &mut self,
        owner: Named,
        side: Side,
        may_read: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(viewer) = self.active().map(|account| account.named.pk) else {
            return;
        };
        let (backend, shell) = (self.backend.clone(), cx.entity().downgrade());
        let list = cx.new(|cx| PeopleList::new(owner.clone(), side, viewer, backend, shell, cx));
        self.pages.lists.insert((owner.pk, side), list.clone());
        let kept = self.backend.list_kept(owner, side);
        cx.spawn_in(window, async move |this, cx| {
            let kept = kept.await.ok().flatten();
            this.update_in(cx, |this, window, cx| {
                // Left before it was answered: kept, the list shows it when
                // it is shown again.
                if let Some(kept) = kept {
                    list.update(cx, |list, cx| list.show_kept(kept, cx));
                }
                let shown = this
                    .pages
                    .lists
                    .values()
                    .any(|held| held.entity_id() == list.entity_id());
                // Shown again meanwhile, it may already be read.
                let unread = list.read(cx).read_id == 0;
                if may_read && shown && unread {
                    this.read_the_rows(&list, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Reads `list` now, behind what it shows: **another account's only
    /// right after its yes** (`settle_the_pages`).
    fn read_the_rows(
        &mut self,
        list: &Entity<PeopleList>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stop = CancelToken::default();
        let (said, mut heard) = watch::channel(Walked::default());
        let (id, owner, side) = list.update(cx, |list, cx| {
            let id = list.start_reading(stop.clone());
            cx.notify();
            (id, list.owner.clone(), list.side)
        });
        let walking = self.backend.list(owner, side, said, stop);
        self.engine_asked(cx);
        let weak = list.downgrade();
        cx.spawn_in(window, async move |this, cx| {
            let mut first = true;
            while heard.changed().await.is_ok() {
                if std::mem::take(&mut first) {
                    this.update(cx, |this, cx| this.engine_answered(cx)).ok();
                }
                let walked = heard.borrow_and_update().clone();
                if weak
                    .update(cx, |list, cx| list.heard(id, walked, cx))
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
        let weak = list.downgrade();
        cx.spawn_in(window, async move |this, cx| {
            let ended = walking
                .await
                .unwrap_or_else(|_| Err(Unread::Failed(String::new())));
            let said = weak
                .update(cx, |list, cx| {
                    list.read_ended(id, ended, cx).then(|| {
                        let look = Look::now(cx).clone();
                        super::super::screens::people::progress_words(list, &look)
                    })
                })
                .ok()
                .flatten();
            this.update(cx, |this, cx| {
                this.engine_answered(cx);
                if let Some(said) = said {
                    this.announce(said, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Keeps the lists `left` in memory, read no further, to be shown again
    /// at once: the newest [`people::PARKED_AT_MOST`] of the account in
    /// use. A list in a window of its own reads on there.
    fn park_the_lists(&mut self, left: Vec<((Pk, Side), Entity<PeopleList>)>, cx: &mut App) {
        let viewer = self.active().map(|account| account.named.pk);
        for (key, list) in left {
            let (alone, theirs) = {
                let read = list.read(cx);
                (read.window.is_some(), Some(read.viewer) == viewer)
            };
            if alone || !theirs {
                continue;
            }
            list.update(cx, |list, _| list.park());
            for gone in self.pages.parked.park(key) {
                self.pages.parked_lists.remove(&gone);
            }
            self.pages.parked_lists.insert(key, list);
        }
        let lists = &self.pages.parked_lists;
        let others = self.pages.parked.let_go(|key| {
            lists
                .get(key)
                .is_some_and(|list| Some(list.read(cx).viewer) == viewer)
        });
        for gone in others {
            self.pages.parked_lists.remove(&gone);
        }
    }

    /// The pictures of every list shown looked for again: what the app
    /// waking does, since a list asks for none while it rests.
    pub(super) fn look_for_the_lists_pictures(&mut self, cx: &mut Context<Self>) {
        self.pages.detached.retain(|list| list.upgrade().is_some());
        let shown = self
            .people_list()
            .into_iter()
            .chain(self.pages.detached.iter().filter_map(WeakEntity::upgrade));
        for list in shown.collect::<Vec<_>>() {
            list.update(cx, |list, cx| list.look_for_pictures(cx));
        }
    }

    /// Puts the usernames of the rows chosen in `list` on the clipboard,
    /// one a line and without their at sign, and says how many: **nothing
    /// is sent**, and nothing else is done with them.
    pub(crate) fn copy_the_usernames(&mut self, list: &Entity<PeopleList>, cx: &mut Context<Self>) {
        let (text, count) = list.read(cx).chosen_usernames();
        if count == 0 {
            return;
        }
        tracing::info!(action = "copy_usernames", count, "action");
        self.system.copy(&text, cx);
        list.update(cx, |list, cx| {
            list.copied = Some(count);
            cx.notify();
        });
        let look = Look::now(cx);
        let count = count as u64;
        let grouped = crate::ui::screens::profile::grouped(count, look.texts.language());
        let said = look.tr_count("people-copied", count, &[("count", &grouped)]);
        self.announce(said, cx);
    }

    /// The list shown, when one is.
    pub(crate) fn people_list(&self) -> Option<Entity<PeopleList>> {
        match &self.shown {
            Screen::People { owner, side } => self.pages.lists.get(&(owner.pk, *side)).cloned(),
            _ => None,
        }
    }

    /// Moves the list shown out to a window of its own, and goes back.
    pub(crate) fn detach_the_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(list) = self.people_list() else {
            return;
        };
        tracing::info!(action = "detach_list", "action");
        list.update(cx, |list, _| list.menu = None);
        let main = window.window_handle();
        let shell = cx.entity().downgrade();
        let opened = crate::ui::detached::open(list.clone(), shell, main, cx);
        match opened {
            Ok(handle) => {
                list.update(cx, |list, _| list.window = Some(handle));
                self.pages.detached.push(list.downgrade());
                self.go_back(window, cx);
            }
            Err(e) => tracing::warn!(error = %e, "the list could not have a window of its own"),
        }
    }

    /// Closes whatever of the pages is open over the screen, the newest
    /// first: whether anything was.
    pub(super) fn dismiss_a_page_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .pages
            .viewer
            .as_ref()
            .is_some_and(|viewer| viewer.menu.is_some())
        {
            self.picture_menu(None, window, cx);
        } else if self.pages.viewer.is_some() {
            self.close_the_picture(window, cx);
        } else if self.pages.about.is_some() {
            self.close_about_the_account(window, cx);
        } else if self.pages.asking.is_some() {
            self.keep_the_list_closed(window, cx);
        } else if self.pages.popup.is_some() {
            self.close_the_popup(window, cx);
        } else if let Some(list) = self.people_list().filter(|_| self.menu.is_none()) {
            return list.update(cx, |list, cx| list.dismiss(window, cx));
        } else {
            return false;
        }
        true
    }

    /// Whether something of the pages is open over the screen: a list's
    /// options included.
    pub(super) fn a_page_overlay_is_open(&self, cx: &App) -> bool {
        self.pages.viewer.is_some()
            || self.pages.asking.is_some()
            || self.pages.popup.is_some()
            || self.pages.about.is_some()
            || self
                .people_list()
                .is_some_and(|list| list.read(cx).menu.is_some())
    }
}

/// The picture over the page at one instant: where it is drawn, how round,
/// how much of it is shown, and how much of the backdrop is under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PictureDrawn {
    pub rect: Rect,
    pub corner: f32,
    pub shown: f32,
    pub backdrop: f32,
}

impl PictureViewer {
    /// Where it is along its way now, from the page (0) to its place (1),
    /// and whether it has arrived.
    pub(crate) fn along(&self, look: &Look) -> (f32, bool) {
        let (elapsed, whole) = (self.since.elapsed(), whole_way(true, look));
        (
            self.run.at(elapsed, whole, motion::STANDARD),
            self.run.arrived(elapsed, whole),
        )
    }

    /// When it arrives, or arrived.
    pub(crate) fn arrives(&self, look: &Look) -> Instant {
        self.since + self.run.lasts(whole_way(true, look))
    }

    /// Its end of the way, the picture fitting in `fit`: zoomed and moved
    /// as the person did once it arrived.
    pub(crate) fn end(&self, fit: Rect) -> Rect {
        let side = fit.w * self.zoom.scale;
        Rect {
            x: fit.x + (fit.w - side) / 2.0 + self.zoom.x,
            y: fit.y + (fit.h - side) / 2.0 + self.zoom.y,
            w: side,
            h: side,
        }
    }

    /// Its corners there: none once zoomed in.
    pub(crate) fn end_corner(&self) -> f32 {
        if self.zoom.zoomed() {
            0.0
        } else {
            profile::VIEWER_CORNER
        }
    }

    /// The picture `at` of its way, fitting in `fit` at its end.
    pub(crate) fn drawn(&self, at: f32, fit: Rect) -> PictureDrawn {
        let (end, end_corner) = (self.end(fit), self.end_corner());
        let (rect, corner, shown) = match self.way {
            Way::Moves => {
                let page = place_on_the_page(&self.on_the_page);
                let (rect, corner) = profile::moved_picture(page, end, end_corner, at);
                (rect, corner, 1.0)
            }
            Way::Fades => (end, end_corner, at),
        };
        PictureDrawn {
            rect,
            corner,
            shown,
            backdrop: at,
        }
    }
}

impl LeavingPicture {
    /// Where it is along its way back now, and whether it is there.
    pub(crate) fn along(&self, look: &Look) -> (f32, bool) {
        let (elapsed, whole) = (self.since.elapsed(), whole_way(false, look));
        (
            self.run.at(elapsed, whole, motion::STANDARD),
            self.run.arrived(elapsed, whole),
        )
    }

    /// The picture `at` of its way back: toward where the page draws it
    /// now, or fading where it was.
    pub(crate) fn drawn(&self, at: f32) -> PictureDrawn {
        let (rect, corner, shown) = match self.way {
            Way::Moves => {
                let page = place_on_the_page(&self.on_the_page);
                let (rect, corner) = profile::moved_picture(page, self.end, self.end_corner, at);
                (rect, corner, 1.0)
            }
            Way::Fades => (self.end, self.end_corner, self.shown * self.run.left(at)),
        };
        PictureDrawn {
            rect,
            corner,
            shown,
            backdrop: at,
        }
    }
}

/// How long the picture's whole way takes, into its viewer or out of it:
/// a fade's time under "reduce motion".
pub(crate) fn whole_way(opening: bool, look: &Look) -> std::time::Duration {
    let lasts = if opening { motion::ENTER } else { motion::EXIT };
    look.moved(lasts, 0.0, 1.0).lasts
}

/// Where the picture fits in its viewer in `window`.
pub(crate) fn viewer_fit(window: &Window) -> Rect {
    let size = window.viewport_size();
    profile::viewer_place(f32::from(size.width), f32::from(size.height), frame::BAND)
}

/// How the picture goes between the page and its viewer now, the page
/// drawing it at `on_the_page` inside `clip`: what of `window` shows the
/// page is everything under the band.
fn way_now(
    on_the_page: &Rc<Cell<Bounds<Pixels>>>,
    clip: &Rc<Cell<Bounds<Pixels>>>,
    window: &Window,
    look: &Look,
) -> Way {
    let size = window.viewport_size();
    let seen = Rect {
        x: 0.0,
        y: frame::BAND,
        w: f32::from(size.width),
        h: f32::from(size.height) - frame::BAND,
    };
    profile::way(
        place_on_the_page(on_the_page),
        seen,
        rect_of(clip.get()),
        look.settings.reduce_motion,
    )
}

/// The picture itself where the page last drew its button, `drawn`: inside
/// the ring and its gap.
fn place_on_the_page(drawn: &Rc<Cell<Bounds<Pixels>>>) -> Rect {
    rect_of(drawn.get()).inset(super::super::screens::profile::AROUND_THE_PICTURE)
}

fn rect_of(bounds: Bounds<Pixels>) -> Rect {
    Rect {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        w: f32::from(bounds.size.width),
        h: f32::from(bounds.size.height),
    }
}

/// How far `since` is through `lasts`, on the standard curve, asking for
/// another frame while it moves.
pub(crate) fn eased(since: Instant, lasts: std::time::Duration, window: &mut Window) -> f32 {
    moving(motion::STANDARD, since, lasts, window)
}

/// How far `since` is through `lasts`, on `curve`, asking for another frame
/// while it moves and none once it has arrived.
pub fn moving(
    curve: motion::Curve,
    since: Instant,
    lasts: std::time::Duration,
    window: &mut Window,
) -> f32 {
    let at = motion::progress(since.elapsed(), lasts);
    if at < 1.0 {
        crate::ui::frames::ask(window);
    }
    curve.at(at)
}
