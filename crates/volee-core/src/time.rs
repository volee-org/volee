//! Points in time, without a clock.
//!
//! `volee-core` never reads the system clock: callers pass the current time
//! in. That keeps every rule deterministic and testable.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch, in UTC.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Builds a timestamp from milliseconds since the Unix epoch.
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Milliseconds since the Unix epoch.
    pub const fn as_millis(self) -> i64 {
        self.0
    }

    /// Milliseconds elapsed from `self` to `later`, never negative.
    pub fn millis_until(self, later: Self) -> u64 {
        u64::try_from(later.0.saturating_sub(self.0)).unwrap_or(0)
    }

    /// Moves the timestamp forward.
    #[must_use]
    pub const fn plus_millis(self, millis: i64) -> Self {
        Self(self.0.saturating_add(millis))
    }
}

/// A compact human description of an elapsed time: `12 s`, `4 min`, `3 h`,
/// `2 d`. Used for "since" columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Elapsed(pub u64);

impl Elapsed {
    /// Time elapsed between two timestamps.
    pub fn between(from: Timestamp, to: Timestamp) -> Self {
        Self(from.millis_until(to))
    }
}

impl fmt::Display for Elapsed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self.0 / 1_000;
        match seconds {
            0..60 => write!(f, "{seconds} s"),
            60..3_600 => write!(f, "{} min", seconds / 60),
            3_600..86_400 => write!(f, "{} h", seconds / 3_600),
            _ => write!(f, "{} d", seconds / 86_400),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_picks_a_readable_unit() {
        let start = Timestamp::from_millis(0);
        let cases = [
            (999, "0 s"),
            (59_000, "59 s"),
            (60_000, "1 min"),
            (3_599_000, "59 min"),
            (3_600_000, "1 h"),
            (86_400_000, "1 d"),
        ];
        for (millis, expected) in cases {
            let label = Elapsed::between(start, Timestamp::from_millis(millis)).to_string();
            assert_eq!(label, expected, "{millis} ms");
        }
    }

    #[test]
    fn elapsed_is_never_negative() {
        let later = Timestamp::from_millis(10_000);
        assert_eq!(
            Elapsed::between(later, Timestamp::from_millis(0)),
            Elapsed(0)
        );
    }
}
