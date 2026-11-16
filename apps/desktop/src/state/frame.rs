//! The frame's two shapes and the way between them: **closed**, the panel
//! held by the frame on four sides, on the way in; **open**, once somebody
//! is signed in, the panel against the window's right and bottom with the
//! rail of sections at its left. A change of shape moves, from wherever the
//! frame is, and the rail's icons come in one after the other.
//!
//! **A function of the screen and of how long a change has gone on**, both
//! handed in: no clock here. Every move ends in a fixed state, so nothing
//! asks for a frame once it has.

use std::time::Duration;

use super::flow::Screen;
use crate::theme::motion;
use crate::theme::tokens::frame;

/// The frame's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// The panel held on four sides: on the way in, adding an account
    /// included.
    Closed,
    /// The panel against the window's right and bottom, the rail at its
    /// left.
    Open,
}

impl Shape {
    /// How open this shape is: 0 or 1.
    pub fn openness(self) -> f32 {
        match self {
            Self::Closed => 0.0,
            Self::Open => 1.0,
        }
    }
}

/// The shape `screen` is drawn in.
pub fn shape_of(screen: &Screen) -> Shape {
    if screen.signed_in() {
        Shape::Open
    } else {
        Shape::Closed
    }
}

/// How many sections the rail holds.
pub const SECTIONS: usize = 3;

/// One of the rail's icons, on its way in or out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Icon {
    /// From 0, not there, to 1, there.
    pub presence: f32,
    /// How far left of its place it is, in logical pixels.
    pub shift: f32,
    pub scale: f32,
}

/// The frame on its way from one openness to another, or at rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Morph {
    from: f32,
    to: f32,
    /// How far each icon had come in when the move began.
    icons_from: [f32; SECTIONS],
    lasts: Duration,
    /// Under "reduce motion" nothing moves: the frame is at its shape at
    /// once and the icons fade.
    reduce: bool,
}

impl Morph {
    /// The frame at rest in `shape`.
    pub fn still(shape: Shape) -> Self {
        let at = shape.openness();
        Self {
            from: at,
            to: at,
            icons_from: [at; SECTIONS],
            lasts: Duration::ZERO,
            reduce: false,
        }
    }

    /// Where this move is going.
    pub fn to(&self) -> Shape {
        if self.to > 0.5 {
            Shape::Open
        } else {
            Shape::Closed
        }
    }

    /// A move to `shape` from wherever this one is `elapsed` into it, in a
    /// time as much shorter as the way left is. Under `reduce`, the frame
    /// is at `shape` at once.
    #[must_use]
    pub fn toward(&self, shape: Shape, elapsed: Duration, reduce: bool) -> Self {
        let from = if reduce {
            shape.openness()
        } else {
            self.openness(elapsed)
        };
        let to = shape.openness();
        let whole = match shape {
            Shape::Open => motion::FRAME_OPEN,
            Shape::Closed => motion::FRAME_CLOSE,
        };
        Self {
            from,
            to,
            icons_from: std::array::from_fn(|at| self.icon(at, elapsed).presence),
            lasts: whole.mul_f32((to - from).abs()),
            reduce,
        }
    }

    /// How open the frame is `elapsed` into the move, from 0 to 1.
    pub fn openness(&self, elapsed: Duration) -> f32 {
        if self.lasts.is_zero() {
            return self.to;
        }
        let at = elapsed.as_secs_f32() / self.lasts.as_secs_f32();
        self.from + (self.to - self.from) * motion::ease_in_out(at)
    }

    /// The icon at `at` on the rail, `elapsed` into the move: one after the
    /// other on the way in, all together on the way out.
    pub fn icon(&self, at: usize, elapsed: Duration) -> Icon {
        let carried = self.icons_from.get(at).copied().unwrap_or(self.to);
        let presence = match (self.to() == Shape::Open, self.reduce) {
            (true, true) => carried.max(share(elapsed, Duration::ZERO, motion::FADE)),
            (true, false) => {
                let starts = motion::RAIL_ICON_DELAY + motion::RAIL_ICON_STAGGER * at as u32;
                let coming = motion::ease_out(share(elapsed, starts, motion::RAIL_ICON_ENTER));
                carried.max(coming)
            }
            (false, true) => 0.0,
            (false, false) => {
                carried * (1.0 - share(elapsed, Duration::ZERO, motion::RAIL_ICON_LEAVE))
            }
        };
        if self.reduce {
            Icon {
                presence,
                shift: 0.0,
                scale: 1.0,
            }
        } else {
            Icon {
                presence,
                shift: -6.0 * (1.0 - presence),
                scale: 0.85 + 0.15 * presence,
            }
        }
    }

