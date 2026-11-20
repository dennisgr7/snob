//! The automations section, apart from how it is drawn: whether an account
//! is still owed its introduction, which automations a search shows, what
//! their cards say and how many sit side by side, whether a page reads its
//! runs again, the editor's canvas (how far it is moved and zoomed, where a step is and
//! what is under a point of it), the arrows between steps (which input one
//! being drawn sticks to, the way it takes and where its head points), and
//! the grid that drifts behind the introduction.
//!
//! **The introduction is shown once an account**: "Get started" is
//! remembered for the account in use (`prefs::Prefs::introduced`), and an
//! account signed in later is shown it in its turn.
//!
//! **Nothing here runs an automation**: what is kept of one is
//! `snob_engine::automations`'s, reached through the backend; the editor's
//! working copy is `editing`.
//!
//! **No window and no clock**: a moment is handed in as how long something
//! has run, and every move ends in a fixed state but the grid's drift, which
//! is decoration and stops whenever the window is not looked at.

use std::f64::consts::TAU;
use std::time::Duration;

use snob_core::Pk;
use snob_core::automation::{Automation, Node, NodeId, Stage as StepStage, Step, can_join};
use snob_core::watch::schedule::Spec;

use super::settings::fold;
use crate::theme::motion;

/// The accounts that have been shown the introduction and went past it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Introduced(Vec<Pk>);

impl Introduced {
    pub fn new(pks: Vec<Pk>) -> Self {
        let mut introduced = Self::default();
        introduced.merge(&pks);
        introduced
    }

    /// Whether the account `pk` is still owed the introduction. Nobody
    /// signed in is owed nothing.
    pub fn owed_to(&self, pk: Option<Pk>) -> bool {
        pk.is_some_and(|pk| !self.0.contains(&pk))
    }

    /// Notes that `pk` went past the introduction, and says whether that
    /// is new, which is when it is to be remembered.
    pub fn begun(&mut self, pk: Pk) -> bool {
        if self.0.contains(&pk) {
            return false;
        }
        self.0.push(pk);
        true
    }

    /// Adds what a second read of what is remembered found. **Nothing is
    /// taken away**: a "Get started" not yet written down is not undone by
    /// a read that came first.
    pub fn merge(&mut self, pks: &[Pk]) {
        for pk in pks {
            self.begun(*pk);
        }
    }
}

/// What the section shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// What automations are, and "Get started", once an account.
    Intro,
    /// The automations, a search among them and "Create".
    List,
    /// One automation: its runs, its flow in words, and the way to its
    /// editor.
    Page,
    /// The canvas an automation is drawn on.
    Editor,
}

/// The stage shown to an account owed the introduction or not, with an
/// automation open or not, and its editor open or not.
pub fn stage(owed: bool, open: bool, editing: bool) -> Stage {
    match (owed, open, editing) {
        (true, _, _) => Stage::Intro,
        (false, _, true) => Stage::Editor,
        (false, true, false) => Stage::Page,
        (false, false, false) => Stage::List,
    }
}

/// The automations `typed` asks for, in their order: those whose name holds
/// every word typed, whatever the case and the accents (as the settings
/// are searched, `settings::shown`). Nothing typed shows every one.
pub fn shown<'a>(all: &'a [Automation], typed: &str) -> Vec<&'a Automation> {
    let wanted = fold(typed);
    all.iter()
        .filter(|automation| super::commands::matches(&fold(&automation.name), &wanted))
        .collect()
}

/// What a card of the list says an automation does: its enabled steps that
/// read, keep and send, in the order a run takes them. Its triggers are
/// said apart ([`card_runs`]).
pub fn card_steps(automation: &Automation) -> Vec<&Node> {
    automation
        .columns()
        .into_iter()
        .flatten()
        .filter_map(|id| automation.node(id))
        .filter(|node| node.enabled && node.step.stage() != StepStage::Trigger)
        .collect()
}

/// When a card says an automation runs.
#[derive(Debug, Clone, PartialEq)]
pub enum Runs<'a> {
    /// On each of its enabled schedules.
    On(Vec<&'a Spec>),
    /// Only when somebody runs it.
    ByHand,
    /// It has no enabled trigger.
    Never,
}

/// When an automation runs, from its enabled triggers: its schedules when
/// it has any, by hand when a manual trigger is all it has.
pub fn card_runs(automation: &Automation) -> Runs<'_> {
    let enabled = || automation.nodes.iter().filter(|node| node.enabled);
    let schedules: Vec<&Spec> = enabled()
        .filter_map(|node| match &node.step {
            Step::Schedule { when } => Some(when),
            _ => None,
        })
        .collect();
    if !schedules.is_empty() {
        Runs::On(schedules)
    } else if enabled().any(|node| matches!(node.step, Step::Manual {})) {
        Runs::ByHand
    } else {
        Runs::Never
    }
}

/// The step a card is drawn for: its first enabled action, in the order a
/// run takes them.
pub fn card_step(automation: &Automation) -> Option<&Step> {
    card_steps(automation)
        .into_iter()
        .map(|node| &node.step)
        .find(|step| step.stage() == StepStage::Action)
}

/// The most cards side by side, however wide the window.
pub const MOST_CARD_COLUMNS: usize = 3;

/// How many cards of at least `card` wide, `gap` apart, fit side by side in
/// `room`: one at least, [`MOST_CARD_COLUMNS`] at most.
pub fn card_columns(room: f32, card: f32, gap: f32) -> usize {
    if !(room > 0.0 && card > 0.0) {
        return 1;
    }
    let fit = ((room + gap) / (card + gap)).floor();
    (fit as usize).clamp(1, MOST_CARD_COLUMNS)
}

/// What an automation's page knows when it decides whether to read its runs
/// again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Live {
    /// Whether the engine says one of its runs is going.
    pub running: bool,
    /// Whether "Run now" was pressed and its run has not shown up yet.
    pub waiting: bool,
    /// Whether the page is the one shown, its editor closed.
    pub shown: bool,
    /// Whether the app rests (`state::rest`).
    pub resting: bool,
    /// Whether a test build was told the moment it is (`ui::now`), which
    /// holds what is shown still for a picture.
    pub pinned: bool,
}

/// Whether the page reads its runs again shortly: **only while a run goes
/// or is awaited**, never at rest, never while the page is not shown, and
/// never in a test build told the moment it is.
pub fn reads_again(live: Live) -> bool {
    (live.running || live.waiting) && live.shown && !live.resting && !live.pinned
}

/// The least the canvas is zoomed to: a quarter, where a flow of a dozen
/// steps still fits a small window.
pub const ZOOM_MIN: f32 = 0.25;
/// The most: twice, past which a step's words are larger than anything
/// needs.
pub const ZOOM_MAX: f32 = 2.0;

/// The zooms the buttons go through, as a browser's do, so that a press
/// lands on a round number whatever the wheel did before it.
const ZOOM_STEPS: [f32; 12] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0,
];

/// How much one logical pixel of the wheel zooms: a notch of three lines of
/// 20 pixels is some 13%.
const WHEEL_ZOOMS: f32 = 0.002;

/// How big a step is drawn on the canvas, in the canvas's own units.
pub const NODE_W: f32 = 210.0;
pub const NODE_H: f32 = 72.0;

