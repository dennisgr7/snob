//! The automations section while it is shown: what its stages hold between
//! two frames (the list's search, an automation's page, the editor's canvas
//! and what is open over it, where the grid behind the introduction has
//! drifted to), and what the person does there. What is shown is decided in
//! `state::automations`, the editor's copy and its undo in `state::editing`;
//! this keeps them and acts on them, and `screens::automations` draws it.
//!
//! **The editor saves instantly**: every change asks the backend to keep
//! the copy (`Shell::keep_the_copy`), **one request at a time**
//! (`state::editing::Saves`, held by the shell so that what is asked
//! while the section is left still goes out in order). **Nothing here
//! runs an automation but "Run now"**, which starts `snob watch once` as
//! the engine does.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{Bounds, Context, Entity, FocusHandle, Pixels, Point, Subscription, Window};
use snob_core::Pk;
use snob_core::automation::{Account, Only, Read, Stage as Kind, Step};

use super::assets;
use super::look::Look;
use super::shell::Shell;
use crate::backend::{Automation, Next, Run};
use crate::state::automations::{Canvas, Drift, Glide, Stage};
use crate::state::editing::Editing;
use crate::state::shelf::Shelf;
use crate::state::steps::{ScheduleForm, WebhookForm};

use super::automations_editor::{Dialog, Leave, NodeMenu, Wire};

/// The editor's four kinds of steps, in the order its panel lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Steps {
    Triggers,
    Actions,
    Filters,
    Outputs,
}

impl Steps {
    pub const ALL: [Self; 4] = [Self::Triggers, Self::Actions, Self::Filters, Self::Outputs];

    pub fn at(self) -> usize {
        match self {
            Self::Triggers => 0,
            Self::Actions => 1,
            Self::Filters => 2,
            Self::Outputs => 3,
        }
    }

    /// The kind of step these are.
    pub fn kind(self) -> Kind {
        match self {
            Self::Triggers => Kind::Trigger,
            Self::Actions => Kind::Action,
            Self::Filters => Kind::Filter,
            Self::Outputs => Kind::Output,
        }
    }

    pub fn of(kind: Kind) -> Self {
        match kind {
            Kind::Trigger => Self::Triggers,
            Kind::Action => Self::Actions,
            Kind::Filter => Self::Filters,
            Kind::Output => Self::Outputs,
        }
    }

    /// What the panel offers of this kind.
    pub fn items(self) -> Vec<Item> {
        match self {
            Self::Triggers => vec![Item::Schedule, Item::Manual, Item::Soon("trigger-webhook")],
            Self::Actions => Read::all().into_iter().map(Item::Read).collect(),
            Self::Filters => Only::ALL.into_iter().map(Item::Filter).collect(),
            Self::Outputs => vec![
                Item::Webhook,
                Item::Soon("output-api"),
                Item::Soon("output-notify"),
            ],
        }
    }
}

/// What the panel offers, and what a step on the canvas is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Schedule,
    Manual,
    Read(Read),
    Filter(Only),
    Webhook,
    /// Offered, and marked as coming: nothing runs behind it.
    Soon(&'static str),
}

impl Item {
    /// The item a step is.
    pub fn of(step: &Step) -> Self {
        match step {
            Step::Schedule { .. } => Self::Schedule,
            Step::Manual {} => Self::Manual,
            Step::Read { what, .. } => Self::Read(what.clone()),
            Step::Filter { only } => Self::Filter(*only),
            Step::Webhook(_) => Self::Webhook,
        }
    }