    /// Whether anything still moves `elapsed` into the move.
    pub fn moving(&self, elapsed: Duration) -> bool {
        let icons = match (self.to() == Shape::Open, self.reduce) {
            (true, true) => motion::FADE,
            (true, false) => {
                motion::RAIL_ICON_DELAY
                    + motion::RAIL_ICON_STAGGER * (SECTIONS as u32 - 1)
                    + motion::RAIL_ICON_ENTER
            }
            (false, true) => Duration::ZERO,
            (false, false) => motion::RAIL_ICON_LEAVE,
        };
        let settled = self.icons_from.iter().all(|carried| *carried == self.to);
        elapsed < self.lasts || (!settled && elapsed < icons)
    }
}

/// How far `elapsed` is through a stretch of `lasts` that starts at
/// `starts`, from 0 to 1.
fn share(elapsed: Duration, starts: Duration, lasts: Duration) -> f32 {
    let into = elapsed.saturating_sub(starts);
    if lasts.is_zero() {
        return 1.0;
    }
    (into.as_secs_f32() / lasts.as_secs_f32()).min(1.0)
}

/// Where the panel is at an openness, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// The frame left of the panel: the rail's width once open.
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    /// The panel's corners: top left, top right, bottom right, bottom left.
    pub radii: [f32; 4],
}

/// Where the panel is at `openness`: held on four sides at 0, against the
/// window's right and bottom at 1, its top left corner round all the way.
pub fn geometry(openness: f32) -> Geometry {
    let open = openness.clamp(0.0, 1.0);
    let closed = 1.0 - open;
    let round = frame::PANEL_RADIUS * closed;
    Geometry {
        left: frame::SIDE + (frame::RAIL - frame::SIDE) * open,
        right: frame::SIDE * closed,
        bottom: frame::SIDE * closed,
        radii: [frame::PANEL_RADIUS, round, round, round],
    }
}

/// The selection on the rail, moving from one section to another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pill {
    /// Where it is, in rows from the first section.
    pub row: f32,
    /// From 0, not there, to 1, there.
    pub presence: f32,
}

