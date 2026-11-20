//! A profile page and the lists it opens, apart from how they are drawn: how
//! the page is laid out and what it shows of the posts, what marks the
//! account, what a click on the picture does, how the picture goes from the
//! page into its viewer and back and how far it is zoomed and moved there,
//! what "About this account" holds, and **when a list may be read without
//! asking**: only the account's own (AGENTS.md, "Consent").
//!
//! No window, no clock and no engine here.

use std::time::Duration;

use snob_core::Pk;

use crate::theme::motion::{self, Curve};

/// How the page is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// The card on top and the posts under it, the whole page scrolling: as
    /// Instagram lays a profile out.
    #[default]
    Stacked,
    /// The card standing at the left, narrower, and the posts at its right,
    /// which alone scroll.
    Split,
}

impl Layout {
    /// The other one: what the layout button changes to.
    pub fn other(self) -> Self {
        match self {
            Self::Stacked => Self::Split,
            Self::Split => Self::Stacked,
        }
    }
}

/// How the posts of a tab are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Showing {
    #[default]
    Grid,
    List,
}

/// The tabs under a profile's card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tab {
    /// Its own posts, counted by the profile.
    #[default]
    Posts,
    /// Other accounts' posts it shared.
    Reposts,
    /// The posts it is tagged in.
    Tagged,
}

impl Tab {
    /// In the order they are shown.
    pub const ALL: [Self; 3] = [Self::Posts, Self::Reposts, Self::Tagged];
}

/// One of the two lists of an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Followers,
    Following,
}

/// Whether reading `owner`'s list, signed in as `viewer`, asks first: **every
/// list but the viewer's own**, and before anything of it is sent.
pub fn asks_first(owner: Pk, viewer: Pk) -> bool {
    owner != viewer
}

/// What a click on the picture does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureClick {
    /// Opens the picture large.
    Open,
    /// A story is up: asks which, the story or the picture.
    Choose,
}

/// What a click on the picture of an account does, `story_up` or not.
/// **Knowing a story is up is all that is read of it**: nothing is opened
/// and nothing is marked as seen.
pub fn picture_click(story_up: bool) -> PictureClick {
    if story_up {
        PictureClick::Choose
    } else {
        PictureClick::Open
    }
}

/// How far the picture in its viewer is zoomed and moved: at a scale of 1 it
/// fits the window and stays in its middle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    pub scale: f32,
    /// How far its middle is from the window's, in logical pixels.
    pub x: f32,
    pub y: f32,
}

impl Default for Zoom {
    fn default() -> Self {
        Self::FIT
    }
}

impl Zoom {
    /// The picture as it opens: fitted, in the middle.
    pub const FIT: Self = Self {
        scale: 1.0,
        x: 0.0,
        y: 0.0,
    };

    /// The most it is zoomed in.
    pub const MOST: f32 = 8.0;

    /// How much one notch of the wheel zooms.
    const NOTCH: f32 = 1.2;

    /// Whether it is zoomed in, and so can be moved.
    pub fn zoomed(&self) -> bool {
        self.scale > 1.0 + 1e-3
    }

    /// Zoomed by `notches` of the wheel (positive in, negative out) about
    /// `at`, the pointer's place from the window's middle, the picture
    /// drawn `size` large when it fits: what is under the pointer stays
    /// under it.
    #[must_use]
    pub fn wheeled(self, notches: f32, at: (f32, f32), size: (f32, f32)) -> Self {
        if !notches.is_finite() {
            return self;
        }
        let scale = (self.scale * Self::NOTCH.powf(notches)).clamp(1.0, Self::MOST);
        // The point of the picture under the pointer, at the scale it was.
        let (px, py) = ((at.0 - self.x) / self.scale, (at.1 - self.y) / self.scale);
        Self {
            scale,
            x: at.0 - px * scale,
            y: at.1 - py * scale,
        }
        .held(size)
    }

    /// Moved by a drag of `by`, only while it is zoomed in, and never so far
    /// that the picture leaves its place.
    #[must_use]
    pub fn dragged(self, by: (f32, f32), size: (f32, f32)) -> Self {
        if !self.zoomed() || !by.0.is_finite() || !by.1.is_finite() {
            return self;
        }
        Self {
            x: self.x + by.0,
            y: self.y + by.1,
            ..self
        }
        .held(size)
    }