    /// The message of its name; its note is the same with `-note`.
    pub fn id(&self) -> &'static str {
        match self {
            Self::Schedule => "trigger-schedule",
            Self::Manual => "trigger-manual",
            Self::Read(read) => match read {
                Read::Lists => "read-lists",
                Read::Unfollowers => "read-unfollowers",
                Read::Fans => "read-fans",
                Read::Friends => "read-friends",
                Read::Posts { .. } => "read-posts",
                Read::Picture { .. } => "read-picture",
                Read::KnownFollowers => "read-known-followers",
                Read::FriendsInCommon => "read-friends-in-common",
            },
            Self::Filter(only) => match only {
                Only::Changes => "filter-changes",
                Only::Verified => "filter-verified",
                Only::Unverified => "filter-unverified",
                Only::Public => "filter-public",
                Only::Private => "filter-private",
            },
            Self::Webhook => "output-webhook",
            Self::Soon(id) => id,
        }
    }

    pub fn drawing(&self) -> &'static str {
        match self {
            Self::Schedule => assets::SCHEDULE,
            Self::Manual => assets::RUN,
            Self::Read(Read::Posts { .. } | Read::Picture { .. }) => assets::PHOTO,
            Self::Read(_) => assets::FOLLOWERS,
            Self::Filter(_) => assets::CONDITION,
            Self::Webhook | Self::Soon("trigger-webhook") => assets::WEBHOOK,
            Self::Soon("output-api") => assets::API,
            Self::Soon(_) => assets::NOTIFY,
        }
    }

    /// The step it puts on the canvas, as good as it can be before
    /// anything is said: a schedule every day, a manual trigger, an action as
    /// the account in use unless it is about another. None for what is not
    /// there yet.
    pub fn step(&self, in_use: Option<Pk>) -> Option<Step> {
        Some(match self {
            Self::Schedule => Step::Schedule {
                when: ScheduleForm {
                    every: "1d".to_string(),
                    ..ScheduleForm::default()
                }
                .spec()
                .unwrap_or_default(),
            },
            Self::Manual => Step::Manual {},
            Self::Read(what) => Step::Read {
                account: (!what.is_about_another()).then(|| Account::own(in_use)),
                what: what.clone(),
            },
            Self::Filter(only) => Step::Filter { only: *only },
            Self::Webhook => WebhookForm::default().step(),
            Self::Soon(_) => return None,
        })
    }
}

/// What an automation's page holds while it is open.
pub(crate) struct Opened {
    pub id: String,
    /// Its history, newest first: none until it is read.
    pub runs: Option<Vec<Run>>,
    /// When it is next due, once asked.
    pub next: Option<Next>,
    /// Whether "Run now" was pressed, and how it went.
    pub started: Option<Result<(), String>>,
}

/// The section while it is shown. **Goes with the screen**: what was typed
/// in the search and the canvas of an automation not created go with it.
pub(crate) struct Page {
    pub search: Entity<InputState>,
    /// The automations listed, once read.
    pub automations: Vec<Automation>,
    pub loaded: bool,
    /// The automation whose page is open.
    pub open: Option<Opened>,
    /// The automation the editor holds, when it is open.
    pub edit: Option<Edit>,
    /// The stage drawn on the last frame, the one before it, and since when
    /// the one drawn is, by the executor's clock: a stage comes in as a
    /// screen does.
    pub shown: Option<Stage>,
    pub left: Option<Stage>,
    pub since: Instant,
    /// The grid behind the introduction: how long it has drifted, on a
    /// clock that starts with the page.
    pub drift: Drift,
    pub born: Instant,
    pub editor: Editor,
    _watching: Subscription,
}

/// The automation the editor holds, and what is going on over its canvas.
pub(crate) struct Edit {
    pub state: Editing,
    /// The name's field: what is typed in it renames the automation.
    pub name: Entity<InputState>,
    pub selected: Option<snob_core::automation::NodeId>,
    /// A step being moved, and an arrow being drawn.
    pub grab: Option<Grab>,
    pub wire: Option<Wire>,
    /// Why the last arrow was refused, or the last save failed.
    pub said: Option<Said>,
    pub menu: Option<NodeMenu>,
    pub dialog: Option<Dialog>,
    pub leaving: Option<Leave>,
    pub _naming: Subscription,
}

/// What the editor says under the canvas: a refusal, or a failure to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Said {
    Unjoined(snob_core::automation::Unjoined),
    NotKept(String),
}

/// A step being moved: where it was picked up, in its own units, and
/// where it is now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Grab {
    pub id: snob_core::automation::NodeId,
    pub offset: (f32, f32),
    pub from: (i32, i32),
    pub at: (i32, i32),
}

impl Grab {
    pub fn moved(&self) -> bool {
        self.at != self.from
    }
}

