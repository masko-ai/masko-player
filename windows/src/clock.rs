//! Play time (spec/player.md, "The clock") and the host's clocks, so tests
//! can drive the player with a fake one.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// The longest time between two reads of the clock that still counts. A
/// program that did not run for longer (Windows froze it, a debugger) saw
/// nothing either: counting that time would replay every timer of it at once
/// when it runs again, so its clips end, time out and stay unplayable.
pub const LONGEST_STOP: f64 = 5.0;

/// The two clocks the player reads.
pub trait Host: Send {
    /// Seconds on a clock that never jumps.
    fn uptime(&self) -> f64;
    /// The wall clock in Unix milliseconds, for signed-link expiry.
    fn unix_ms(&self) -> u64;
}

/// The computer's own clocks. Uptime is awake time, like the Mac player's
/// `systemUptime`: the hours the computer sleeps do not count (on Windows,
/// `Instant` counts them, and a night asleep would run the mascot's clips on
/// by a night), and neither does a stop of the program (`LONGEST_STOP`).
pub struct RealHost {
    awake: SkipStops<AwakeClock>,
}

impl RealHost {
    pub fn new() -> RealHost {
        RealHost {
            awake: SkipStops::new(AwakeClock {
                origin: awake_seconds(),
            }),
        }
    }
}

impl Default for RealHost {
    fn default() -> Self {
        RealHost::new()
    }
}

impl Host for RealHost {
    fn uptime(&self) -> f64 {
        self.awake.uptime()
    }

    fn unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Seconds the computer was awake, since `origin`.
struct AwakeClock {
    origin: f64,
}

impl Host for AwakeClock {
    fn uptime(&self) -> f64 {
        awake_seconds() - self.origin
    }

    fn unix_ms(&self) -> u64 {
        0
    }
}

/// Awake time in seconds: `QueryUnbiasedInterruptTimePrecise` leaves out the
/// time asleep or hibernating (Windows 10 and later).
#[cfg(windows)]
fn awake_seconds() -> f64 {
    // SAFETY: no arguments; it returns 100 ns steps.
    let ticks =
        unsafe { windows::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTimePrecise() };
    ticks as f64 / 10_000_000.0
}

/// Awake time in seconds: `Instant` already leaves out sleep on the Mac
/// (`CLOCK_UPTIME_RAW`) and on Linux (`CLOCK_MONOTONIC`).
#[cfg(not(windows))]
fn awake_seconds() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// A host's clock with every stop longer than `LONGEST_STOP` between two reads
/// taken out. The player reads it on every pump, many times a second while a
/// clip plays, so only a program that did not run skips time.
pub struct SkipStops<H> {
    inner: H,
    stops: Mutex<Stops>,
}

#[derive(Debug, Default)]
struct Stops {
    last: Option<f64>,
    skipped: f64,
    given: f64,
}

impl<H: Host> SkipStops<H> {
    pub fn new(inner: H) -> SkipStops<H> {
        SkipStops {
            inner,
            stops: Mutex::new(Stops::default()),
        }
    }
}

impl<H: Host> Host for SkipStops<H> {
    fn uptime(&self) -> f64 {
        let raw = self.inner.uptime();
        let mut s = lock(&self.stops);
        if let Some(last) = s.last {
            if raw - last > LONGEST_STOP {
                s.skipped += raw - last;
            }
        }
        s.last = Some(raw);
        // Never backwards.
        s.given = s.given.max(raw - s.skipped);
        s.given
    }

    fn unix_ms(&self) -> u64 {
        self.inner.unix_ms()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A clock tests move by hand. Clones share the same time.
#[derive(Clone, Default)]
pub struct FakeHost {
    seconds: Arc<Mutex<f64>>,
}

impl FakeHost {
    pub fn new() -> FakeHost {
        FakeHost::default()
    }

    /// Moves time on.
    pub fn advance(&self, seconds: f64) {
        *lock(&self.seconds) += seconds;
    }
}

impl Host for FakeHost {
    fn uptime(&self) -> f64 {
        *lock(&self.seconds)
    }

    fn unix_ms(&self) -> u64 {
        1_800_000_000_000 + (self.uptime() * 1000.0) as u64
    }
}

/// Play time: 0 when the mascot starts, moving with real time, stopped while
/// the player is paused.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PlayClock {
    /// Play time banked before the current run, in seconds.
    banked: f64,
    /// The uptime at which the current run began, while running.
    since: Option<f64>,
}

impl PlayClock {
    pub fn seconds(&self, uptime: f64) -> f64 {
        self.banked + self.since.map(|s| (uptime - s).max(0.0)).unwrap_or(0.0)
    }

    /// Whole milliseconds, as every engine call takes them.
    pub fn milliseconds(&self, uptime: f64) -> u64 {
        (self.seconds(uptime) * 1000.0).floor() as u64
    }

    pub fn run(&mut self, uptime: f64) {
        if self.since.is_none() {
            self.since = Some(uptime);
        }
    }

    pub fn stop(&mut self, uptime: f64) {
        self.banked = self.seconds(uptime);
        self.since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_stop_is_left_out_and_short_ones_count() {
        let fake = FakeHost::new();
        let clock = SkipStops::new(fake.clone());
        assert_eq!(clock.uptime(), 0.0);
        fake.advance(1.0);
        assert_eq!(clock.uptime(), 1.0);
        fake.advance(LONGEST_STOP);
        assert_eq!(
            clock.uptime(),
            1.0 + LONGEST_STOP,
            "a stop of LONGEST_STOP counts"
        );
        // A night the program did not run: time goes on from where it was.
        fake.advance(8.0 * 3600.0);
        assert_eq!(clock.uptime(), 1.0 + LONGEST_STOP);
        fake.advance(0.25);
        assert_eq!(clock.uptime(), 1.25 + LONGEST_STOP);
    }

    #[test]
    fn the_real_clock_moves_on() {
        let clock = RealHost::new();
        let a = clock.uptime();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = clock.uptime();
        assert!(b - a >= 0.015 && b - a < 1.0, "{a} then {b}");
    }

    #[test]
    fn play_time_stops_while_paused() {
        let mut c = PlayClock::default();
        assert_eq!(c.milliseconds(5.0), 0);
        c.run(10.0);
        assert_eq!(c.milliseconds(10.25), 250);
        c.stop(10.5);
        assert_eq!(c.milliseconds(99.0), 500);
        c.run(20.0);
        assert_eq!(c.milliseconds(20.1), 600);
    }
}
