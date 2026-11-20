//! How often the window is drawn while something moves: the intervals
//! between frames each asked for by the one before, kept while every frame
//! asks for the next, and summed up in a [`Cadence`] at the first frame
//! nobody asked for (a pointer's, a key's, an answer's), or every
//! [`REPORTED_EVERY`] of a run that goes on. Handed the moment of each
//! frame and whether it was asked for; **it asks for no frame, and keeps
//! no more at rest than it does moving**: its intervals are a fixed array,
//! sorted in place for a summary.

use std::time::{Duration, Instant};

/// An asked-for frame this late was held back, not drawn slowly: the
/// window was out of sight or the machine asleep. It ends the run, and its
/// interval is in no summary.
pub const HELD_BACK: Duration = Duration::from_secs(1);

/// How often a run that goes on is summed up.
pub const REPORTED_EVERY: Duration = Duration::from_secs(2);

/// The fewest intervals worth a summary: fewer are a redraw or two for a
/// click, not something moving.
pub const FEWEST: usize = 10;

/// The most intervals kept: [`REPORTED_EVERY`] at 500 Hz. A run that fills
/// them is summed up sooner.
const MOST: usize = 1000;

/// How a run of frames was drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cadence {
    /// The frames drawn after the run's first, each with the interval
    /// before it.
    pub frames: usize,
    pub mean_ms: f32,
    /// The interval 95 of 100 frames came within.
    pub p95_ms: f32,
    pub worst_ms: f32,
    /// The rate the mean interval makes.
    pub hz: f32,
    /// How long after the run's last frame came the frame that showed it
    /// had ended; none for a stretch of a run that goes on.
    pub ended: Option<Duration>,
}

/// The intervals of the run being drawn.
#[derive(Debug, Clone)]
pub struct FrameMeter {
    last: Option<Instant>,
    /// In microseconds, the first `held` of them.
    gaps: [u32; MOST],
    held: usize,
    span: Duration,
}

impl Default for FrameMeter {
    fn default() -> Self {
        Self {
            last: None,
            gaps: [0; MOST],
            held: 0,
            span: Duration::ZERO,
        }
    }
}

impl FrameMeter {
    /// A frame drawn `at`, `asked` for by the frame before it: the run it
    /// ended, or the stretch of a run that went on long enough, summed up
    /// when there was one.
    pub fn drew(&mut self, at: Instant, asked: bool) -> Option<Cadence> {
        let last = self.last.replace(at)?;
        let gap = at.saturating_duration_since(last);
        if !asked || gap >= HELD_BACK {
            return self.sum_up(Some(gap));
        }
        // Two draws at one moment are one frame.
        if gap.is_zero() {
            return None;
        }
        self.gaps[self.held] = u32::try_from(gap.as_micros()).unwrap_or(u32::MAX);
        self.held += 1;
        self.span += gap;
        if self.span >= REPORTED_EVERY || self.held == MOST {
            return self.sum_up(None);
        }
        None
    }

