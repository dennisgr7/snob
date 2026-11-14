//! Telling that the machine slept, so a walk treats it as the end of a
//! sitting rather than as network failures in a row (`pager::ListWalker`).
//!
//! **Two clocks, compared.** The wall clock moves while the machine sleeps
//! and [`now`] does not; how much further the first ran since the last look
//! is the time asleep. [`std::time::Instant`] alone will not do: on Windows
//! it counts through a sleep.

use std::time::Duration;

use snob_core::EpochMs;

/// How much further the wall clock may run than the awake one before the
/// difference is a sleep. Far above what the time service corrects at a time,
/// and below the shortest sleep that takes the network away.
pub const MARGIN: Duration = Duration::from_secs(60);

/// A clock that stands still while the machine sleeps. Only differences
/// between two readings mean anything. `Instant` is one on Linux
/// (`CLOCK_MONOTONIC`) and macOS (`CLOCK_UPTIME_RAW`); on Windows it counts
/// through a sleep, so the unbiased interrupt time is read there.
pub fn now() -> Duration {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;

        let mut hundreds_of_nanoseconds = 0u64;
        // SAFETY: the pointer is to a local the call writes one value into.
        let read = unsafe { QueryUnbiasedInterruptTime(&mut hundreds_of_nanoseconds) };
        if read != 0 {
            return Duration::from_nanos(hundreds_of_nanoseconds.saturating_mul(100));
        }
        // Fails only on a null pointer. A reading that does not move makes
        // every gap look like sleep, which is the safe mistake.
        Duration::ZERO
    }
    #[cfg(not(windows))]
    {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed()
    }
}

/// The last look at both clocks.
#[derive(Debug, Clone, Copy)]
pub struct Asleep {
    wall: EpochMs,
    awake: Duration,
}

impl Asleep {
    pub fn new(wall: EpochMs, awake: Duration) -> Self {
        Self { wall, awake }
    }

    /// How long the machine slept since the last look, when the wall clock
    /// ran at least [`MARGIN`] further than the awake one. Reported once. A
    /// wall clock that went backwards is a clock somebody set, not a sleep.
    pub fn check(&mut self, wall: EpochMs, awake: Duration) -> Option<Duration> {
        let on_the_wall = wall - self.wall;
        let awake_for = awake.saturating_sub(self.awake);
        *self = Self::new(wall, awake);
        let on_the_wall = Duration::from_millis(u64::try_from(on_the_wall).ok()?);
        on_the_wall
            .checked_sub(awake_for)
            .filter(|slept| *slept >= MARGIN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: i64 = 1_700_000_000_000;

    fn at(wall_ms: i64, awake_s: u64) -> (EpochMs, Duration) {
        (EpochMs::new(START + wall_ms), Duration::from_secs(awake_s))
    }

    #[test]
    fn a_machine_that_stayed_awake_did_not_sleep() {
        let (wall, awake) = at(0, 100);
        let mut asleep = Asleep::new(wall, awake);
        let (wall, awake) = at(3_600_000, 3_700);
        assert_eq!(asleep.check(wall, awake), None, "an hour each");
    }

    #[test]
    fn the_wall_running_ahead_is_the_time_asleep() {
        let (wall, awake) = at(0, 100);
        let mut asleep = Asleep::new(wall, awake);
        let (wall, awake) = at(8 * 3_600_000 + 10_000, 110);
        assert_eq!(
            asleep.check(wall, awake),
            Some(Duration::from_secs(8 * 3_600))
        );
        let (wall, awake) = at(8 * 3_600_000 + 20_000, 120);
        assert_eq!(asleep.check(wall, awake), None, "reported once");
    }

    #[test]
    fn a_small_correction_is_not_a_sleep() {
        let (wall, awake) = at(0, 100);
        let mut asleep = Asleep::new(wall, awake);
        let (wall, awake) = at(30_000, 100);
        assert_eq!(asleep.check(wall, awake), None, "under the margin");
    }

    #[test]
    fn a_clock_set_back_is_not_a_sleep() {
        let (wall, awake) = at(0, 100);
        let mut asleep = Asleep::new(wall, awake);
        let (wall, awake) = at(-3_600_000, 160);
        assert_eq!(asleep.check(wall, awake), None);
    }
}
