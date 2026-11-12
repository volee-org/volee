//! Activity skylines.
//!
//! The registry keeps only an agent's last activity and its state. The
//! interface counts the activity events it applies in short time buckets,
//! and keeps when each agent's state changed, so cards can show how an
//! agent spent the last minutes: a bar per bucket, low while it worked
//! quietly and taller with each event, another color while it waited for
//! the person or when it failed. Everything comes from the time of events,
//! so the history replayed at start draws the same skyline as the live one.

use std::collections::{HashMap, VecDeque};

use volee_core::time::Timestamp;
use volee_core::{AgentId, AgentState};

/// How many buckets are kept: the widest activity a card shows, one bucket
/// per column (two minutes and forty seconds).
pub(crate) const BUCKETS: usize = 32;

/// How long one bucket lasts.
const BUCKET_MILLIS: i64 = 5_000;

/// How many state changes are kept per agent: more than the buckets show.
const PHASES: usize = 32;

/// What an agent did during a bucket, for the color of its bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Phase {
    /// Nothing, or nothing known.
    Rest,
    /// Waited for the person.
    Wait,
    /// Worked.
    Work,
    /// Failed.
    Fail,
}

impl Phase {
    const fn of(state: &AgentState) -> Self {
        match state {
            AgentState::Working | AgentState::Launching => Self::Work,
            AgentState::NeedsInput { .. } => Self::Wait,
            AgentState::Failed => Self::Fail,
            _ => Self::Rest,
        }
    }
}

/// One column of a skyline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bar {
    /// Its glyph, from `▁` to `█`.
    pub(crate) glyph: char,
    /// What the agent did then.
    pub(crate) phase: Phase,
}

/// Activity counts and state changes per agent.
#[derive(Clone, Debug, Default)]
pub(crate) struct ActivityLog {
    by_agent: HashMap<AgentId, VecDeque<(i64, u16)>>,
    phases: HashMap<AgentId, VecDeque<(i64, Phase)>>,
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

    /// Notes that `agent` entered `state` at `at`.
    pub(crate) fn record_state(&mut self, agent: AgentId, at: Timestamp, state: &AgentState) {
        let phases = self.phases.entry(agent).or_default();
        let phase = Phase::of(state);
        if phases.back().is_some_and(|(_, last)| *last == phase) {
            return;
        }
        phases.push_back((at.as_millis(), phase));
        while phases.len() > PHASES {
            phases.pop_front();
        }
    }

    /// Forgets an agent.
    pub(crate) fn forget(&mut self, agent: AgentId) {
        self.by_agent.remove(&agent);
        self.phases.remove(&agent);
    }

    /// What `agent` did between `start` and `end`, the most telling first:
    /// a failure, then work, then a wait.
    fn phase_during(&self, agent: AgentId, start: i64, end: i64) -> Phase {
        let Some(phases) = self.phases.get(&agent) else {
            return Phase::Rest;
        };
        let before = phases
            .iter()
            .rev()
            .find(|(at, _)| *at <= start)
            .map_or(Phase::Rest, |(_, phase)| *phase);
        phases
            .iter()
            .filter(|(at, _)| *at > start && *at < end)
            .map(|(_, phase)| *phase)
            .fold(before, Phase::max)
    }

    /// The last `width` buckets of `agent` up to `now`, oldest first.
    pub(crate) fn skyline(&self, agent: AgentId, now: Timestamp, width: usize) -> Vec<Bar> {
        let counts = self.counts(agent, now);
        let shown = width.min(BUCKETS);
        let last = now.as_millis().div_euclid(BUCKET_MILLIS);
        (0..shown)
            .map(|column| {
                let age = i64::try_from(shown - 1 - column).unwrap_or(0);
                let bucket = last - age;
                let start = bucket * BUCKET_MILLIS;
                let phase = self.phase_during(agent, start, start + BUCKET_MILLIS);
                let events = counts[BUCKETS - shown + column];
                Bar {
                    glyph: glyph(phase, events),
                    phase,
                }
            })
            .collect()
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

/// The glyph of a bucket: a low bar while the agent worked, a step taller
/// with each event (a tool call every second fills it); a low one while it
/// waited, a higher one when it failed; events alone, else the baseline,
/// while it rested.
fn glyph(phase: Phase, events: u16) -> char {
    let level = |height: u16| BARS[usize::from(height.clamp(1, 8)) - 1];
    match phase {
        Phase::Work => level(2 + events.min(6)),
        Phase::Wait => level(2),
        Phase::Fail => level(3),
        Phase::Rest => level(1 + events.min(7)),
    }
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
        log.record(agent(), at(11));
        // Buckets of five seconds: 0 and 1 share one, 11 is two later.
        let counts = log.counts(agent(), at(12));
        assert_eq!(counts[BUCKETS - 1], 1);
        assert_eq!(counts[BUCKETS - 2], 0);
        assert_eq!(counts[BUCKETS - 3], 2);
        assert_eq!(counts.iter().sum::<u16>(), 3);
        // Later, the same activity slides to the left, then out.
        assert_eq!(log.counts(agent(), at(32))[BUCKETS - 5], 1);
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
    fn a_skyline_shows_work_waits_and_failures() {
        let mut log = ActivityLog::default();
        let agent = agent();
        log.record_state(agent, at(0), &AgentState::Working);
        log.record(agent, at(6));
        for second in [11, 12, 13] {
            log.record(agent, at(second));
        }
        log.record_state(
            agent,
            at(15),
            &AgentState::NeedsInput {
                request: volee_core::state::InputRequest::Question,
            },
        );
        log.record_state(agent, at(26), &AgentState::Failed);
        let bars: String = log
            .skyline(agent, at(29), 7)
            .iter()
            .map(|bar| bar.glyph)
            .collect();
        // Buckets of five seconds, the last one now: before work, quiet
        // work, one event, three events, a wait, a wait, a failure.
        assert_eq!(bars, "▁▂▃▅▂▂▃");
        let phases: Vec<Phase> = log
            .skyline(agent, at(29), 3)
            .iter()
            .map(|b| b.phase)
            .collect();
        assert_eq!(phases, [Phase::Wait, Phase::Wait, Phase::Fail]);
        // The same state twice is one change.
        log.record_state(agent, at(30), &AgentState::Failed);
        assert_eq!(log.phases[&agent].len(), 3, "work, wait, failure");
        log.forget(agent);
        assert!(
            log.skyline(agent, at(29), 3)
                .iter()
                .all(|bar| bar.glyph == '▁')
        );
    }
}
