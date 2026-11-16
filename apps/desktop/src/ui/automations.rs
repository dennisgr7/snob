//! The automations section while it is shown: what its stages hold between
//! two frames (the list's search, the editor's canvas, where the grid
//! behind the introduction has drifted to), and what the person does there.
//! What is shown is decided in `state::automations`; this keeps it and acts
//! on it, and `screens::automations` draws it.
//!
//! **Nothing here reaches the engine but "Get started"**, which is
//! remembered for the account in use (`Backend::remember_introduced`).
//! There is no backend for automations yet: "Create" opens an empty canvas
//! and nothing on it is kept.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{Bounds, Context, Entity, FocusHandle, Pixels, Point, Subscription, Window};

use super::look::Look;
use super::shell::Shell;
use crate::state::automations::{Automation, Canvas, Drift, Glide, Stage};

/// How often the grid behind the introduction is drawn again while it
/// drifts: sixty times a second, whatever the display's rate. It moves a few
/// pixels a second, and a frame for every refresh of a fast display would
/// cost the processor for nothing.
pub const DRIFT_FRAME: Duration = Duration::from_micros(16_667);

/// The editor's three kinds of steps, in the order its panel lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Steps {
    Triggers,
    Conditions,
    Actions,
}

impl Steps {
    pub const ALL: [Self; 3] = [Self::Triggers, Self::Conditions, Self::Actions];

    fn at(self) -> usize {
        match self {
            Self::Triggers => 0,
            Self::Conditions => 1,
            Self::Actions => 2,
        }
    }
}

/// The section while it is shown. **Goes with the screen**: what was typed
/// in the search and the canvas of an automation not created go with it.
pub(crate) struct Page {
    pub search: Entity<InputState>,
    /// The automations listed: none until there is a backend for them.
    pub automations: Vec<Automation>,
    /// Whether the editor is open.
    pub editing: bool,
    /// The stage drawn on the last frame, the one before it, and since when
    /// the one drawn is, by the executor's clock: a stage comes in as a
    /// screen does.
    pub shown: Option<Stage>,
    pub left: Option<Stage>,
    pub since: Instant,
    /// The grid behind the introduction: how long it has drifted, on a
    /// clock that starts with the page, and whether a frame is on its way.
    pub drift: Drift,
    pub born: Instant,
    pub ticking: bool,
    pub editor: Editor,
    _watching: Subscription,
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
    /// Each kind of step: whether it is unfolded, and since when.
    pub open: [(bool, Option<Instant>); 3],
}

impl Editor {
    fn new(cx: &mut Context<Shell>) -> Self {
        let now = cx.background_executor().now();
        Self {
            glide: Glide::still(Canvas::default()),
            glide_since: now,
            dragging: None,
            space: false,
            ctrl: false,
            focus: cx.focus_handle().tab_stop(true),
            bounds: Rc::default(),
            // **The triggers are unfolded**: an automation starts with one,
            // and there is none yet.
            open: [(true, None), (false, None), (false, None)],
        }
    }

    /// Where the canvas is now, asking for the next frame while a button's
    /// glide goes on.
    pub fn canvas_now(&self, window: &mut Window, cx: &gpui_kit::App) -> Canvas {
        let reduce = Look::now(cx).settings.reduce_motion;
        let elapsed = cx
            .background_executor()
            .now()
            .saturating_duration_since(self.glide_since);
        if self.glide.moving(elapsed, reduce) {
            window.request_animation_frame();
        }
        self.glide.at(elapsed, reduce)
    }

    /// Whether `steps` is unfolded.
    pub fn is_open(&self, steps: Steps) -> bool {
        self.open[steps.at()].0
    }

    /// Since when `steps` was last folded or unfolded: never, at a start.
    pub fn toggled(&self, steps: Steps) -> Option<Instant> {
        self.open[steps.at()].1
    }

    /// The center of the canvas on the window.
    fn middle(&self) -> Point<Pixels> {
        self.bounds.get().center()
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
            editing: false,
            shown: None,
            left: None,
            since: now,
            drift: Drift::default(),
            born: now,
            ticking: false,
            editor: Editor::new(cx),
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

    /// "Create": the editor, on an empty canvas at its own size.
    pub(crate) fn create_an_automation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = &mut self.automations else {
            return;
        };
        tracing::info!(action = "automation_create", "action");
        page.editing = true;
        page.editor = Editor::new(cx);
        window.focus(&self.title, cx);
        cx.notify();
    }

    /// Back from the editor to the list. Nothing drawn is kept: there was
    /// nothing to draw yet.
    pub(crate) fn leave_the_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = &mut self.automations else {
            return;
        };
        if !page.editing {
            return;
        }
        page.editing = false;
        window.focus(&self.title, cx);
        cx.notify();
    }

    /// Whether the editor is open: what Escape closes when nothing else
    /// is.
    pub(crate) fn is_editing(&self) -> bool {
        self.automations.as_ref().is_some_and(|page| page.editing)
    }

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
        let to = to(from, editor.middle());
        editor.glide = if glides {
            Glide { from, to }
        } else {
            Glide::still(to)
        };
        editor.glide_since = now;
        cx.notify();
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

    /// A press on the canvas: focus comes to it, and a drag starts when it
    /// is one that moves it.
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
        window.focus(&page.editor.focus, cx);
        let moves = moves || page.editor.space;
        if moves {
            page.editor.dragging = Some(at);
        }
        cx.notify();
    }

    /// The pointer moved over the canvas to `at`, with Ctrl held or not.
    pub(crate) fn point_on_the_canvas(
        &mut self,
        at: Point<Pixels>,
        ctrl: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = &mut self.automations else {
            return;
        };
        let editor = &mut page.editor;
        let changed = editor.ctrl != ctrl;
        editor.ctrl = ctrl;
        match editor.dragging {
            Some(was) => {
                editor.dragging = Some(at);
                let by = at - was;
                self.pan_the_canvas((f32::from(by.x), f32::from(by.y)), false, cx);
            }
            None if changed => cx.notify(),
            None => {}
        }
    }

    /// The button that moved the canvas was let go.
    pub(crate) fn let_go_of_the_canvas(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = &mut self.automations
            && page.editor.dragging.take().is_some()
        {
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

    /// Unfolds a kind of step in the panel, or folds it.
    pub(crate) fn toggle_the_steps(&mut self, steps: Steps, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        if let Some(page) = &mut self.automations {
            let open = &mut page.editor.open[steps.at()];
            *open = (!open.0, Some(now));
            cx.notify();
        }
    }
}
