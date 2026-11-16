//! How long things move, and how: short, ending slowly, and replaced by a
//! brief fade where the system asks for less motion. **Every animation ends
//! in a fixed state**; nothing loops but the loading ring, which says that
//! something is going on and keeps turning either way.

use std::time::Duration;

/// A change that answers a pointer or a key: a hover, a press, a focus.
pub const INTERACTION: Duration = Duration::from_millis(180);

/// Something coming into view.
pub const ENTER: Duration = Duration::from_millis(300);

/// Something leaving it.
pub const EXIT: Duration = Duration::from_millis(250);

/// The fade that stands in for any of them under "reduce motion".
pub const FADE: Duration = Duration::from_millis(120);

/// The least the splash stays, so a start that takes no time does not flash.
pub const SPLASH_AT_LEAST: Duration = Duration::from_millis(400);

/// How the welcome comes in: slower than another screen, since it is
/// looked at rather than used. A [`FADE`] under "reduce motion", like any
/// other move.
pub const WELCOME_ENTER: Duration = Duration::from_millis(600);

/// How long the welcome stays, from when it starts coming in, before the
/// home screen takes its place. **The same under "reduce motion"**: it is
/// time to read the name, not a move. 1.2 s was reported as too short.
pub const WELCOME_STAYS: Duration = Duration::from_millis(2800);

/// One turn of the loading ring.
/// How long a picture takes to come in once it is read, when it was not
/// there at once: a fade and a small growth into its circle.
pub const PICTURE_ARRIVES: Duration = Duration::from_millis(260);

pub const RING_TURN: Duration = Duration::from_millis(1100);

/// What a transition does, beside taking time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Move {
    pub lasts: Duration,
    /// How far it travels, in logical pixels: none under "reduce motion".
    pub travels: f32,
    /// The scale it starts or ends at beside 1: 1 under "reduce motion".
    pub scales: f32,
}

/// The move asked for, or the fade that replaces it when `reduce` is on.
pub fn moved(reduce: bool, lasts: Duration, travels: f32, scales: f32) -> Move {
    if reduce {
        Move {
            lasts: FADE,
            travels: 0.0,
            scales: 1.0,
        }
    } else {
        Move {
            lasts,
            travels,
            scales,
        }
    }
}

/// A curve that brakes at its end: the cubic ease-out, from 0 to 1.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_durations_are_inside_what_the_design_allows() {
        assert!((150..=200).contains(&INTERACTION.as_millis()));
        for lasts in [ENTER, EXIT] {
            assert!((250..=350).contains(&lasts.as_millis()));
        }
        assert!(FADE < INTERACTION);
    }

    #[test]
    fn the_welcome_is_on_screen_long_enough_to_read_once_it_has_come_in() {
        assert!(WELCOME_ENTER > ENTER);
        let read = WELCOME_STAYS - WELCOME_ENTER;
        assert!(read >= Duration::from_secs(2), "{read:?} to read the name");
    }

    #[test]
    fn the_curve_starts_fast_and_brakes_at_its_end() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(2.0), 1.0);
        let first_half = ease_out(0.5);
        assert!(first_half > 0.8, "most of the way in half the time");
        let mut before = 0.0;
        for step in 1..=20 {
            let now = ease_out(step as f32 / 20.0);
            assert!(now > before, "never backward");
            before = now;
        }
    }

    #[test]
    fn less_motion_is_a_short_fade_that_goes_nowhere() {
        let usual = moved(false, ENTER, 16.0, 0.96);
        assert_eq!(
            (usual.lasts, usual.travels, usual.scales),
            (ENTER, 16.0, 0.96)
        );
        let reduced = moved(true, ENTER, 16.0, 0.96);
        assert_eq!(
            (reduced.lasts, reduced.travels, reduced.scales),
            (FADE, 0.0, 1.0)
        );
    }
}