    /// Kept where the picture still covers the place it fits in.
    fn held(self, size: (f32, f32)) -> Self {
        let room = |side: f32| ((side * self.scale - side) / 2.0).max(0.0);
        Self {
            x: self.x.clamp(-room(size.0), room(size.0)),
            y: self.y.clamp(-room(size.1), room(size.1)),
            ..self
        }
    }
}

/// A rectangle in logical pixels, from the window's top left corner.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Smaller by `by` on every side, never under nothing.
    #[must_use]
    pub fn inset(self, by: f32) -> Self {
        let w = (self.w - 2.0 * by).max(0.0);
        let h = (self.h - 2.0 * by).max(0.0);
        Self {
            x: self.x + (self.w - w) / 2.0,
            y: self.y + (self.h - h) / 2.0,
            w,
            h,
        }
    }

    /// Whether it is something, and all of it inside `other`.
    pub fn inside(self, other: Self) -> bool {
        let finite = [self.x, self.y, self.w, self.h]
            .iter()
            .all(|side| side.is_finite());
        finite
            && self.w > 0.0
            && self.h > 0.0
            && self.x >= other.x
            && self.y >= other.y
            && self.x + self.w <= other.x + other.w
            && self.y + self.h <= other.y + other.h
    }

    /// `t` of the way from this one to `to`, each exactly at its end.
    fn toward(self, to: Self, t: f32) -> Self {
        Self {
            x: between(self.x, to.x, t),
            y: between(self.y, to.y, t),
            w: between(self.w, to.w, t),
            h: between(self.h, to.h, t),
        }
    }
}

/// `t` of the way from `a` to `b`: `a` at 0 and `b` at 1, exactly.
fn between(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

/// The corners of the picture where it fits in its viewer.
pub const VIEWER_CORNER: f32 = 10.0;

/// How much of the window's shorter side the picture takes when it fits.
const FITS: f32 = 0.72;

/// The largest the picture is drawn when it fits, in logical pixels.
const FITS_AT_MOST: f32 = 720.0;

/// Where the picture fits in its viewer, in a window `width` by `height`
/// under a band `band` high: a square in its middle.
pub fn viewer_place(width: f32, height: f32, band: f32) -> Rect {
    let side = (width.min(height - band) * FITS).clamp(80.0, FITS_AT_MOST);
    Rect {
        x: (width - side) / 2.0,
        y: (height - side) / 2.0,
        w: side,
        h: side,
    }
}

/// How the picture goes between its place on the page and its viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// It moves and grows from where the page draws it, its corners going
    /// from a circle's to the viewer's: the page's own is not drawn
    /// meanwhile, so it is never seen twice.
    Moves,
    /// It fades where it is, the page's own left as it is.
    Fades,
}

/// How the picture goes, its place on the page being `page` (nothing when
/// it was not drawn), `seen` what of the window shows the page and `clip`
/// what the page's scrolling leaves of it where the picture is: **it moves
/// only from and to a place all in sight**, and never under "reduce
/// motion".
pub fn way(page: Rect, seen: Rect, clip: Rect, reduce_motion: bool) -> Way {
    if !reduce_motion && page.inside(seen) && page.inside(clip) {
        Way::Moves
    } else {
        Way::Fades
    }
}

/// One run of the picture along its way, measured from 0 where the page
/// draws it to 1 where it fits in its viewer: from where the run started,
/// toward the viewer or back to the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    from: f32,
    opening: bool,
}

impl Run {
    /// From the page into the viewer, the whole way.
    pub const OPENING: Self = Self {
        from: 0.0,
        opening: true,
    };

    /// Whether it goes toward the viewer.
    pub fn opening(self) -> bool {
        self.opening
    }

    /// How long it lasts, the whole way lasting `whole`: as long as the
    /// share of the way it has left, so a run turned back near its start
    /// is short.
    pub fn lasts(self, whole: Duration) -> Duration {
        whole.mul_f32(self.left_at_start())
    }

