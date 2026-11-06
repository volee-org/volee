//! Time, injectable so tests are deterministic.

use std::fmt::Debug;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use volee_core::time::Timestamp;

/// A source of the current time.
pub trait Clock: Send + Sync + Debug + 'static {
    /// The current time.
    fn now(&self) -> Timestamp;
}

/// The system clock.
#[derive(Debug, Default, Clone, Copy)]
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

/// A clock that only moves when told to, for tests.
#[derive(Debug, Default)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    /// A clock stopped at `millis`.
    pub fn at(millis: i64) -> Self {
        Self(AtomicI64::new(millis))
    }

    /// Moves the clock forward.
    pub fn advance(&self, millis: i64) {
        self.0.fetch_add(millis, Ordering::Relaxed);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::Relaxed))
    }
}
