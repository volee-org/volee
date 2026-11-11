//! Activity sparklines.
//!
//! The registry keeps only an agent's last activity. The interface counts
//! the activity events it applies in short time buckets, so cards can show
//! how busy an agent has been over the last few minutes.

use std::collections::{HashMap, VecDeque};

use volee_core::AgentId;
use volee_core::time::Timestamp;

/// How many buckets are kept: the widest activity a card shows, one bucket
/// per column (eight minutes).
pub(crate) const BUCKETS: usize = 32;

/// How long one bucket lasts.
const BUCKET_MILLIS: i64 = 15_000;

/// Activity counts per agent and per bucket.
#[derive(Clone, Debug, Default)]
pub(crate) struct ActivityLog {
    by_agent: HashMap<AgentId, VecDeque<(i64, u16)>>,
}

impl ActivityLog {
    /// Counts one activity of `agent` at `at`.
    pub(crate) fn record(&mut self, agent: AgentId, at: Timestamp) {
        let bucket = at.as_millis().div_euclid(BUCKET_MILLIS);
        let buckets = self.by_agent.entry(agent).or_default();
        if let Some((_, count)) = buckets.iter_mut().find(|(b, _)| *b == bucket) {
            *count = count.saturating_add(1);
        } else {
            let position = buckets.partition_point(|(b, _)| *b < bucket);
            buckets.insert(position, (bucket, 1));
        }
        // Keep only what a sparkline can show, measured from the newest bucket.
        let newest = buckets.back().map_or(bucket, |(b, _)| *b);
        let oldest_kept = newest - i64::try_from(BUCKETS).unwrap_or(0);
        while buckets.front().is_some_and(|(b, _)| *b < oldest_kept) {
            buckets.pop_front();
        }
    }

    /// Forgets an agent.
    pub(crate) fn forget(&mut self, agent: AgentId) {
        self.by_agent.remove(&agent);
    }

    /// The counts of the last [`BUCKETS`] buckets up to `now`, oldest first.
    pub(crate) fn counts(&self, agent: AgentId, now: Timestamp) -> [u16; BUCKETS] {
        let mut counts = [0; BUCKETS];
        let Some(buckets) = self.by_agent.get(&agent) else {
            return counts;
        };
        let last = now.as_millis().div_euclid(BUCKET_MILLIS);
        for (bucket, count) in buckets {
            let age = last - bucket;
            if let Ok(age) = usize::try_from(age)
                && age < BUCKETS
            {
                counts[BUCKETS - 1 - age] = *count;
            }
        }
        counts
    }
}

/// The bars of a sparkline, from low to high.
pub(crate) const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The fewest events that fill a bar: a quiet agent's lone events show as
/// short bars, not as full ones.
const FULL_BAR: u16 = 3;

/// The bar of each count, scaled to the highest one (and to at least
/// [`FULL_BAR`] events); `None` for zero.
pub(crate) fn bars(counts: &[u16]) -> Vec<Option<char>> {
    let max = counts.iter().copied().max().unwrap_or(0).max(FULL_BAR);
    counts
        .iter()
        .map(|&count| {
            (count > 0).then(|| {
                let level = (usize::from(count) * BARS.len()).div_ceil(usize::from(max));
                BARS[level.clamp(1, BARS.len()) - 1]
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ulid::Ulid;

    use super::*;

    fn agent() -> AgentId {
        AgentId::from_ulid(Ulid::from_parts(1, 1))
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_millis(1_800_000_000_000 + seconds * 1_000)
    }

    #[test]
    fn counts_land_in_buckets_ending_now() {
        let mut log = ActivityLog::default();
        log.record(agent(), at(0));
        log.record(agent(), at(1));
        log.record(agent(), at(31));
        // Buckets of fifteen seconds: 0 and 1 share one, 31 is two later.
        let counts = log.counts(agent(), at(35));
        assert_eq!(counts[BUCKETS - 1], 1);
        assert_eq!(counts[BUCKETS - 2], 0);
        assert_eq!(counts[BUCKETS - 3], 2);
        assert_eq!(counts.iter().sum::<u16>(), 3);
        // Later, the same activity slides to the left, then out.
        assert_eq!(log.counts(agent(), at(95))[BUCKETS - 5], 1);
        assert_eq!(log.counts(agent(), at(600)), [0; BUCKETS]);
    }

    #[test]
    fn old_buckets_are_dropped() {
        let mut log = ActivityLog::default();
        for minute in 0..60 {
            log.record(agent(), at(minute * 60));
        }
        assert!(log.by_agent[&agent()].len() <= BUCKETS + 1);
        log.forget(agent());
        assert_eq!(log.counts(agent(), at(0)), [0; BUCKETS]);
    }

    #[test]
    fn bars_scale_to_the_busiest_bucket() {
        let bars = bars(&[0, 1, 2, 4]);
        assert_eq!(bars, [None, Some('▂'), Some('▄'), Some('█')]);
        assert_eq!(super::bars(&[0, 0]), [None, None]);
        // A quiet agent: one event is a third of a bar, three fill it.
        assert_eq!(
            super::bars(&[1, 0, 1, 2, 3]),
            [Some('▃'), None, Some('▃'), Some('▆'), Some('█')]
        );
    }
}