/// How wide the editor's panel of steps is at the default text size, the
/// most of the editor's width it takes, and the room kept on each side of
/// it.
pub const PANEL_WIDTH: f32 = 288.0;
pub const PANEL_SHARE: f32 = 0.4;
pub const PANEL_INSET: f32 = 16.0;

/// How wide the panel is drawn in an editor `width` wide, its words
/// `scale` times their default size.
pub fn panel_width(width: f32, scale: f32) -> f32 {
    (PANEL_WIDTH * scale).min(width * PANEL_SHARE)
}

/// How much of a view's width the editor's panel hides at the right, with
/// its margins.
pub fn panel_reserve(width: f32, scale: f32) -> f32 {
    panel_width(width, scale) + 2.0 * PANEL_INSET
}

/// How much of a view's height the editor's top bar and zoom bar hide, each
/// above and below.
pub const BAR_RESERVE: f32 = 72.0;

/// The room kept round what is fitted, in the canvas's own units.
const FIT_MARGIN: f32 = 40.0;

/// The distance between two dots of the canvas's grid, in the canvas's own
/// units, at a zoom of 1.
pub const GRID: f32 = 24.0;

/// The least distance between two dots on the screen: zoomed out further,
/// every other dot is left out, so the grid never turns into a gray.
const LEAST_GAP: f32 = 12.0;

/// How far an arrow key moves the canvas, in logical pixels.
pub const ARROW_PANS: f32 = 48.0;

/// Where the canvas is moved and how far it is zoomed: a point of the
/// canvas at `world` is drawn at `pan + world * zoom` from the middle of
/// what shows it. **It has no edge**: it is moved as far as anybody likes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Canvas {
    pan: (f32, f32),
    zoom: f32,
}

impl Default for Canvas {
    /// Its origin in the middle, at its own size.
    fn default() -> Self {
        Self {
            pan: (0.0, 0.0),
            zoom: 1.0,
        }
    }
}

impl Canvas {
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// How far its origin is from the middle of what shows it, in logical
    /// pixels.
    pub fn pan(&self) -> (f32, f32) {
        self.pan
    }

    /// The zoom as a person reads it: a whole percentage.
    pub fn percent(&self) -> u32 {
        (self.zoom * 100.0).round() as u32
    }

    /// Moved by `by` logical pixels, the way the pointer went.
    #[must_use]
    pub fn panned(self, by: (f32, f32)) -> Self {
        if !(by.0.is_finite() && by.1.is_finite()) {
            return self;
        }
        Self {
            pan: (self.pan.0 + by.0, self.pan.1 + by.1),
            ..self
        }
    }

    /// Zoomed by `factor` around `at`, a point measured from the middle of
    /// what shows the canvas: **what is under it stays under it**, as long
    /// as the zoom is inside its bounds.
    #[must_use]
    pub fn zoomed_at(self, factor: f32, at: (f32, f32)) -> Self {
        if !(factor.is_finite() && factor > 0.0 && at.0.is_finite() && at.1.is_finite()) {
            return self;
        }
        self.zoomed_to(self.zoom * factor, at)
    }

    /// At the zoom `zoom`, held inside its bounds, around `at`.
    #[must_use]
    pub fn zoomed_to(self, zoom: f32, at: (f32, f32)) -> Self {
        let zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        let kept = zoom / self.zoom;
        Self {
            pan: (
                at.0 - (at.0 - self.pan.0) * kept,
                at.1 - (at.1 - self.pan.1) * kept,
            ),
            zoom,
        }
    }

    /// One step in, around the middle: the next round zoom above.
    #[must_use]
    pub fn zoomed_in(self) -> Self {
        let next = ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > self.zoom + 0.005)
            .unwrap_or(ZOOM_MAX);
        self.zoomed_to(next, (0.0, 0.0))
    }

    /// One step out, around the middle: the next round zoom below.
    #[must_use]
    pub fn zoomed_out(self) -> Self {
        let next = ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < self.zoom - 0.005)
            .unwrap_or(ZOOM_MIN);
        self.zoomed_to(next, (0.0, 0.0))
    }

    /// Back at its own size, where it is.
    #[must_use]
    pub fn at_its_size(self) -> Self {
        self.zoomed_to(1.0, (0.0, 0.0))
    }

    pub fn can_zoom_in(&self) -> bool {
        self.zoom < ZOOM_MAX
    }

    pub fn can_zoom_out(&self) -> bool {
        self.zoom > ZOOM_MIN
    }

    /// Where the point `world` of the canvas is drawn, from the middle of
    /// what shows it.
    pub fn on_screen(&self, world: (f32, f32)) -> (f32, f32) {
        (
            self.pan.0 + world.0 * self.zoom,
            self.pan.1 + world.1 * self.zoom,
        )
    }

    /// The point of the canvas drawn at `screen`, measured from the middle
    /// of what shows it: the other way of [`Self::on_screen`].
    pub fn in_the_canvas(&self, screen: (f32, f32)) -> (f32, f32) {
        (
            (screen.0 - self.pan.0) / self.zoom,
            (screen.1 - self.pan.1) / self.zoom,
        )
    }

    /// The grid's dots on a view `size` big: how far apart they are and
    /// where the first one is from its top left corner, both in logical
    /// pixels. Every other dot goes as long as they would be closer than
    /// [`LEAST_GAP`].
    pub fn grid(&self, size: (f32, f32)) -> Grid {
        let mut gap = GRID * self.zoom;
        while gap < LEAST_GAP {
            gap *= 2.0;
        }
        // The origin's place from the top left, and the first dot left of
        // and above the view's corner.
        let origin = (size.0 / 2.0 + self.pan.0, size.1 / 2.0 + self.pan.1);
        Grid {
            gap,
            first: (origin.0.rem_euclid(gap), origin.1.rem_euclid(gap)),
        }
    }

    /// The canvas `through` of the way from `self` to `to`, from 0 to 1:
    /// zoomed at an even pace to the eye, which is evenly in proportion,
    /// and moved in a line.
    #[must_use]
    pub fn toward(self, to: Self, through: f32) -> Self {
        let at = through.clamp(0.0, 1.0);
        Self {
            pan: (
                self.pan.0 + (to.pan.0 - self.pan.0) * at,
                self.pan.1 + (to.pan.1 - self.pan.1) * at,
            ),
            zoom: self.zoom * (to.zoom / self.zoom).powf(at),
        }
    }
}

/// How much a turn of the wheel of `by` logical pixels zooms: in when
/// turned up (positive), out when turned down, the same proportion either
/// way.
pub fn wheel_factor(by: f32) -> f32 {
    if by.is_finite() {
        (by * WHEEL_ZOOMS).exp()
    } else {
        1.0
    }
}

/// The canvas's grid on a view: see [`Canvas::grid`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub gap: f32,
    pub first: (f32, f32),
}

/// The canvas on its way from one place to another after a button moved
/// it, `INTERACTION` long; a wheel or a drag moves it at once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glide {
    pub from: Canvas,
    pub to: Canvas,
}

impl Glide {
    /// A canvas still at `canvas`.
    pub fn still(canvas: Canvas) -> Self {
        Self {
            from: canvas,
            to: canvas,
        }
    }

