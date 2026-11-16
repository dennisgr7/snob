//! A profile page and the lists it opens, apart from how they are drawn: how
//! the page is laid out and what it shows of the posts, what a click on the
//! picture does, how far the picture is zoomed and moved in its viewer, and
//! **when a list may be read without asking**: only the account's own
//! (AGENTS.md, "Consent").
//!
//! No window, no clock and no engine here.

use snob_core::Pk;

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
    fn the_grid_has_three_columns_and_more_on_a_wide_page() {
        assert_eq!(columns(600.0, 260.0), 3);
        assert_eq!(columns(1100.0, 260.0), 5);
        assert_eq!(columns(5000.0, 260.0), 6);
        assert_eq!(columns(f32::NAN, 260.0), 3);
    }
}