    /// Where it is, `elapsed` into it, the whole way lasting `whole`: its
    /// start at once, its end exactly once it lasted long enough, and in
    /// between as `curve` eases.
    pub fn at(self, elapsed: Duration, whole: Duration, curve: Curve) -> f32 {
        let eased = curve.at(motion::progress(elapsed, self.lasts(whole)));
        let end = if self.opening { 1.0 } else { 0.0 };
        between(self.from, end, eased)
    }

    /// Whether it has arrived, `elapsed` into it.
    pub fn arrived(self, elapsed: Duration, whole: Duration) -> bool {
        motion::progress(elapsed, self.lasts(whole)) >= 1.0
    }

    /// The run the other way, from `at`, where this one is now: **it
    /// continues from there**, never from an end.
    #[must_use]
    pub fn turned(self, at: f32) -> Self {
        Self {
            from: if at.is_finite() {
                at.clamp(0.0, 1.0)
            } else {
                self.from
            },
            opening: !self.opening,
        }
    }

    /// How much of its own way it still has to go at `at`: 1 where it
    /// started and 0 at its end. What a picture that fades back keeps of
    /// how much it was shown when it turned.
    pub fn left(self, at: f32) -> f32 {
        let total = self.left_at_start();
        if total <= 0.0 {
            return 0.0;
        }
        let to_go = if self.opening { 1.0 - at } else { at };
        if to_go.is_nan() {
            return 0.0;
        }
        (to_go / total).clamp(0.0, 1.0)
    }

    /// The share of the whole way it has to go from its start.
    fn left_at_start(self) -> f32 {
        let from = if self.from.is_finite() {
            self.from.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if self.opening { 1.0 - from } else { from }
    }
}

/// Where the picture is drawn and how round its corners are, `at` of its
/// way from `page`, where it is a circle, to `viewer`, where its corners
/// are `viewer_corner`: each end exactly.
pub fn moved_picture(page: Rect, viewer: Rect, viewer_corner: f32, at: f32) -> (Rect, f32) {
    let at = if at.is_finite() {
        at.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let round = page.w.min(page.h) / 2.0;
    (page.toward(viewer, at), between(round, viewer_corner, at))
}

/// What marks an account beside its names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Verified,
    Private,
    /// Each follows the other.
    Friend,
    /// It follows the viewer, who is not known to follow it.
    FollowsYou,
}

/// The marks of an account, in the order they are shown, from what its
/// profile says: **a friend is one both follow**, and is marked as that
/// alone. What is not known marks nothing.
pub fn marks(
    verified: bool,
    private: bool,
    you_follow: Option<bool>,
    follows_you: Option<bool>,
) -> Vec<Mark> {
    let follows_you = follows_you == Some(true);
    let friend = follows_you && you_follow == Some(true);
    [
        verified.then_some(Mark::Verified),
        private.then_some(Mark::Private),
        friend.then_some(Mark::Friend),
        (follows_you && !friend).then_some(Mark::FollowsYou),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// What "About this account" tells of an account, as Instagram's own app
/// shows it. **The engine reads none of it yet**: each stays `None` until it
/// does, and a row that is `None` says it is not read. Nothing is made up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct About {
    pub joined: Option<Joined>,
    pub based_in: Option<String>,
    pub username_changes: Option<u64>,
}

/// The month an account joined in, as Instagram's app says it: no day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Joined {
    pub year: i32,
    /// 1 to 12.
    pub month: u32,
}

/// A row of "About this account".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AboutRow {
    Joined,
    BasedIn,
    UsernameChanges,
}

impl AboutRow {
    /// The rows in the order they are shown.
    pub const ALL: [Self; 3] = [Self::Joined, Self::BasedIn, Self::UsernameChanges];
}

/// How far each of the words naming two views is shown, `t` of the way from
/// the one left to the one chosen: the first leaves over the first half and
/// the second comes over the second, so they are never read over each other.
pub fn crossing(t: f32) -> (f32, f32) {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        1.0
    };
    ((1.0 - 2.0 * t).max(0.0), (2.0 * t - 1.0).max(0.0))
}