    /// Where the canvas is `elapsed` into the glide; at once under
    /// "reduce motion".
    pub fn at(&self, elapsed: Duration, reduce: bool) -> Canvas {
        if reduce || self.from == self.to {
            return self.to;
        }
        let through = elapsed.as_secs_f32() / motion::INTERACTION.as_secs_f32();
        self.from.toward(self.to, motion::ease_out(through))
    }

    /// Whether it still moves `elapsed` into the glide.
    pub fn moving(&self, elapsed: Duration, reduce: bool) -> bool {
        !reduce && self.from != self.to && elapsed < motion::INTERACTION
    }
}

/// The topmost step under the point `at` of the canvas, leaving out
/// `except`.
pub fn step_at(automation: &Automation, at: (f32, f32), except: Option<NodeId>) -> Option<NodeId> {
    automation
        .nodes
        .iter()
        .rev()
        .filter(|node| Some(node.id) != except)
        .find(|node| {
            let (x, y) = (node.at.0 as f32, node.at.1 as f32);
            at.0 >= x && at.0 <= x + NODE_W && at.1 >= y && at.1 <= y + NODE_H
        })
        .map(|node| node.id)
}

/// Where an arrow leaves a step drawn at `at`: the middle of its right side.
pub fn leaves_at(at: (i32, i32)) -> (f32, f32) {
    (at.0 as f32 + NODE_W, at.1 as f32 + NODE_H / 2.0)
}

/// Where an arrow comes into a step drawn at `at`: the middle of its left
/// side.
pub fn comes_in_at(at: (i32, i32)) -> (f32, f32) {
    (at.0 as f32, at.1 as f32 + NODE_H / 2.0)
}

/// How near an arrow being drawn comes to a step's input before it sticks
/// to it, in logical pixels on the screen.
pub const SNAP_IN: f32 = 28.0;

/// How far from that input the pointer goes before the arrow lets go of
/// it: twice as far as it came, so that it does not flicker at the edge.
pub const SNAP_OUT: f32 = 56.0;

/// The inputs an arrow from `from` may stick to, at the middle of their
/// left side: **every step the arrow would be drawn to**
/// (`Automation::connect`), so none it is already drawn to, none of a
/// stage it cannot lead to (`can_join`) and none that would close a
/// circle.
pub fn inputs_for(automation: &Automation, from: NodeId) -> Vec<(NodeId, (f32, f32))> {
    let Some(stage) = automation.node(from).map(|node| node.step.stage()) else {
        return Vec::new();
    };
    let mut probe = automation.clone();
    automation
        .nodes
        .iter()
        .filter(|node| node.id != from && can_join(stage, node.step.stage()))
        .filter(|node| {
            let joins = probe.connect(from, node.id).is_ok();
            if joins {
                probe.disconnect(from, node.id);
            }
            joins
        })
        .map(|node| (node.id, comes_in_at(node.at)))
        .collect()
}

/// The input an arrow being drawn sticks to with the pointer at `at`, both
/// in the canvas's own units, drawn `zoom` times its size: the nearest of
/// `inputs` within [`SNAP_IN`] when it is nearer than the one held, else
/// **the one held while the pointer stays within [`SNAP_OUT`] of it**,
/// else none.
pub fn snapped(
    at: (f32, f32),
    inputs: &[(NodeId, (f32, f32))],
    held: Option<NodeId>,
    zoom: f32,
) -> Option<NodeId> {
    let zoom = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    let away = |input: (f32, f32)| (input.0 - at.0).hypot(input.1 - at.1) * zoom;
    let kept = held
        .and_then(|held| inputs.iter().find(|(id, _)| *id == held))
        .map(|(id, input)| (*id, away(*input)))
        .filter(|(_, away)| *away <= SNAP_OUT);
    let nearest = inputs
        .iter()
        .map(|(id, input)| (*id, away(*input)))
        .filter(|(_, away)| *away <= SNAP_IN)
        .min_by(|one, other| one.1.total_cmp(&other.1));
    match (kept, nearest) {
        (Some(kept), Some(nearest)) if nearest.1 < kept.1 => Some(nearest.0),
        (Some(kept), _) => Some(kept.0),
        (None, nearest) => nearest.map(|(id, _)| id),
    }
}

/// The least an arrow reaches out sideways from a step, in the canvas's
/// own units.
const REACH_MIN: f32 = 50.0;

/// How far from the steps' edges an arrow that goes round them passes.
const ROUND_BY: f32 = 24.0;

/// A piece of an arrow, a cubic Bezier: where it starts, its two control
/// points and where it ends, in the canvas's own units.
pub type Bend = [(f32, f32); 4];

/// The way an arrow takes from a step's output at `from` to `to`: an input
/// it comes into from the left, or the pointer when `loose`, which it heads
/// toward. **An arrow to a step behind its start goes round both steps**,
/// through the room between them or past the farther edge, rather than
/// across them.
pub fn route(from: (f32, f32), to: (f32, f32), loose: bool) -> Vec<Bend> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    if loose {
        let reach = (dx.abs() * 0.5).max(REACH_MIN);
        let ca = (from.0 + reach, from.1);
        let cb = ((ca.0 + to.0) / 2.0, (ca.1 + to.1) / 2.0);
        return vec![[from, ca, cb, to]];
    }
    if dx >= 0.0 {
        let reach = (dx * 0.5).max(REACH_MIN);
        return vec![[from, (from.0 + reach, from.1), (to.0 - reach, to.1), to]];
    }
    // Behind: down (or up) from the output, across, and into the input.
    let half = NODE_H / 2.0;
    let gap = dy.abs() - NODE_H;
    let across = if gap >= ROUND_BY {
        (from.1 + to.1) / 2.0
    } else if dy >= 0.0 {
        from.1.max(to.1) + half + ROUND_BY
    } else {
        from.1.min(to.1) - half - ROUND_BY
    };
    let reach = REACH_MIN;
    let (turn, back) = ((from.0, across), (to.0, across));
    let third = |one: f32, other: f32| (one + (other - one) / 3.0, one + (other - one) * 2.0 / 3.0);
    let (first, second) = third(turn.0, back.0);
    vec![
        [
            from,
            (from.0 + reach, from.1),
            (from.0 + reach, across),
            turn,
        ],
        [turn, (first, across), (second, across), back],
        [back, (to.0 - reach, across), (to.0 - reach, to.1), to],
    ]
}

/// Which way an arrow heads where it ends: the tangent of its last piece
/// there, of length 1. Rightward when it has no length.
pub fn heading(route: &[Bend]) -> (f32, f32) {
    let Some([a, ca, cb, b]) = route.last() else {
        return (1.0, 0.0);
    };
    [cb, ca, a]
        .into_iter()
        .map(|before| (b.0 - before.0, b.1 - before.1))
        .find_map(|(x, y)| {
            let length = x.hypot(y);
            (length > 1e-3 && length.is_finite()).then(|| (x / length, y / length))
        })
        .unwrap_or((1.0, 0.0))
}