/// The selection `elapsed` after it left `from` for `to`: it slides between
/// two sections, and comes or goes where there is none at one end.
pub fn pill(
    from: Option<usize>,
    to: Option<usize>,
    elapsed: Duration,
    reduce: bool,
) -> Option<Pill> {
    let lasts = if reduce {
        motion::FADE
    } else {
        motion::INTERACTION
    };
    let at = motion::ease_out(share(elapsed, Duration::ZERO, lasts));
    match (from, to) {
        (Some(from), Some(to)) if !reduce => Some(Pill {
            row: from as f32 + (to as f32 - from as f32) * at,
            presence: 1.0,
        }),
        (_, Some(to)) if from == Some(to) || from.is_some() => Some(Pill {
            row: to as f32,
            presence: 1.0,
        }),
        (None, Some(to)) => Some(Pill {
            row: to as f32,
            presence: at,
        }),
        (Some(from), None) if at < 1.0 => Some(Pill {
            row: from as f32,
            presence: 1.0 - at,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::flow::Method;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn the_frame_is_open_where_somebody_is_signed_in_and_closed_on_the_way_in() {
        for screen in [
            Screen::Home,
            Screen::Search,
            Screen::Automations,
            Screen::Settings,
        ] {
            assert_eq!(shape_of(&screen), Shape::Open, "{screen:?}");
        }
        for screen in [
            Screen::Splash,
            Screen::Start {
                expired: None,
                back: false,
            },
            // Adding an account closes it.
            Screen::Start {
                expired: None,
                back: true,
            },
            Screen::Browser,
            Screen::Paste,
            Screen::Updating {
                then: Method::Paste,
                at: crate::state::flow::Updating::Checking,
            },
        ] {
            assert_eq!(shape_of(&screen), Shape::Closed, "{screen:?}");
        }
    }

    #[test]
    fn the_frame_opens_only_forward_and_ends_open() {
        let morph = Morph::still(Shape::Closed).toward(Shape::Open, Duration::ZERO, false);
        let mut before = 0.0;
        for step in 0..=60 {
            let now = morph.openness(MS(step * 10));
            assert!(now >= before, "never backward: {now} after {before}");
            before = now;
        }
        assert_eq!(morph.openness(motion::FRAME_OPEN), 1.0);
        assert_eq!(morph.openness(MS(10_000)), 1.0);
        assert!(morph.moving(MS(500)));
        assert!(!morph.moving(MS(2_000)), "and then asks for nothing");
    }

    #[test]
    fn a_move_interrupted_half_way_goes_back_from_where_it_was() {
        let opening = Morph::still(Shape::Closed).toward(Shape::Open, Duration::ZERO, false);
        let half = opening.openness(motion::FRAME_OPEN / 2);
        assert!((0.3..0.7).contains(&half), "{half}");

        let closing = opening.toward(Shape::Closed, motion::FRAME_OPEN / 2, false);
        assert!(
            (closing.openness(Duration::ZERO) - half).abs() < 1e-6,
            "no cut"
        );
        assert!(
            closing.lasts < motion::FRAME_CLOSE,
            "a shorter way takes less time"
        );
        assert_eq!(closing.openness(motion::FRAME_CLOSE), 0.0);
        assert_eq!(closing.to(), Shape::Closed);
    }

    #[test]
    fn the_icons_come_in_one_after_the_other_and_leave_together() {
        let opening = Morph::still(Shape::Closed).toward(Shape::Open, Duration::ZERO, false);
        let at = |elapsed| {
            let icons: Vec<f32> = (0..SECTIONS)
                .map(|at| opening.icon(at, elapsed).presence)
                .collect();
            icons
        };
        assert_eq!(at(MS(100)), [0.0; SECTIONS], "not before the frame moved");
        let staggered = at(MS(400));
        assert!(staggered[0] > staggered[1] && staggered[1] > staggered[2]);
        assert_eq!(at(MS(700)), [1.0; SECTIONS]);
        let first = opening.icon(0, MS(250));
        assert!(first.shift < 0.0 && first.scale < 1.0, "{first:?}");
        assert_eq!(
            opening.icon(0, MS(700)),
            Icon {
                presence: 1.0,
                shift: 0.0,
                scale: 1.0
            }
        );

        let closing = Morph::still(Shape::Open).toward(Shape::Closed, Duration::ZERO, false);
        let leaving: Vec<f32> = (0..SECTIONS)
            .map(|at| closing.icon(at, motion::RAIL_ICON_LEAVE / 2).presence)
            .collect();
        assert!(
            leaving
                .iter()
                .all(|presence| (*presence - 0.5).abs() < 1e-3)
        );
        assert_eq!(closing.icon(2, motion::RAIL_ICON_LEAVE).presence, 0.0);
    }

    #[test]
    fn with_less_motion_the_frame_changes_at_once_and_the_icons_only_fade() {
        let opening = Morph::still(Shape::Closed).toward(Shape::Open, Duration::ZERO, true);
        assert_eq!(opening.openness(Duration::ZERO), 1.0);
        let icon = opening.icon(2, motion::FADE / 2);
        assert!((icon.presence - 0.5).abs() < 1e-3);
        assert_eq!((icon.shift, icon.scale), (0.0, 1.0));
        assert!(!opening.moving(motion::FADE));

        let closing = Morph::still(Shape::Open).toward(Shape::Closed, Duration::ZERO, true);
        assert_eq!(closing.openness(Duration::ZERO), 0.0);
        assert_eq!(closing.icon(0, Duration::ZERO).presence, 0.0);
    }

    #[test]
    fn a_frame_at_rest_asks_for_nothing() {
        for shape in [Shape::Closed, Shape::Open] {
            let still = Morph::still(shape);
            assert!(!still.moving(Duration::ZERO));
            assert_eq!(still.openness(Duration::ZERO), shape.openness());
            assert_eq!(
                still.toward(shape, Duration::ZERO, false).lasts,
                Duration::ZERO
            );
        }
    }

    #[test]
    fn open_the_panel_meets_the_window_and_leaves_the_rail_its_width() {
        let closed = geometry(0.0);
        assert_eq!(
            (closed.left, closed.right, closed.bottom),
            (frame::SIDE, frame::SIDE, frame::SIDE)
        );
        assert_eq!(closed.radii, [frame::PANEL_RADIUS; 4]);
        let open = geometry(1.0);
        assert_eq!(
            (open.left, open.right, open.bottom),
            (frame::RAIL, 0.0, 0.0)
        );
        let half = geometry(0.5);
        assert!(half.left > frame::SIDE && half.left < frame::RAIL);
        assert_eq!(open.radii, [frame::PANEL_RADIUS, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn the_selection_slides_between_sections_and_comes_where_there_was_none() {
        let half = motion::INTERACTION / 2;
        let sliding = pill(Some(0), Some(2), half, false).unwrap();
        assert!(sliding.row > 0.0 && sliding.row < 2.0 && sliding.presence == 1.0);
        assert_eq!(
            pill(Some(0), Some(2), motion::INTERACTION, false),
            Some(Pill {
                row: 2.0,
                presence: 1.0
            })
        );
        assert_eq!(
            pill(Some(0), Some(2), Duration::ZERO, true),
            Some(Pill {
                row: 2.0,
                presence: 1.0
            }),
            "with less motion it does not slide"
        );
        let coming = pill(None, Some(1), half, false).unwrap();
        assert!(coming.presence > 0.0 && coming.presence < 1.0);
        assert_eq!(pill(Some(1), None, motion::INTERACTION, false), None);
        assert_eq!(pill(None, None, Duration::ZERO, false), None);
    }
}