/// Where a list's reading stands, as a share from 0 to 1 of what the
/// account says it holds: none while that is not known.
pub fn share_read(read: usize, expected: Option<u64>) -> Option<f32> {
    let expected = expected.filter(|expected| *expected > 0)?;
    Some((read as f64 / expected as f64).min(1.0) as f32)
}

/// How many columns the grid of posts takes in `width` logical pixels: three
/// as Instagram has them, more once each would be wider than `widest`.
pub fn columns(width: f32, widest: f32) -> usize {
    if !width.is_finite() || !widest.is_finite() || widest <= 0.0 {
        return 3;
    }
    ((width / widest).ceil() as usize).clamp(3, 6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_viewer_s_own_lists_are_read_without_asking() {
        let me = Pk::new(42);
        assert!(!asks_first(me, me));
        assert!(asks_first(Pk::new(9001), me));
    }

    #[test]
    fn a_story_up_asks_which_and_otherwise_the_picture_opens() {
        assert_eq!(picture_click(false), PictureClick::Open);
        assert_eq!(picture_click(true), PictureClick::Choose);
    }

    #[test]
    fn the_layout_button_goes_from_one_layout_to_the_other_and_back() {
        assert_eq!(Layout::default(), Layout::Stacked);
        assert_eq!(Layout::Stacked.other(), Layout::Split);
        assert_eq!(Layout::Stacked.other().other(), Layout::Stacked);
    }

    #[test]
    fn the_wheel_zooms_about_the_pointer_within_its_bounds() {
        let size = (400.0, 400.0);
        let fit = Zoom::FIT;
        assert!(!fit.zoomed());
        // In, about the middle: it stays in the middle.
        let once = fit.wheeled(1.0, (0.0, 0.0), size);
        assert!((once.scale - 1.2).abs() < 1e-4 && once.x == 0.0 && once.y == 0.0);
        // About a corner: what was under the pointer stays under it.
        let at = (100.0, -50.0);
        let near = fit.wheeled(2.0, at, size);
        let under = ((at.0 - near.x) / near.scale, (at.1 - near.y) / near.scale);
        assert!((under.0 - 100.0).abs() < 1e-3 && (under.1 + 50.0).abs() < 1e-3);
        // Never past the most, never under the fit, and back to the middle.
        assert_eq!(fit.wheeled(100.0, at, size).scale, Zoom::MOST);
        let out = near.wheeled(-100.0, at, size);
        assert_eq!(out, Zoom::FIT);
        assert_eq!(fit.wheeled(f32::NAN, at, size), fit);
    }

    #[test]
    fn a_drag_moves_the_picture_only_when_zoomed_and_never_off_its_place() {
        let size = (400.0, 300.0);
        assert_eq!(Zoom::FIT.dragged((50.0, 50.0), size), Zoom::FIT);
        let twice = Zoom {
            scale: 2.0,
            ..Zoom::FIT
        };
        let moved = twice.dragged((50.0, -20.0), size);
        assert_eq!((moved.x, moved.y), (50.0, -20.0));
        // Half the picture's growth on each side is as far as it goes.
        let far = twice.dragged((10_000.0, -10_000.0), size);
        assert_eq!((far.x, far.y), (200.0, -150.0));
    }

    #[test]
    fn what_was_read_of_a_list_is_a_share_of_what_it_says_it_holds() {
        assert_eq!(share_read(0, None), None);
        assert_eq!(share_read(5, Some(0)), None);
        assert_eq!(share_read(25, Some(100)), Some(0.25));
        // Instagram's count can be under what a walk reads.
        assert_eq!(share_read(130, Some(100)), Some(1.0));
    }

    #[test]
    fn friends_are_marked_friend_and_not_follows_you() {
        assert_eq!(marks(false, false, Some(true), Some(true)), [Mark::Friend]);
        assert_eq!(
            marks(false, false, Some(false), Some(true)),
            [Mark::FollowsYou]
        );
        assert_eq!(marks(false, false, None, Some(true)), [Mark::FollowsYou]);
        assert_eq!(marks(false, false, Some(true), Some(false)), []);
        assert_eq!(marks(false, false, None, None), []);
        assert_eq!(
            marks(true, true, Some(true), Some(true)),
            [Mark::Verified, Mark::Private, Mark::Friend]
        );
    }

    #[test]
    fn the_name_of_one_view_leaves_before_the_other_comes() {
        assert_eq!(crossing(0.0), (1.0, 0.0));
        assert_eq!(crossing(0.5), (0.0, 0.0));
        assert_eq!(crossing(1.0), (0.0, 1.0));
        for step in 0..=20 {
            let (leaving, coming) = crossing(step as f32 / 20.0);
            assert!(
                leaving == 0.0 || coming == 0.0,
                "both shown at {step}: {leaving}, {coming}"
            );
        }
        assert_eq!(crossing(f32::NAN), (0.0, 1.0));
        assert_eq!(crossing(7.0), (0.0, 1.0));
    }

    const PAGE: Rect = Rect {
        x: 120.0,
        y: 90.0,
        w: 132.0,
        h: 132.0,
    };

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_picture_moves_from_where_the_page_drew_it_to_where_it_fits_each_end_exactly() {
        let viewer = viewer_place(1200.0, 800.0, 42.0);
        assert_eq!(
            moved_picture(PAGE, viewer, VIEWER_CORNER, 0.0),
            (PAGE, 66.0)
        );
        assert_eq!(
            moved_picture(PAGE, viewer, VIEWER_CORNER, 1.0),
            (viewer, VIEWER_CORNER)
        );
        let (half, corner) = moved_picture(PAGE, viewer, VIEWER_CORNER, 0.5);
        assert!(PAGE.x < half.x && half.x < viewer.x, "{half:?}");
        assert!(PAGE.w < half.w && half.w < viewer.w, "{half:?}");
        assert!(VIEWER_CORNER < corner && corner < 66.0, "{corner}");
        // Nothing past its ends, and a broken share is the viewer's place.
        assert_eq!(moved_picture(PAGE, viewer, VIEWER_CORNER, -2.0).0, PAGE);
        assert_eq!(moved_picture(PAGE, viewer, VIEWER_CORNER, 3.0).0, viewer);
        assert_eq!(
            moved_picture(PAGE, viewer, VIEWER_CORNER, f32::NAN).0,
            viewer
        );
    }

    #[test]
    fn the_picture_fits_as_a_square_in_the_middle_of_the_window() {
        let place = viewer_place(1200.0, 800.0, 42.0);
        assert_eq!(place.w, place.h);
        assert!((place.w - (800.0 - 42.0) * 0.72).abs() < 1e-3);
        assert!((place.x + place.w / 2.0 - 600.0).abs() < 1e-3);
        assert!((place.y + place.h / 2.0 - 400.0).abs() < 1e-3);
        assert_eq!(viewer_place(4000.0, 4000.0, 42.0).w, FITS_AT_MOST);
        assert_eq!(viewer_place(60.0, 60.0, 42.0).w, 80.0);
    }

    #[test]
    fn a_run_starts_where_it_is_and_ends_exactly_at_its_end() {
        let whole = ms(300);
        let open = Run::OPENING;
        assert!(open.opening());
        assert_eq!(open.at(Duration::ZERO, whole, motion::STANDARD), 0.0);
        assert_eq!(open.at(whole, whole, motion::STANDARD), 1.0);
        assert_eq!(open.at(ms(9_000), whole, motion::STANDARD), 1.0);
        assert!(!open.arrived(ms(299), whole) && open.arrived(whole, whole));
        let back = open.turned(1.0);
        assert!(!back.opening());
        assert_eq!(back.at(Duration::ZERO, ms(250), motion::STANDARD), 1.0);
        assert_eq!(back.at(ms(250), ms(250), motion::STANDARD), 0.0);
        let mut before = 1.0;
        for step in 1..=25 {
            let now = back.at(ms(step * 10), ms(250), motion::STANDARD);
            assert!(now <= before, "never back toward the viewer");
            before = now;
        }
    }

    #[test]
    fn a_run_turned_back_continues_from_where_it_was_and_takes_only_its_share_of_the_time() {
        let (enter, exit) = (ms(300), ms(250));
        let open = Run::OPENING;
        let at = open.at(ms(60), enter, motion::STANDARD);
        assert!(0.0 < at && at < 1.0, "{at}");
        let back = open.turned(at);
        assert_eq!(back.at(Duration::ZERO, exit, motion::STANDARD), at);
        assert_eq!(back.lasts(exit), exit.mul_f32(at));
        assert_eq!(back.at(back.lasts(exit), exit, motion::STANDARD), 0.0);
        // Turned again on its way back, it goes on from there to the end.
        let later = back.at(ms(20), exit, motion::STANDARD);
        let again = back.turned(later);
        assert!(again.opening());
        assert_eq!(again.at(Duration::ZERO, enter, motion::STANDARD), later);
        assert_eq!(again.at(enter, enter, motion::STANDARD), 1.0);
        // A run turned at its very end lasts nothing and stays there.
        let done = open.turned(0.0);
        assert_eq!(done.lasts(exit), Duration::ZERO);
        assert_eq!(done.at(Duration::ZERO, exit, motion::STANDARD), 0.0);
        assert_eq!(
            open.turned(f32::NAN),
            Run {
                from: 0.0,
                opening: false
            }
        );
    }

    #[test]
    fn what_is_left_of_a_run_is_all_of_it_at_its_start_and_none_at_its_end() {
        let back = Run::OPENING.turned(0.6);
        assert_eq!(back.left(0.6), 1.0);
        assert!((back.left(0.3) - 0.5).abs() < 1e-6);
        assert_eq!(back.left(0.0), 0.0);
        assert_eq!(back.left(f32::NAN), 0.0);
        let open = Run::OPENING;
        assert_eq!(open.left(0.0), 1.0);
        assert_eq!(open.left(1.0), 0.0);
        assert_eq!(Run::OPENING.turned(0.0).left(0.0), 0.0);
    }

    #[test]
    fn a_picture_out_of_sight_or_scrolled_out_of_its_card_fades_and_so_does_any_under_less_motion()
    {
        let seen = Rect {
            x: 0.0,
            y: 42.0,
            w: 1200.0,
            h: 758.0,
        };
        assert_eq!(way(PAGE, seen, seen, false), Way::Moves);
        assert_eq!(way(PAGE, seen, seen, true), Way::Fades);
        // Scrolled under the band, past the window's foot, or never drawn.
        let under_the_band = Rect { y: 10.0, ..PAGE };
        assert_eq!(way(under_the_band, seen, seen, false), Way::Fades);
        let below = Rect { y: 790.0, ..PAGE };
        assert_eq!(way(below, seen, seen, false), Way::Fades);
        assert_eq!(way(Rect::default(), seen, seen, false), Way::Fades);
        let broken = Rect {
            x: f32::NAN,
            ..PAGE
        };
        assert_eq!(way(broken, seen, seen, false), Way::Fades);
        // In the window, but partly scrolled out of the card it is in.
        let card = Rect {
            x: 0.0,
            y: 100.0,
            w: 400.0,
            h: 700.0,
        };
        assert_eq!(way(PAGE, seen, card, false), Way::Fades);
        assert_eq!(
            way(Rect { y: 120.0, ..PAGE }, seen, card, false),
            Way::Moves
        );
    }

    #[test]
    fn a_rectangle_inset_keeps_its_middle_and_never_turns_inside_out() {
        let inset = PAGE.inset(6.0);
        assert_eq!(
            inset,
            Rect {
                x: 126.0,
                y: 96.0,
                w: 120.0,
                h: 120.0
            }
        );
        let gone = PAGE.inset(500.0);
        assert_eq!((gone.w, gone.h), (0.0, 0.0));
        assert_eq!(gone.x, PAGE.x + PAGE.w / 2.0);
    }

    #[test]
    fn the_grid_has_three_columns_and_more_on_a_wide_page() {
        assert_eq!(columns(600.0, 260.0), 3);
        assert_eq!(columns(1100.0, 260.0), 5);
        assert_eq!(columns(5000.0, 260.0), 6);
        assert_eq!(columns(f32::NAN, 260.0), 3);
    }
}