/// The editor's canvas and its panel of steps.
pub(crate) struct Editor {
    /// Where the canvas is, and where a button sent it from and when.
    pub glide: Glide,
    pub glide_since: Instant,
    /// Where the pointer was while the canvas is dragged.
    pub dragging: Option<Point<Pixels>>,
    /// Whether the space bar is held, or Ctrl was under the pointer when it
    /// last moved: either way a drag with the left button moves the canvas.
    pub space: bool,
    pub ctrl: bool,
    /// The canvas, focused by a click on it so that its keys reach it.
    pub focus: FocusHandle,
    /// Where the canvas was last drawn, which the pointer is measured from.
    pub bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The panel of steps: what it shows, and since when it shows it.
    pub shelf: Shelf,
    pub shelf_since: Option<Instant>,
    /// The search in the steps of one kind, while it is open.
    pub search: Option<Search>,
    /// Each kind's way in, the way back to them and the magnifier: where
    /// focus goes as the panel moves.
    pub kinds: [FocusHandle; 4],
    pub shelf_back: FocusHandle,
    pub shelf_search: FocusHandle,
    /// **Whether the canvas still shows what it was fitted to**: it is
    /// fitted again when the window changes size, until the person moves
    /// or zooms it.
    pub fitted: bool,
    /// How big the canvas was when it was last fitted.
    pub fitted_for: Cell<gpui_kit::Size<Pixels>>,
}

/// The search in the panel's steps: its field, and since when it is open.
pub(crate) struct Search {
    pub input: Entity<InputState>,
    pub since: Instant,
    _typing: Subscription,
}

impl Editor {
    /// **Held at the triggers** for an automation that has none
    /// (`state::shelf`).
    pub(crate) fn new(has_a_trigger: bool, cx: &mut Context<Shell>) -> Self {
        let now = cx.background_executor().now();
        Self {
            glide: Glide::still(Canvas::default()),
            glide_since: now,
            dragging: None,
            space: false,
            ctrl: false,
            focus: cx.focus_handle().tab_stop(true),
            bounds: Rc::default(),
            shelf: Shelf::new(has_a_trigger),
            shelf_since: None,
            search: None,
            kinds: std::array::from_fn(|_| cx.focus_handle()),
            shelf_back: cx.focus_handle(),
            shelf_search: cx.focus_handle(),
            fitted: true,
            fitted_for: Cell::default(),
        }
    }

    /// Where the canvas is now, asking for the next frame while a button's
    /// glide goes on.
    pub fn canvas_now(&self, window: &mut Window, cx: &gpui_kit::App) -> Canvas {
        let reduce = Look::now(cx).settings.reduce_motion;
        let elapsed = self.since_glide(cx);
        if self.glide.moving(elapsed, reduce) {
            window.request_animation_frame();
        }
        self.glide.at(elapsed, reduce)
    }

    fn since_glide(&self, cx: &gpui_kit::App) -> Duration {
        cx.background_executor()
            .now()
            .saturating_duration_since(self.glide_since)
    }

    /// Where the canvas is now, without asking for a frame.
    pub fn canvas_here(&self, cx: &gpui_kit::App) -> Canvas {
        let reduce = Look::now(cx).settings.reduce_motion;
        self.glide.at(self.since_glide(cx), reduce)
    }

    /// The center of the canvas on the window.
    pub(crate) fn middle(&self) -> Point<Pixels> {
        self.bounds.get().center()
    }

    /// The point of the canvas under `at`, a point of the window.
    pub fn under(&self, at: Point<Pixels>, cx: &gpui_kit::App) -> (f32, f32) {
        let from = at - self.middle();
        self.canvas_here(cx)
            .in_the_canvas((f32::from(from.x), f32::from(from.y)))
    }
}

impl Page {
    pub fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        let placeholder = Look::now(cx).tr("automations-search");
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let watching = cx.subscribe_in(&search, window, |_, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });
        let now = cx.background_executor().now();
        Self {
            search,
            automations: Vec::new(),
            loaded: false,
            open: None,
            edit: None,
            shown: None,
            left: None,
            since: now,
            drift: Drift::default(),
            born: now,
            editor: Editor::new(true, cx),
            _watching: watching,
        }
    }
}