/// The head of an arrow `size` long whose tip is at `tip`, turned to
/// `heading`: the tip and the two corners of its base.
pub fn head(tip: (f32, f32), heading: (f32, f32), size: f32) -> [(f32, f32); 3] {
    let (x, y) = heading;
    let base = (tip.0 - x * size * 1.5, tip.1 - y * size * 1.5);
    let side = (-y * size * 0.8, x * size * 0.8);
    [
        tip,
        (base.0 + side.0, base.1 + side.1),
        (base.0 - side.0, base.1 - side.1),
    ]
}

/// The corners of the box that holds every step: left, top, right and
/// bottom, in the canvas's own units. None when there is no step.
pub fn content_of(automation: &Automation) -> Option<(f32, f32, f32, f32)> {
    automation.nodes.iter().fold(None, |held, node| {
        let (x, y) = (node.at.0 as f32, node.at.1 as f32);
        let (right, bottom) = (x + NODE_W, y + NODE_H);
        Some(match held {
            None => (x, y, right, bottom),
            Some((l, t, r, b)) => (l.min(x), t.min(y), r.max(right), b.max(bottom)),
        })
    })
}

/// The least a fit zooms out to at the default text size, and twice the
/// text doubles it, up to the canvas's own size: **a step's words are still
/// read there** (its detail is 11 units, some 8 pixels at this zoom).
pub const FIT_LEAST: f32 = 0.75;

/// The canvas that shows `content` whole, as large as its own size at most,
/// in what the panel and the bars leave of a view `size` big, whose words
/// are `scale` times their default size. **Not zoomed out past where its
/// words are read** ([`FIT_LEAST`]) while the whole view holds it: what
/// does not fit beside the panel is shown from its start, at the left, and
/// goes on under the panel, never past the view's edge. With nothing to
/// show, the canvas at its own size with its middle where that room's is.
pub fn fitting(content: Option<(f32, f32, f32, f32)>, size: (f32, f32), scale: f32) -> Canvas {
    let reserve = panel_reserve(size.0, scale);
    let bars = BAR_RESERVE * scale;
    let middle_x = -reserve / 2.0;
    let Some((left, top, right, bottom)) = content else {
        return Canvas::default().panned((middle_x, 0.0));
    };
    let free = (
        (size.0 - reserve).max(100.0),
        (size.1 - 2.0 * bars).max(100.0),
    );
    let whole = (
        right - left + 2.0 * FIT_MARGIN,
        bottom - top + 2.0 * FIT_MARGIN,
    );
    // The least never puts a step past the window's edge: under the panel
    // at most, which floats over the canvas.
    let least = (FIT_LEAST * scale)
        .min(1.0)
        .min(size.0 / whole.0)
        .min(free.1 / whole.1);
    let zoom = (free.0 / whole.0)
        .min(free.1 / whole.1)
        .max(least)
        .clamp(ZOOM_MIN, 1.0);
    // Along each side, the middle of what fits, or its start.
    let across = |low: f32, high: f32, whole: f32, free: f32, room_from: f32, middle: f32| {
        if whole * zoom <= free + 0.5 {
            middle - (low + high) / 2.0 * zoom
        } else {
            room_from + (FIT_MARGIN - low) * zoom
        }
    };
    Canvas {
        pan: (
            across(left, right, whole.0, free.0, -size.0 / 2.0, middle_x),
            across(top, bottom, whole.1, free.1, -size.1 / 2.0 + bars, 0.0),
        ),
        zoom,
    }
}

/// The side of a cell of the grid behind the introduction, in logical
/// pixels.
pub const CELL: f32 = 44.0;

/// How fast that grid drifts, in logical pixels a second: down and to the
/// right, slowly enough to be read as calm rather than as something to
/// follow. 7 by 3.5 was reported as too slow to be noticed.
const DRIFT: (f64, f64) = (10.0, 5.0);

/// How long a wave of light takes to cross one of its lengths, and how long
/// that length is: one every seven seconds or so.
const WAVE_EVERY: f64 = 7.0;
const WAVE_LENGTH: f64 = 420.0;

/// How long the grid has drifted: it runs while it is shown and looked at,
/// and holds still otherwise, **taking up where it stopped** rather than
/// jumping by the time it was not looked at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Drift {
    /// What it ran before it last stopped.
    carried: Duration,
    /// When it last started, on the clock handed in.
    since: Option<Duration>,
}

impl Drift {
    /// How long it has run at `now`, a reading of any clock that only goes
    /// forward, `running` or not from now on.
    pub fn at(&mut self, now: Duration, running: bool) -> Duration {
        let ran = self.carried
            + self
                .since
                .map_or(Duration::ZERO, |since| now.saturating_sub(since));
        match (running, self.since) {
            (true, None) => self.since = Some(now),
            (false, Some(_)) => {
                self.carried = ran;
                self.since = None;
            }
            _ => {}
        }
        ran
    }
}

/// One stroke of the grid: a thin rectangle, in logical pixels from the
/// view's top left, and how much of the grid's ink it takes, from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub ink: f32,
}

/// A crossing of the grid lit by a passing wave: where, and how much.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spark {
    pub x: f32,
    pub y: f32,
    pub ink: f32,
}

/// The grid behind the introduction on a view `size` big, `ran` into its
/// drift: one stroke a side of each cell, **fainter toward the view's edges
/// and under the words** around `middle`, and brighter where a slow wave of
/// light passes; and the crossings the crest of the wave lights.
pub fn drifting_grid(
    size: (f32, f32),
    middle: (f32, f32),
    ran: Duration,
) -> (Vec<Stroke>, Vec<Spark>) {
    let (width, height) = size;
    if !(width > 0.0 && height > 0.0 && width.is_finite() && height.is_finite()) {
        return (Vec::new(), Vec::new());
    }
    // In seconds as an `f64`, each phase taken round its period: an `f32`
    // of a day's seconds would step visibly from frame to frame.
    let seconds = ran.as_secs_f64();
    let cell = f64::from(CELL);
    let shift = (
        ((seconds * DRIFT.0) % cell) as f32,
        ((seconds * DRIFT.1) % cell) as f32,
    );
    let wave = ((seconds % WAVE_EVERY) / WAVE_EVERY * TAU) as f32;
    let light = |x: f32, y: f32| {
        let along = (f64::from(x) * 0.8 + f64::from(y) * 0.6) / WAVE_LENGTH * TAU;
        0.5 + 0.5 * ((along as f32) - wave).cos()
    };
    let reach = (width.max(1.0) * 0.62, height.max(1.0) * 0.62);
    let seen = |x: f32, y: f32| {
        let from = (((x - middle.0) / reach.0).powi(2) + ((y - middle.1) / reach.1).powi(2)).sqrt();
        let edge = 1.0 - smoothstep(0.45, 1.0, from);
        let words = 0.45 + 0.55 * smoothstep(0.05, 0.4, from);
        edge * words
    };

    let columns = (width / CELL).ceil() as usize + 2;
    let rows = (height / CELL).ceil() as usize + 2;
    let line_x = |at: usize| shift.0 - CELL + at as f32 * CELL;
    let line_y = |at: usize| shift.1 - CELL + at as f32 * CELL;
    let mut strokes = Vec::with_capacity(columns * rows * 2);
    let mut sparks = Vec::new();
    for column in 0..columns {
        for row in 0..rows {
            let (x, y) = (line_x(column), line_y(row));
            // The cell's left side and its top, each lit at its middle.
            for (stroke, (mid_x, mid_y)) in [
                (
                    Stroke {
                        x: x - 0.5,
                        y,
                        width: 1.0,
                        height: CELL,
                        ink: 0.0,
                    },
                    (x, y + CELL / 2.0),
                ),
                (
                    Stroke {
                        x,
                        y: y - 0.5,
                        width: CELL,
                        height: 1.0,
                        ink: 0.0,
                    },
                    (x + CELL / 2.0, y),
                ),
            ] {
                let ink = seen(mid_x, mid_y) * (0.35 + 0.65 * light(mid_x, mid_y));
                if ink > 0.01 {
                    strokes.push(Stroke { ink, ..stroke });
                }
            }
            let crest = (light(x, y) - 0.86) / 0.14;
            if crest > 0.0 {
                let ink = crest * crest * seen(x, y);
                if ink > 0.02 {
                    sparks.push(Spark { x, y, ink });
                }
            }
        }
    }
    (strokes, sparks)
}

