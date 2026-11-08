//! Noticing time away from the computer, so the tray can offer to take it
//! out of whatever was being tracked.
//!
//! A platform probe says when input stopped and started again (Wayland's
//! ext-idle-notify on Linux, the HID idle time on macOS). The tray's
//! once-a-second tick adds sleep: wall-clock time that a monotonic clock
//! didn't see pass means the machine was suspended, even if the probe saw
//! nothing.

/// Never count less sleep than this as away, however short the idle
/// threshold, so a stepped wall clock (an NTP correction) isn't mistaken
/// for a nap.
const MIN_SLEEP_MS: i64 = 60_000;

/// A stretch away from the computer, as unix ms [from, to).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Away {
    pub from: i64,
    pub to: i64,
}

/// Turns probe reports and ticks into [`Away`] stretches.
pub struct Tracker {
    /// How long without input counts as away, in ms; 0 turns tracking off.
    threshold: i64,
    /// While away: when input stopped.
    since: Option<i64>,
    /// The last tick's wall and monotonic clocks.
    last_tick: Option<(i64, i64)>,
    /// Stretches never start before this: the end of the last one, so a
    /// probe that counts sleep as idle can't report it twice.
    floor: i64,
}

impl Tracker {
    pub fn new(threshold_ms: i64) -> Self {
        Self {
            threshold: threshold_ms.max(0),
            since: None,
            last_tick: None,
            floor: i64::MIN,
        }
    }

    pub fn threshold(&self) -> i64 {
        self.threshold
    }

    /// The probe noticed no input since `since`.
    pub fn idle(&mut self, since: i64) {
        if self.threshold > 0 && self.since.is_none() {
            self.since = Some(since);
        }
    }

    /// The probe saw input at `at`: the stretch away, if long enough.
    pub fn active(&mut self, at: i64) -> Option<Away> {
        let from = self.since.take()?;
        self.finish(from, at)
    }

    /// For probes that report time since the last input (macOS), polled
    /// at `now`.
    #[cfg(any(target_os = "macos", test))]
    pub fn idle_for(&mut self, now: i64, idle_ms: i64) -> Option<Away> {
        if self.threshold > 0 && idle_ms >= self.threshold {
            self.idle(now - idle_ms);
            None
        } else {
            self.active(now - idle_ms)
        }
    }

    /// Once a second, with the wall clock and a monotonic clock in ms
    /// (from any fixed point) that stops while the machine sleeps, as
    /// Rust's `Instant` does on Linux and macOS. Wall time the monotonic
    /// clock didn't see was sleep: a stretch away, unless the probe already
    /// has one open. A merely late tick (a stalled loop, a menu held open)
    /// moves both clocks alike, so it isn't.
    pub fn tick(&mut self, now: i64, mono: i64) -> Option<Away> {
        let (last, last_mono) = self.last_tick.replace((now, mono))?;
        let slept = (now - last) - (mono - last_mono);
        if self.threshold == 0 || self.since.is_some() || slept < self.threshold.max(MIN_SLEEP_MS)
        {
            return None;
        }
        self.finish(last, now)
    }

    fn finish(&mut self, from: i64, to: i64) -> Option<Away> {
        let from = from.max(self.floor);
        if to - from < self.threshold {
            return None;
        }
        self.floor = to;
        Some(Away { from, to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    #[test]
    fn idle_then_active_is_a_stretch() {
        let mut t = Tracker::new(5 * MIN);
        assert_eq!(t.active(MIN), None);
        t.idle(10 * MIN);
        // Reported again while still away: keeps the first start.
        t.idle(12 * MIN);
        assert_eq!(
            t.active(30 * MIN),
            Some(Away {
                from: 10 * MIN,
                to: 30 * MIN
            })
        );
        assert_eq!(t.active(31 * MIN), None);
    }

    #[test]
    fn off_when_threshold_is_zero() {
        let mut t = Tracker::new(0);
        t.idle(0);
        assert_eq!(t.active(60 * MIN), None);
        t.tick(0, 0);
        assert_eq!(t.tick(60 * MIN, 0), None);
        assert_eq!(t.idle_for(60 * MIN, 60 * MIN), None);
        assert_eq!(t.idle_for(61 * MIN, 0), None);
    }

    #[test]
    fn wall_time_the_monotonic_clock_missed_is_sleep() {
        let mut t = Tracker::new(5 * MIN);
        assert_eq!(t.tick(0, 0), None);
        assert_eq!(t.tick(1_000, 1_000), None);
        // Late by 20 minutes, but awake all along: a stall, not sleep.
        assert_eq!(t.tick(20 * MIN, 20 * MIN), None);
        // 40 minutes of wall time, one second of it awake.
        assert_eq!(
            t.tick(60 * MIN, 20 * MIN + 1_000),
            Some(Away {
                from: 20 * MIN,
                to: 60 * MIN
            })
        );
        // Shorter than the threshold: not away.
        assert_eq!(t.tick(64 * MIN, 20 * MIN + 2_000), None);
    }

    #[test]
    fn sleep_while_idle_is_one_stretch() {
        let mut t = Tracker::new(5 * MIN);
        t.tick(0, 0);
        t.idle(MIN);
        // Suspended while away: the probe's stretch covers it.
        assert_eq!(t.tick(50 * MIN, 2 * MIN), None);
        assert_eq!(
            t.active(51 * MIN),
            Some(Away {
                from: MIN,
                to: 51 * MIN
            })
        );
    }

    #[test]
    fn polled_idle_time() {
        let mut t = Tracker::new(5 * MIN);
        assert_eq!(t.idle_for(10 * MIN, MIN), None);
        assert_eq!(t.idle_for(20 * MIN, 6 * MIN), None);
        assert_eq!(t.idle_for(30 * MIN, 16 * MIN), None);
        // Input 10 s ago: away from 14 min (20 - 6) until then.
        assert_eq!(
            t.idle_for(31 * MIN, 10_000),
            Some(Away {
                from: 14 * MIN,
                to: 31 * MIN - 10_000
            })
        );
    }

    #[test]
    fn sleep_is_not_reported_twice() {
        // A probe that counts sleep as idle reports, after waking, idle
        // since before the sleep the tick already reported.
        let mut t = Tracker::new(5 * MIN);
        t.tick(0, 0);
        assert_eq!(t.tick(60 * MIN, 1_000), Some(Away { from: 0, to: 60 * MIN }));
        assert_eq!(t.idle_for(60 * MIN + 1_000, 61 * MIN), None);
        // Input right after waking: from the floor, too short to count.
        assert_eq!(t.idle_for(60 * MIN + 2_000, 0), None);
    }
}