impl Shell {
    /// "Get started": the introduction is not shown to the account in use
    /// again, here or after a restart.
    pub(crate) fn begin_the_automations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pk) = self.active().map(|account| account.named.pk) else {
            return;
        };
        tracing::info!(action = "automations_begin", "action");
        if self.introduced.begun(pk) {
            self.backend().remember_introduced(pk);
        }
        window.focus(&self.title, cx);
        cx.notify();
    }

    /// Reads the automations kept, for the list.
    pub(crate) fn load_the_automations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reading = self.backend().automations();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(all) = reading.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(page) = &mut this.automations {
                    page.automations = all;
                    page.loaded = true;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// A press on an automation of the list: its page, its history read.
    pub(crate) fn open_an_automation(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = &mut self.automations else {
            return;
        };
        tracing::info!(action = "automation_open", "action");
        page.open = Some(Opened {
            id: id.clone(),
            runs: None,
            next: None,
            started: None,
        });
        let automation = page.automations.iter().find(|one| one.id == id).cloned();
        window.focus(&self.title, cx);
        self.read_the_history(id, automation, window, cx);
        cx.notify();
    }

    /// Reads an open automation's runs and when it is next due.
    pub(crate) fn read_the_history(
        &mut self,
        id: String,
        automation: Option<Automation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend().clone();
        let runs = backend.automation_runs(id.clone());
        let next = automation.map(|automation| backend.next_automation_run(automation));
        cx.spawn_in(window, async move |this, cx| {
            let runs = runs.await.unwrap_or_default();
            let next = match next {
                Some(next) => next.await.ok(),
                None => None,
            };
            this.update(cx, |this, cx| {
                if let Some(open) = this
                    .automations
                    .as_mut()
                    .and_then(|page| page.open.as_mut())
                    && open.id == id
                {
                    open.runs = Some(runs);
                    open.next = next;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The way back from an automation's page to the list.
    pub(crate) fn close_the_automation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = &mut self.automations else {
            return;
        };
        if page.open.take().is_some() {
            window.focus(&self.title, cx);
            cx.notify();
        }
    }

    /// "Run now": the engine starts the automation apart from this program.
    pub(crate) fn run_the_automation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self
            .automations
            .as_ref()
            .and_then(|page| page.open.as_ref())
            .map(|open| open.id.clone())
        else {
            return;
        };
        tracing::info!(action = "automation_run", "action");
        let starting = self.backend().run_automation(id.clone());
        cx.spawn_in(window, async move |this, cx| {
            let started = starting.await.unwrap_or_else(|_| Err(String::new()));
            this.update(cx, |this, cx| {
                if let Some(open) = this
                    .automations
                    .as_mut()
                    .and_then(|page| page.open.as_mut())
                    && open.id == id
                {
                    open.started = Some(started);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Create": a new automation, named as the editor names one, open on
    /// an empty canvas at its own size. **Nothing is kept until something
    /// is done to it.**
    pub(crate) fn create_an_automation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.automations.is_none() {
            return;
        }
        tracing::info!(action = "automation_create", "action");
        let asking = self.backend().new_automation_id();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(id) = asking.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let name = Look::now(cx).tr("automation-new-name");
                this.open_the_editor(Automation::new(id, name), true, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// The pencil of an automation's page: its editor.
    pub(crate) fn edit_the_automation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = &self.automations else {
            return;
        };
        let Some(automation) = page
            .open
            .as_ref()
            .and_then(|open| page.automations.iter().find(|one| one.id == open.id))
            .cloned()
        else {
            return;
        };
        tracing::info!(action = "automation_edit", "action");
        self.open_the_editor(automation, false, window, cx);
    }

    /// Whether the editor is open: what Escape closes when nothing else
    /// is.
    pub(crate) fn is_editing(&self) -> bool {
        self.automations
            .as_ref()
            .is_some_and(|page| page.edit.is_some())
    }

    /// Whether an automation's page is open: what Escape closes when the
    /// editor is not.
    pub(crate) fn has_an_automation_open(&self) -> bool {
        self.automations
            .as_ref()
            .is_some_and(|page| page.open.is_some() && page.edit.is_none())
    }
}

impl Shell {
    /// Moves the canvas to `to`: at once, or with `glides` from where it
    /// is, as a button does.
    fn place_the_canvas(
        &mut self,
        to: impl FnOnce(Canvas, Point<Pixels>) -> Canvas,
        glides: bool,
        cx: &mut Context<Self>,
    ) {
        let reduce = Look::now(cx).settings.reduce_motion;
        let now = cx.background_executor().now();
        let Some(page) = &mut self.automations else {
            return;
        };
        let editor = &mut page.editor;
        let elapsed = now.saturating_duration_since(editor.glide_since);
        let from = editor.glide.at(elapsed, reduce);
        editor.fitted = false;
        let to = to(from, editor.middle());
        editor.glide = if glides {
            Glide { from, to }
        } else {
            Glide::still(to)
        };
        editor.glide_since = now;
        cx.notify();
    }

    /// The canvas moved and zoomed to show every step in what the panel
    /// leaves, at once or gliding. Nothing happens before the canvas has
    /// been drawn once, which is what tells how big it is.
    pub(crate) fn fit_the_canvas(&mut self, glides: bool, cx: &mut Context<Self>) {
        let Some(page) = &self.automations else {
            return;
        };
        let bounds = page.editor.bounds.get();
        let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if size.0 <= 0.0 || size.1 <= 0.0 {
            return;
        }
        let content = page
            .edit
            .as_ref()
            .and_then(|edit| crate::state::automations::content_of(edit.state.automation()));
        let target = crate::state::automations::fitting(content, size);
        page.editor.fitted_for.set(bounds.size);
        self.place_the_canvas(|_, _| target, glides, cx);
        if let Some(page) = &mut self.automations {
            page.editor.fitted = true;
        }
    }

    /// Fits the canvas again when it is still as it was fitted: what a
    /// first drawing and a window that changed size ask for.
    pub(crate) fn fit_the_canvas_if_untouched(&mut self, cx: &mut Context<Self>) {
        if self
            .automations
            .as_ref()
            .is_some_and(|page| page.editor.fitted)
        {
            self.fit_the_canvas(false, cx);
        }
    }

    /// A press of one of the zoom's buttons.
    pub(crate) fn zoom_the_canvas(&mut self, change: fn(Canvas) -> Canvas, cx: &mut Context<Self>) {
        self.place_the_canvas(|canvas, _| change(canvas), true, cx);
    }

    /// The wheel turned by `by` logical pixels over the canvas at `at`, on
    /// the window.
    pub(crate) fn wheel_the_canvas(&mut self, by: f32, at: Point<Pixels>, cx: &mut Context<Self>) {
        let factor = crate::state::automations::wheel_factor(by);
        self.place_the_canvas(
            |canvas, middle| {
                let from = at - middle;
                canvas.zoomed_at(factor, (f32::from(from.x), f32::from(from.y)))
            },
            false,
            cx,
        );
    }

    /// Moves the canvas by `by` logical pixels, at once or gliding.
    pub(crate) fn pan_the_canvas(&mut self, by: (f32, f32), glides: bool, cx: &mut Context<Self>) {
        self.place_the_canvas(|canvas, _| canvas.panned(by), glides, cx);
    }

    /// A press on the canvas itself: focus comes to it, nothing is chosen
    /// any more, and a drag starts when it is one that moves it.
    pub(crate) fn press_on_the_canvas(
        &mut self,
        at: Point<Pixels>,
        moves: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = &mut self.automations else {
            return;
        };
        // A press on a step, or on the handle an arrow starts from, has
        // been taken by it already.
        if page
            .edit
            .as_ref()
            .is_some_and(|edit| edit.grab.is_some() || edit.wire.is_some())
        {
            return;
        }
        window.focus(&page.editor.focus, cx);
        let moves = moves || page.editor.space;
        if moves {
            page.editor.dragging = Some(at);
        }
        if let Some(edit) = &mut page.edit {
            edit.selected = None;
            edit.said = None;
        }
        cx.notify();
    }

    /// The pointer moved over the canvas to `at`, with Ctrl held or not:
    /// the canvas moves with it, or the step or the arrow that is held.
    pub(crate) fn point_on_the_canvas(
        &mut self,
        at: Point<Pixels>,
        ctrl: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = &mut self.automations else {
            return;
        };
        let world = page.editor.under(at, cx);
        let editor = &mut page.editor;
        let changed = editor.ctrl != ctrl;
        editor.ctrl = ctrl;
        if let Some(was) = editor.dragging {
            editor.dragging = Some(at);
            let by = at - was;
            self.pan_the_canvas((f32::from(by.x), f32::from(by.y)), false, cx);
            return;
        }
        let mut moved = changed;
        if let Some(edit) = &mut page.edit {
            if let Some(grab) = &mut edit.grab {
                let next = (
                    (world.0 - grab.offset.0).round() as i32,
                    (world.1 - grab.offset.1).round() as i32,
                );
                moved |= next != grab.at;
                grab.at = next;
            }
            if let Some(wire) = &mut edit.wire {
                wire.to = world;
                moved = true;
            }
        }
        if moved {
            cx.notify();
        }
    }

    /// The button was let go over, or away from, the canvas at `at`: the
    /// canvas stops moving, a step is put where it was dropped, and an
    /// arrow is drawn to the step it was let go on.
    pub(crate) fn let_go_of_the_canvas(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(page) = &mut self.automations else {
            return;
        };
        let mut changed = page.editor.dragging.take().is_some();
        let world = page.editor.under(at, cx);
        let Some(edit) = &mut page.edit else {
            if changed {
                cx.notify();
            }
            return;
        };
        let (grab, wire) = (edit.grab.take(), edit.wire.take());
        changed |= grab.is_some() || wire.is_some();
        let mut saves = false;
        if let Some(grab) = grab.filter(Grab::moved) {
            saves |= edit.state.move_to(grab.id, grab.at);
        }
        if let Some(wire) = wire
            && let Some(to) =
                crate::state::automations::step_at(edit.state.automation(), world, Some(wire.from))
        {
            match edit.state.connect(wire.from, to) {
                Ok(()) => saves = true,
                Err(why) => edit.said = Some(Said::Unjoined(why)),
            }
        }
        if saves {
            self.keep_the_copy(cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// The space bar held or let go over the canvas.
    pub(crate) fn hold_space_on_the_canvas(&mut self, held: bool, cx: &mut Context<Self>) {
        if let Some(page) = &mut self.automations
            && page.editor.space != held
        {
            page.editor.space = held;
            cx.notify();
        }
    }

    /// Into a kind of step in the panel, its way back focused.
    pub(crate) fn enter_the_steps(
        &mut self,
        steps: Steps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = cx.background_executor().now();
        let Some(page) = &mut self.automations else {
            return;
        };
        let editor = &mut page.editor;
        if editor.shelf.enter(steps.kind()) {
            editor.shelf_since = Some(now);
            editor.search = None;
            window.focus(&editor.shelf_back, cx);
            cx.notify();
        }
    }

    /// Back to the kinds of step, the one just left focused. Whether it
    /// went: never while the panel is held at the triggers.
    pub(crate) fn back_to_the_kinds(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let now = cx.background_executor().now();
        let Some(page) = &mut self.automations else {
            return false;
        };
        let editor = &mut page.editor;
        let left = match editor.shelf.at() {
            crate::state::shelf::At::In(kind) => Steps::of(kind),
            crate::state::shelf::At::Kinds => return false,
        };
        if !editor.shelf.back() {
            return false;
        }
        editor.shelf_since = Some(now);
        editor.search = None;
        window.focus(&editor.kinds[left.at()], cx);
        cx.notify();
        true
    }

    /// The search in the steps shown, opened and focused, or closed and
    /// emptied.
    pub(crate) fn toggle_the_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self
            .automations
            .as_ref()
            .is_some_and(|page| page.editor.search.is_some());
        if open {
            self.close_the_search(window, cx);
            return;
        }
        let placeholder = Look::now(cx).tr("editor-shelf-search-hint");
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let typing = cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let now = cx.background_executor().now();
        if let Some(page) = &mut self.automations {
            page.editor.search = Some(Search {
                input,
                since: now,
                _typing: typing,
            });
        }
        cx.notify();
    }

    /// Closes the search in the panel, focus back on its magnifier. Whether
    /// there was one open.
    pub(crate) fn close_the_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(page) = &mut self.automations else {
            return false;
        };
        if page.editor.search.take().is_none() {
            return false;
        }
        window.focus(&page.editor.shelf_search, cx);
        cx.notify();
        true
    }
}