    /// The intervals kept, summed up when there are enough, and forgotten.
    fn sum_up(&mut self, ended: Option<Duration>) -> Option<Cadence> {
        let held = std::mem::take(&mut self.held);
        let span = std::mem::take(&mut self.span);
        if held < FEWEST {
            return None;
        }
        let gaps = &mut self.gaps[..held];
        gaps.sort_unstable();
        let ms = |micros: u32| micros as f32 / 1000.0;
        let mean_ms = span.as_secs_f32() * 1000.0 / held as f32;
        Some(Cadence {
            frames: held,
            mean_ms,
            p95_ms: ms(gaps[(held * 95).div_ceil(100) - 1]),
            worst_ms: ms(gaps[held - 1]),
            hz: 1000.0 / mean_ms,
            ended,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `count` frames `every` apart from `start`, each `asked` for or not,
    /// handed to `meter`, with what it said and the moment of the last.
    fn draw(
        meter: &mut FrameMeter,
        start: Instant,
        count: usize,
        every: Duration,
        asked: bool,
    ) -> (Vec<Cadence>, Instant) {
        let mut said = Vec::new();
        let mut at = start;
        for frame in 0..count {
            at = start + every * frame as u32;
            said.extend(meter.drew(at, asked));
        }
        (said, at)
    }

    fn close(a: f32, b: f32, within: f32) -> bool {
        (a - b).abs() <= within
    }

    #[test]
    fn a_steady_run_at_164_hz_is_summed_up_at_the_first_frame_nobody_asked_for() {
        let mut meter = FrameMeter::default();
        let every = Duration::from_micros(6_098);
        let start = Instant::now();
        let (said, last) = draw(&mut meter, start, 50, every, true);
        assert!(said.is_empty(), "nothing while it runs: {said:?}");
        let cadence = meter.drew(last + Duration::from_secs(1), false).unwrap();
        assert_eq!(cadence.frames, 49);
        assert!(close(cadence.mean_ms, 6.098, 0.01), "{cadence:?}");
        assert!(close(cadence.p95_ms, 6.098, 0.01), "{cadence:?}");
        assert!(close(cadence.worst_ms, 6.098, 0.01), "{cadence:?}");
        assert!(close(cadence.hz, 164.0, 0.5), "{cadence:?}");
        assert_eq!(cadence.ended, Some(Duration::from_secs(1)));
    }

    #[test]
    fn a_run_at_60_hz_says_so_and_its_slowest_frame_stands_out() {
        let mut meter = FrameMeter::default();
        let every = Duration::from_micros(16_667);
        let start = Instant::now();
        let (_, last) = draw(&mut meter, start, 30, every, true);
        // One frame late, by one refresh.
        let late = last + every * 2;
        assert_eq!(meter.drew(late, true), None);
        let (said, last) = draw(&mut meter, late + every, 9, every, true);
        assert!(said.is_empty());
        let cadence = meter.drew(last + every, false).unwrap();
        assert_eq!(cadence.frames, 39);
        assert!(close(cadence.hz, 58.5, 1.0), "{cadence:?}");
        assert!(close(cadence.p95_ms, 16.667, 0.01), "{cadence:?}");
        assert!(close(cadence.worst_ms, 33.334, 0.01), "{cadence:?}");
    }

    #[test]
    fn a_stall_between_two_asked_frames_is_the_worst_one() {
        let mut meter = FrameMeter::default();
        let every = Duration::from_micros(6_098);
        let start = Instant::now();
        let (_, last) = draw(&mut meter, start, 20, every, true);
        let stalled = last + Duration::from_millis(150);
        assert_eq!(meter.drew(stalled, true), None);
        let (_, last) = draw(&mut meter, stalled + every, 20, every, true);
        let cadence = meter.drew(last + every, false).unwrap();
        assert_eq!(cadence.frames, 40);
        assert!(close(cadence.worst_ms, 150.0, 0.01), "{cadence:?}");
    }

    #[test]
    fn redraws_nobody_asked_for_are_no_run_however_close_together() {
        let mut meter = FrameMeter::default();
        // A scroll or a drag: a frame for each move of the pointer.
        let (said, last) = draw(
            &mut meter,
            Instant::now(),
            300,
            Duration::from_micros(16_667),
            false,
        );
        assert!(said.is_empty(), "{said:?}");
        assert_eq!(meter.drew(last + Duration::from_secs(1), false), None);
    }

    #[test]
    fn a_frame_nobody_asked_for_cuts_a_run_in_two_and_each_half_is_its_own() {
        let mut meter = FrameMeter::default();
        let every = Duration::from_micros(6_098);
        let start = Instant::now();
        let (_, last) = draw(&mut meter, start, 20, every, true);
        let after = last + Duration::from_millis(500);
        let first = meter.drew(after, false).unwrap();
        assert_eq!(first.frames, 19);
        assert_eq!(first.ended, Some(Duration::from_millis(500)));
        let sixty = Duration::from_micros(16_667);
        let (said, last) = draw(&mut meter, after + sixty, 30, sixty, true);
        assert!(said.is_empty());
        let second = meter.drew(last + sixty, false).unwrap();
        // The pause is in neither; the frame after it began the second.
        assert_eq!(second.frames, 30);
        assert!(second.worst_ms < 17.0, "{second:?}");
        assert!(close(second.hz, 60.0, 1.0), "{second:?}");
    }

    #[test]
    fn an_asked_frame_held_back_ends_the_run_and_counts_in_none() {
        let mut meter = FrameMeter::default();
        let every = Duration::from_micros(6_098);
        let (_, last) = draw(&mut meter, Instant::now(), 20, every, true);
        let woke = last + Duration::from_secs(30);
        let cadence = meter.drew(woke, true).unwrap();
        assert_eq!(cadence.frames, 19);
        assert!(close(cadence.worst_ms, 6.098, 0.01), "{cadence:?}");
        assert_eq!(cadence.ended, Some(Duration::from_secs(30)));
    }

    #[test]
    fn a_run_that_goes_on_is_summed_up_every_two_seconds() {
        let mut meter = FrameMeter::default();
        let every = Duration::from_micros(6_098);
        let (said, _) = draw(&mut meter, Instant::now(), 1000, every, true);
        // 999 intervals of 6.098 ms: 2 s every 328 of them.
        assert_eq!(said.len(), 3, "{said:?}");
        for cadence in &said {
            assert_eq!(cadence.frames, 328);
            assert!(close(cadence.hz, 164.0, 0.5), "{cadence:?}");
            assert_eq!(cadence.ended, None);
        }
    }

    #[test]
    fn nothing_is_said_at_rest_or_for_a_few_frames() {
        let mut meter = FrameMeter::default();
        let start = Instant::now();
        // A frame now and then, as a click or an answer draws one.
        for second in 0..20 {
            assert_eq!(meter.drew(start + Duration::from_secs(second), false), None);
        }
        let (said, last) = draw(
            &mut meter,
            start + Duration::from_secs(30),
            FEWEST,
            Duration::from_millis(8),
            true,
        );
        assert!(said.is_empty());
        assert_eq!(meter.drew(last + Duration::from_secs(5), false), None);
        // Draws at one moment, as a test's clock that stands still makes.
        let still = start + Duration::from_secs(60);
        for _ in 0..2000 {
            assert_eq!(meter.drew(still, true), None);
        }
    }
}