/// Hermite's step from 0 at `from` to 1 at `to`.
fn smoothstep(from: f32, to: f32, at: f32) -> f32 {
    let t = ((at - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECS: fn(u64) -> Duration = Duration::from_secs;

    fn near(one: f32, other: f32) -> bool {
        (one - other).abs() < 1e-3
    }

    #[test]
    fn the_introduction_is_owed_once_to_each_account_and_to_nobody_signed_out() {
        let (me, other) = (Pk::new(42), Pk::new(9001));
        let mut introduced = Introduced::default();
        assert!(introduced.owed_to(Some(me)));
        assert!(!introduced.owed_to(None), "nobody is owed anything");

        assert!(introduced.begun(me), "new: to be remembered");
        assert!(!introduced.owed_to(Some(me)));
        assert!(!introduced.begun(me), "and only once");
        assert!(introduced.owed_to(Some(other)), "each account its own");

        // A read of what was remembered from before the press takes
        // nothing away.
        introduced.merge(&[other]);
        assert!(!introduced.owed_to(Some(me)) && !introduced.owed_to(Some(other)));
        assert_eq!(Introduced::new(vec![me, me]), {
            let mut once = Introduced::default();
            once.begun(me);
            once
        });
    }

    #[test]
    fn the_introduction_comes_before_the_list_the_page_and_the_editor() {
        assert_eq!(stage(true, false, false), Stage::Intro);
        assert_eq!(stage(true, true, true), Stage::Intro);
        assert_eq!(stage(false, false, false), Stage::List);
        assert_eq!(stage(false, true, false), Stage::Page);
        assert_eq!(stage(false, true, true), Stage::Editor);
        assert_eq!(stage(false, false, true), Stage::Editor);
    }

    #[test]
    fn the_step_under_a_point_is_the_topmost_one_and_arrows_meet_its_sides() {
        use snob_core::automation::Step;
        use snob_core::watch::schedule::Spec;
        let mut automation = Automation::new("a1", "Daily");
        let step = || Step::Schedule {
            when: Spec::default(),
        };
        let below = automation.insert(step(), (0, 0));
        let above = automation.insert(step(), (100, 20));
        assert_eq!(step_at(&automation, (10.0, 10.0), None), Some(below));
        assert_eq!(step_at(&automation, (150.0, 40.0), None), Some(above));
        assert_eq!(
            step_at(&automation, (150.0, 40.0), Some(above)),
            Some(below)
        );
        assert_eq!(step_at(&automation, (-5.0, 10.0), None), None);
        assert_eq!(leaves_at((0, 0)), (NODE_W, NODE_H / 2.0));
        assert_eq!(comes_in_at((0, 0)), (0.0, NODE_H / 2.0));
        let canvas = Canvas::default()
            .panned((30.0, -12.0))
            .zoomed_to(1.5, (0.0, 0.0));
        let there = canvas.on_screen((40.0, 8.0));
        let back = canvas.in_the_canvas(there);
        assert!(near(back.0, 40.0) && near(back.1, 8.0));
    }

    #[test]
    fn a_search_finds_automations_by_every_word_of_their_name() {
        let all: Vec<Automation> = [
            "Watch new followers",
            "Daily report",
            "Unfollowers to Slack",
        ]
        .into_iter()
        .enumerate()
        .map(|(at, name)| Automation::new(format!("a{at}"), name))
        .collect();
        let names = |typed: &str| -> Vec<&str> {
            shown(&all, typed)
                .into_iter()
                .map(|automation| automation.name.as_str())
                .collect()
        };
        assert_eq!(names("").len(), 3, "nothing typed shows every one");
        assert_eq!(names("  ").len(), 3);
        assert_eq!(
            names("FOLLOW"),
            ["Watch new followers", "Unfollowers to Slack"]
        );
        assert_eq!(
            names("slack unfollow"),
            ["Unfollowers to Slack"],
            "in any order"
        );
        assert_eq!(
            names("r\u{e9}port"),
            ["Daily report"],
            "with or without accents"
        );
        assert!(names("weekly").is_empty());
    }

    #[test]
    fn fitting_shows_every_step_in_what_the_panel_leaves_and_never_zooms_past_its_own_size() {
        use snob_core::automation::Step;
        use snob_core::watch::schedule::Spec;
        let mut automation = Automation::new("a1", "Daily");
        assert_eq!(content_of(&automation), None);
        let step = || Step::Schedule {
            when: Spec::default(),
        };
        automation.insert(step(), (-245, -40));
        automation.insert(step(), (245, 120));
        let content = content_of(&automation).unwrap();
        assert_eq!(content, (-245.0, -40.0, 245.0 + NODE_W, 120.0 + NODE_H));

        for (size, scale) in [
            ((1028.0, 700.0), 1.0),
            ((3000.0, 2000.0), 1.0),
            ((2400.0, 1400.0), 2.0),
        ] {
            let canvas = fitting(Some(content), size, scale);
            assert!(canvas.zoom() <= 1.0 && canvas.zoom() >= ZOOM_MIN);
            let corners = [(content.0, content.1), (content.2, content.3)];
            let room = (
                (-size.0 / 2.0, size.0 / 2.0 - panel_reserve(size.0, scale)),
                (
                    -size.1 / 2.0 + BAR_RESERVE * scale,
                    size.1 / 2.0 - BAR_RESERVE * scale,
                ),
            );
            for corner in corners {
                let at = canvas.on_screen(corner);
                assert!(
                    at.0 >= room.0.0 - 1.0 && at.0 <= room.0.1 + 1.0,
                    "{size:?} {at:?}"
                );
                assert!(
                    at.1 >= room.1.0 - 1.0 && at.1 <= room.1.1 + 1.0,
                    "{size:?} {at:?}"
                );
            }
        }
        assert_eq!(fitting(Some(content), (3000.0, 2000.0), 1.0).zoom(), 1.0);
        let empty = fitting(None, (1028.0, 700.0), 1.0);
        assert_eq!((empty.zoom(), empty.pan().1), (1.0, 0.0));
        assert!(empty.pan().0 < 0.0, "toward the room the panel leaves");
    }

    #[test]
    fn a_flow_too_wide_to_read_whole_is_fitted_from_its_start_at_a_zoom_its_words_are_read() {
        use snob_core::automation::Step;
        use snob_core::watch::schedule::Spec;
        let mut automation = Automation::new("a1", "Daily");
        let step = || Step::Schedule {
            when: Spec::default(),
        };
        for at in 0..4 {
            automation.insert(step(), (at * 280 - 420, 0));
        }
        let content = content_of(&automation).unwrap();
        let whole = content.2 - content.0 + 2.0 * FIT_MARGIN;
        for (size, scale, least) in [
            ((1028.0, 700.0), 1.0, FIT_LEAST),
            ((728.0, 520.0), 1.0, 728.0 / whole),
            ((1028.0, 700.0), 2.0, 1028.0 / whole),
        ] {
            let canvas = fitting(Some(content), size, scale);
            assert!(
                (canvas.zoom() - least).abs() < 1e-4,
                "{size:?} at {scale}: {}",
                canvas.zoom()
            );
            // The first step whole, at the room's left, the last one inside
            // the view, and the flow's middle in the middle of its height.
            let first = canvas.on_screen((content.0, content.1));
            assert!(
                (first.0 - (-size.0 / 2.0 + FIT_MARGIN * least)).abs() < 0.5,
                "{first:?}"
            );
            let last = canvas.on_screen((content.2, content.3));
            assert!(last.0 <= size.0 / 2.0 + 0.5, "{last:?} in {size:?}");
            let middle = canvas.on_screen((0.0, (content.1 + content.3) / 2.0));
            assert!(middle.1.abs() < 0.5, "{middle:?}");
        }
    }

    #[test]
    fn the_wheel_zooms_around_the_pointer_and_keeps_what_is_under_it() {
        let canvas = Canvas::default().panned((30.0, -12.0));
        let pointer = (120.0, 80.0);
        // The point of the canvas under the pointer, before and after.
        let under = |canvas: &Canvas| {
            (
                (pointer.0 - canvas.pan().0) / canvas.zoom(),
                (pointer.1 - canvas.pan().1) / canvas.zoom(),
            )
        };
        let before = under(&canvas);
        let zoomed = canvas.zoomed_at(wheel_factor(120.0), pointer);
        assert!(zoomed.zoom() > 1.0, "turned up, it zooms in");
        let after = under(&zoomed);
        assert!(
            near(before.0, after.0) && near(before.1, after.1),
            "{before:?} {after:?}"
        );
        assert!(canvas.zoomed_at(wheel_factor(-120.0), pointer).zoom() < 1.0);
        assert!(
            near(wheel_factor(60.0) * wheel_factor(-60.0), 1.0),
            "in and out by as much"
        );
    }

    #[test]
    fn the_zoom_stays_inside_its_bounds_and_the_buttons_land_on_round_numbers() {
        let mut canvas = Canvas::default();
        for _ in 0..50 {
            canvas = canvas.zoomed_at(wheel_factor(500.0), (10.0, 10.0));
        }
        assert_eq!(canvas.zoom(), ZOOM_MAX);
        assert!(!canvas.can_zoom_in());
        for _ in 0..50 {
            canvas = canvas.zoomed_at(wheel_factor(-500.0), (10.0, 10.0));
        }
        assert_eq!(canvas.zoom(), ZOOM_MIN);
        assert!(!canvas.can_zoom_out() && canvas.can_zoom_in());

        let at_its_size = Canvas::default();
        assert_eq!(at_its_size.zoomed_in().percent(), 110);
        assert_eq!(at_its_size.zoomed_out().percent(), 90);
        let odd = at_its_size.zoomed_at(1.37, (0.0, 0.0));
        assert_eq!(odd.zoomed_in().percent(), 150);
        assert_eq!(odd.zoomed_out().percent(), 125);
        assert_eq!(odd.at_its_size().percent(), 100);
        assert_eq!(
            Canvas::default().zoomed_to(9.0, (0.0, 0.0)).zoom(),
            ZOOM_MAX
        );
        // Nonsense moves nothing.
        assert_eq!(at_its_size.zoomed_at(f32::NAN, (0.0, 0.0)), at_its_size);
        assert_eq!(at_its_size.zoomed_at(0.0, (0.0, 0.0)), at_its_size);
        assert_eq!(at_its_size.panned((f32::INFINITY, 0.0)), at_its_size);
    }

    #[test]
    fn the_canvas_moves_as_far_as_the_pointer_and_has_no_edge() {
        let mut canvas = Canvas::default();
        for _ in 0..1000 {
            canvas = canvas.panned((500.0, -500.0));
        }
        assert_eq!(canvas.pan(), (500_000.0, -500_000.0));
        assert_eq!(canvas.on_screen((0.0, 0.0)), canvas.pan());
        let zoomed = Canvas::default().zoomed_to(2.0, (0.0, 0.0));
        assert_eq!(zoomed.on_screen((10.0, -5.0)), (20.0, -10.0));
    }

    #[test]
    fn the_grid_follows_the_canvas_and_never_crowds() {
        let size = (800.0, 600.0);
        let grid = Canvas::default().grid(size);
        assert_eq!(grid.gap, GRID);
        // The origin, in the middle, is on a dot.
        assert!(near((400.0 - grid.first.0) % grid.gap, 0.0));
        let moved = Canvas::default().panned((5.0, 0.0)).grid(size);
        assert!(near(moved.first.0, (grid.first.0 + 5.0) % grid.gap));
        for zoom in [ZOOM_MIN, 0.3, 0.5, 0.7, 1.0, 1.6, ZOOM_MAX] {
            let grid = Canvas::default().zoomed_to(zoom, (0.0, 0.0)).grid(size);
            assert!(grid.gap >= LEAST_GAP, "{zoom}: {}", grid.gap);
            assert!(grid.first.0 >= 0.0 && grid.first.0 < grid.gap);
            assert!(grid.first.1 >= 0.0 && grid.first.1 < grid.gap);
        }
    }

    #[test]
    fn a_button_glides_the_canvas_and_ends_where_it_was_sent() {
        let from = Canvas::default();
        let to = from.zoomed_in();
        let glide = Glide { from, to };
        assert_eq!(glide.at(Duration::ZERO, false), from);
        let half = glide.at(motion::INTERACTION / 2, false).zoom();
        assert!(half > 1.0 && half < to.zoom(), "{half}");
        assert_eq!(glide.at(motion::INTERACTION, false), to);
        assert!(glide.moving(motion::INTERACTION / 2, false));
        assert!(!glide.moving(motion::INTERACTION, false));
        assert_eq!(
            glide.at(Duration::ZERO, true),
            to,
            "at once with less motion"
        );
        assert!(!glide.moving(Duration::ZERO, true));
        assert!(!Glide::still(to).moving(Duration::ZERO, false));
    }

    #[test]
    fn the_drift_holds_still_while_not_looked_at_and_takes_up_where_it_stopped() {
        let mut drift = Drift::default();
        assert_eq!(drift.at(SECS(100), true), Duration::ZERO);
        assert_eq!(drift.at(SECS(103), true), SECS(3));
        // Looked away from at 105, back at 160.
        assert_eq!(drift.at(SECS(105), false), SECS(5));
        assert_eq!(
            drift.at(SECS(140), false),
            SECS(5),
            "nothing moves meanwhile"
        );
        assert_eq!(drift.at(SECS(160), true), SECS(5), "no jump");
        assert_eq!(drift.at(SECS(162), true), SECS(7));
        // A clock read out of order costs nothing.
        assert_eq!(drift.at(SECS(150), true), SECS(5));
    }

    #[test]
    fn the_grid_drifts_slowly_and_fades_toward_the_edges() {
        let size = (1000.0, 700.0);
        let middle = (500.0, 350.0);
        let (still, sparks) = drifting_grid(size, middle, Duration::ZERO);
        assert!(!still.is_empty());
        for stroke in &still {
            assert!((0.0..=1.0).contains(&stroke.ink), "{stroke:?}");
        }
        for spark in &sparks {
            assert!((0.0..=1.0).contains(&spark.ink), "{spark:?}");
        }
        // Faint at the corners, more there half way out.
        let ink_near = |strokes: &[Stroke], x: f32, y: f32| {
            strokes
                .iter()
                .filter(|stroke| (stroke.x - x).abs() < CELL && (stroke.y - y).abs() < CELL)
                .map(|stroke| stroke.ink)
                .fold(0.0, f32::max)
        };
        assert!(ink_near(&still, 5.0, 5.0) < 0.05);
        assert!(ink_near(&still, 230.0, 350.0) > ink_near(&still, 5.0, 5.0));

        // A second later every line has moved less than a quarter of a
        // cell, and the same again after a day.
        for at in [0, 86_400] {
            let one = drifting_grid(size, middle, SECS(at));
            let next = drifting_grid(size, middle, SECS(at) + SECS(1));
            let x = |strokes: &[Stroke]| {
                strokes
                    .iter()
                    .find(|stroke| stroke.width == 1.0)
                    .map(|stroke| stroke.x.rem_euclid(CELL))
                    .unwrap()
            };
            let moved = (x(&next.0) - x(&one.0)).rem_euclid(CELL);
            assert!(moved > 0.0 && moved < CELL / 4.0, "{moved} after {at} s");
        }
        assert!(drifting_grid((0.0, 0.0), middle, SECS(1)).0.is_empty());
    }

    fn a_flow() -> Automation {
        use snob_core::automation::{Account, Only, Read, Webhook};
        let mut automation = Automation::new("a1", "Daily");
        let every = Spec {
            every: Some(SECS(6 * 3600)),
            ..Spec::default()
        };
        let trigger = automation.insert(Step::Schedule { when: every }, (0, 0));
        let hook = automation.insert(Step::Webhook(Webhook::default()), (600, 0));
        let filter = automation.insert(
            Step::Filter {
                only: Only::Changes,
            },
            (400, 0),
        );
        let read = automation.insert(
            Step::Read {
                account: Some(Account::own(None)),
                what: Read::Fans,
            },
            (200, 0),
        );
        for (from, to) in [(trigger, read), (read, filter), (filter, hook)] {
            automation.connect(from, to).unwrap();
        }
        automation
    }

    #[test]
    fn a_card_lists_the_enabled_steps_after_the_triggers_in_the_order_a_run_takes_them() {
        let mut automation = a_flow();
        let stages = |automation: &Automation| {
            card_steps(automation)
                .iter()
                .map(|node| node.step.stage())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            stages(&automation),
            [StepStage::Action, StepStage::Filter, StepStage::Output],
            "in the order of the arrows, not of the canvas"
        );
        assert!(matches!(card_step(&automation), Some(Step::Read { .. })));
        automation.nodes[3].enabled = false;
        assert_eq!(stages(&automation), [StepStage::Filter, StepStage::Output]);
        assert_eq!(card_step(&automation), None, "a disabled read is not drawn");
        assert!(card_steps(&Automation::new("a2", "Empty")).is_empty());
    }

    #[test]
    fn a_card_runs_on_its_schedules_else_by_hand_else_never() {
        let mut automation = a_flow();
        assert!(matches!(card_runs(&automation), Runs::On(specs) if specs.len() == 1));
        automation.insert(Step::Manual {}, (0, 100));
        assert!(
            matches!(card_runs(&automation), Runs::On(_)),
            "a schedule is said over a manual trigger"
        );
        automation.nodes[0].enabled = false;
        assert_eq!(card_runs(&automation), Runs::ByHand);
        automation.nodes.retain(|node| node.step != Step::Manual {});
        assert_eq!(card_runs(&automation), Runs::Never);
    }

    #[test]
    fn as_many_cards_sit_side_by_side_as_fit_from_one_to_three() {
        assert_eq!(card_columns(900.0, 280.0, 12.0), 3);
        assert_eq!(card_columns(572.0, 280.0, 12.0), 2, "two exactly");
        assert_eq!(card_columns(571.0, 280.0, 12.0), 1);
        assert_eq!(card_columns(5000.0, 280.0, 12.0), MOST_CARD_COLUMNS);
        assert_eq!(card_columns(100.0, 280.0, 12.0), 1, "one, however narrow");
        assert_eq!(card_columns(0.0, 280.0, 12.0), 1);
        assert_eq!(card_columns(f32::NAN, 280.0, 12.0), 1);
    }

    #[test]
    fn an_arrow_sticks_to_an_input_it_comes_near_and_lets_go_only_far_from_it() {
        let input = (100.0, 0.0);
        let inputs = [(7, input)];
        let at = |x: f32| (input.0 - x, input.1);
        assert_eq!(snapped(at(40.0), &inputs, None, 1.0), None, "too far");
        assert_eq!(snapped(at(20.0), &inputs, None, 1.0), Some(7), "comes near");
        assert_eq!(
            snapped(at(50.0), &inputs, Some(7), 1.0),
            Some(7),
            "held between the two distances"
        );
        assert_eq!(
            snapped(at(50.0), &inputs, None, 1.0),
            None,
            "but not taken there"
        );
        assert_eq!(snapped(at(60.0), &inputs, Some(7), 1.0), None, "let go");
        // The distances are the screen's: zoomed out, as far on the canvas
        // is nearer on the screen.
        assert_eq!(snapped(at(40.0), &inputs, None, 0.5), Some(7));
        assert_eq!(snapped(at(100.0), &inputs, Some(7), 0.5), Some(7));
        assert_eq!(snapped(at(120.0), &inputs, Some(7), 0.5), None);
    }

    #[test]
    fn an_arrow_sticks_to_the_nearest_of_two_inputs() {
        let inputs = [(1, (0.0, 0.0)), (2, (30.0, 0.0))];
        assert_eq!(snapped((20.0, 0.0), &inputs, None, 1.0), Some(2));
        assert_eq!(snapped((10.0, 0.0), &inputs, None, 1.0), Some(1));
        assert_eq!(
            snapped((15.0, 0.0), &inputs, Some(1), 1.0),
            Some(1),
            "what it holds, it keeps against one as near"
        );
        assert_eq!(
            snapped((-40.0, 0.0), &inputs, Some(1), 1.0),
            Some(1),
            "and while near enough"
        );
        assert_eq!(
            snapped((20.0, 0.0), &inputs, Some(1), 1.0),
            Some(2),
            "a nearer input within reach takes it"
        );
    }

    #[test]
    fn zoomed_out_an_arrow_held_by_one_input_moves_to_the_next_one_under_the_pointer() {
        // Two steps stacked with a gap of 20: their inputs are 46 apart on
        // the screen at half size, closer than an arrow lets go.
        let (one, other) = (comes_in_at((260, 0)), comes_in_at((260, 92)));
        let inputs = [(1, one), (2, other)];
        assert_eq!(snapped(other, &inputs, Some(1), 0.5), Some(2));
        assert_eq!(snapped(one, &inputs, Some(2), 0.5), Some(1));
        let between = (one.0, (one.1 + other.1) / 2.0 - 4.0);
        assert_eq!(snapped(between, &inputs, Some(1), 0.5), Some(1));
    }

    #[test]
    fn an_arrow_sticks_only_to_a_step_it_can_lead_to() {
        use snob_core::automation::{Only, Webhook};
        let mut automation = Automation::new("a1", "Daily");
        let trigger = automation.insert(
            Step::Schedule {
                when: Spec::default(),
            },
            (0, 0),
        );
        let other = automation.insert(Step::Manual {}, (260, 0));
        let filter = automation.insert(
            Step::Filter {
                only: Only::Changes,
            },
            (260, 120),
        );
        let hook = automation.insert(Step::Webhook(Webhook::default()), (520, 0));
        let from_filter: Vec<NodeId> = inputs_for(&automation, filter)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(from_filter, [hook], "a filter leads to an output only");
        assert_eq!(
            inputs_for(&automation, filter)[0].1,
            comes_in_at((520, 0)),
            "at its input"
        );
        // A trigger leads to no other trigger, nor to a filter: right on
        // their inputs, nothing sticks.
        let from_trigger = inputs_for(&automation, trigger);
        assert!(from_trigger.is_empty(), "{from_trigger:?}");
        let on_the_other = comes_in_at((260, 0));
        assert_eq!(snapped(on_the_other, &from_trigger, None, 1.0), None);
        assert!(inputs_for(&automation, other).is_empty());
        assert!(inputs_for(&automation, 99).is_empty(), "no such step");
    }

    #[test]
    fn an_arrow_sticks_to_no_step_it_already_leads_to_nor_to_one_that_would_close_a_circle() {
        use snob_core::automation::Only;
        let mut automation = Automation::new("a1", "Daily");
        let filter = |automation: &mut Automation, y| {
            automation.insert(
                Step::Filter {
                    only: Only::Changes,
                },
                (260, y),
            )
        };
        let (first, second, third) = (
            filter(&mut automation, 0),
            filter(&mut automation, 120),
            filter(&mut automation, 240),
        );
        automation.connect(first, second).unwrap();
        let ids = |from| -> Vec<NodeId> {
            inputs_for(&automation, from)
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        };
        assert_eq!(ids(first), [third], "already led to: the second");
        assert_eq!(ids(second), [third], "back to the first: a circle");
        assert_eq!(ids(third), [first, second]);
        assert_eq!(
            snapped(
                comes_in_at((260, 120)),
                &inputs_for(&automation, first),
                None,
                1.0
            ),
            None
        );
    }

    fn angle(route: &[Bend]) -> f32 {
        let (x, y) = heading(route);
        y.atan2(x)
    }

    #[test]
    fn the_head_of_an_arrow_points_along_the_end_of_its_curve() {
        let from = leaves_at((0, 0));
        // Into an input, to the right, below or behind, the head points
        // into the step, as the arrow comes in from its left.
        for to in [(400, 0), (400, 300), (-300, 0), (-300, 300), (-300, -300)] {
            let route = route(from, comes_in_at(to), false);
            assert!(angle(&route).abs() < 1e-3, "{to:?}: {}", angle(&route));
            assert_eq!(route.last().unwrap()[3], comes_in_at(to), "it ends there");
        }
        // Loose, it turns toward the pointer: level to the right, down
        // below, back behind.
        let right = angle(&route(from, (from.0 + 300.0, from.1), true));
        assert!(right.abs() < 1e-3, "{right}");
        let below = angle(&route(from, (from.0 + 100.0, from.1 + 300.0), true));
        assert!(
            below > 0.5 && below < std::f32::consts::FRAC_PI_2 + 1e-3,
            "{below}"
        );
        let behind = angle(&route(from, (from.0 - 300.0, from.1), true));
        assert!(behind.abs() > std::f32::consts::FRAC_PI_2, "{behind}");

        // The head's tip is where it is put, its base behind it along the
        // way it points.
        let [tip, one, other] = head((10.0, 10.0), (0.0, 1.0), 4.0);
        assert_eq!(tip, (10.0, 10.0));
        assert!(near(one.1, 4.0) && near(other.1, 4.0), "{one:?} {other:?}");
        assert!(near((one.0 - other.0).abs(), 6.4));
        assert_eq!(heading(&[]), (1.0, 0.0));
        let still = [(5.0, 5.0); 4];
        assert_eq!(heading(&[still]), (1.0, 0.0), "no length: rightward");
    }

    #[test]
    fn a_curve_to_a_step_behind_its_start_goes_round_it() {
        // Every point of the way, sampled.
        let along = |route: &[Bend]| -> Vec<(f32, f32)> {
            route
                .iter()
                .flat_map(|[a, b, c, d]| {
                    (0..=40).map(move |at| {
                        let t = at as f32 / 40.0;
                        let u = 1.0 - t;
                        let mix = |p: f32, q: f32, r: f32, s: f32| {
                            u * u * u * p
                                + 3.0 * u * u * t * q
                                + 3.0 * u * t * t * r
                                + t * t * t * s
                        };
                        (mix(a.0, b.0, c.0, d.0), mix(a.1, b.1, c.1, d.1))
                    })
                })
                .collect()
        };
        let inside = |at: (f32, f32), step: (i32, i32)| {
            let (x, y) = (step.0 as f32, step.1 as f32);
            at.0 > x + 1.0 && at.0 < x + NODE_W - 1.0 && at.1 > y + 1.0 && at.1 < y + NODE_H - 1.0
        };
        let source = (0, 0);
        for target in [(-260, 0), (-230, 20), (-260, 200), (-100, -160), (100, 100)] {
            let way = route(leaves_at(source), comes_in_at(target), false);
            for at in along(&way) {
                assert!(
                    !inside(at, source) && !inside(at, target),
                    "{target:?}: through a step at {at:?}"
                );
            }
        }
        // In front, it is one curve.
        assert_eq!(
            route(leaves_at(source), comes_in_at((300, 40)), false).len(),
            1
        );
    }

    #[test]
    fn a_page_reads_its_runs_again_only_while_one_goes_where_it_is_seen_and_awake() {
        let going = Live {
            running: true,
            shown: true,
            ..Live::default()
        };
        assert!(reads_again(going));
        assert!(reads_again(Live {
            running: false,
            waiting: true,
            ..going
        }));
        assert!(!reads_again(Live {
            running: false,
            ..going
        }));
        assert!(!reads_again(Live {
            shown: false,
            ..going
        }));
        assert!(!reads_again(Live {
            resting: true,
            ..going
        }));
        assert!(!reads_again(Live {
            pinned: true,
            ..going
        }));
    }
}
