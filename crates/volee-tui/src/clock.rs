//! Clocks: where the interface gets the current time.
//!
//! Elapsed times, spinners and the demo scenario all depend on the time, so
//! tests and recordings pass a [`ManualClock`] and get the same frame on
//! every run.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use volee_core::time::Timestamp;

/// Tells the interface what time it is.
pub trait Clock: Send + Sync + fmt::Debug {
    /// The current time.
    fn now(&self) -> Timestamp;
}

/// The system's wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
            });
        Timestamp::from_millis(millis)
    }
}

/// A clock that moves only when told to. Clones share the same time.
#[derive(Clone, Debug)]
pub struct ManualClock {
    millis: Arc<AtomicI64>,
}

impl ManualClock {
    /// A clock stopped at `at`.
    pub fn new(at: Timestamp) -> Self {
        Self {
            millis: Arc::new(AtomicI64::new(at.as_millis())),
        }
    }

    /// Sets the time.
    pub fn set(&self, at: Timestamp) {
        self.millis.store(at.as_millis(), Ordering::Relaxed);
    }

    /// Moves the time forward.
    pub fn advance(&self, millis: i64) {
        self.millis.fetch_add(millis, Ordering::Relaxed);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.millis.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_clock_moves_only_when_told() {
        let clock = ManualClock::new(Timestamp::from_millis(1_000));
        let shared = clock.clone();
        assert_eq!(clock.now(), Timestamp::from_millis(1_000));
        shared.advance(500);
        assert_eq!(clock.now(), Timestamp::from_millis(1_500));
        clock.set(Timestamp::from_millis(42));
        assert_eq!(shared.now(), Timestamp::from_millis(42));
    }

    #[test]
    fn the_system_clock_is_after_2025() {
        assert!(SystemClock.now() > Timestamp::from_millis(1_735_689_600_000));
    }
}
