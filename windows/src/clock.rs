//! Play time (spec/player.md, "The clock") and the host's clocks, so tests
//! can drive the player with a fake one.

use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The two clocks the player reads.
pub trait Host: Send {
    /// Seconds on a clock that never jumps.
    fn uptime(&self) -> f64;
    /// The wall clock in Unix milliseconds, for signed-link expiry.
    fn unix_ms(&self) -> u64;
}

/// The computer's own clocks.
pub struct RealHost {
    origin: Instant,
}

impl RealHost {
    pub fn new() -> RealHost {
        RealHost {
            origin: Instant::now(),
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
        self.origin.elapsed().as_secs_f64()
    }

    fn unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
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
        *self.seconds.lock().unwrap() += seconds;
    }
}

impl Host for FakeHost {
    fn uptime(&self) -> f64 {
        *self.seconds.lock().unwrap()
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
