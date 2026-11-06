//! Where events are persisted.
//!
//! The daemon only needs three operations from its log, behind a trait so the
//! SQLite store (`volee-store`) and an in-memory log for tests are
//! interchangeable.

use volee_core::Event;

/// Persisting or reading events failed.
#[derive(Debug, thiserror::Error)]
#[error("event log: {0}")]
pub struct LogError(pub String);

/// An append-only log of events with strictly increasing sequence numbers.
pub trait EventLog: Send + 'static {
    /// Appends events atomically and returns their sequence numbers.
    fn append(&mut self, events: &[Event]) -> Result<Vec<u64>, LogError>;

    /// Every event after `after` (from the start when `None`), in order.
    fn since(&self, after: Option<u64>) -> Result<Vec<(u64, Event)>, LogError>;

    /// The last sequence number, if the log is not empty.
    fn last_seq(&self) -> Result<Option<u64>, LogError>;
}

/// An in-memory log, for tests and ephemeral runs.
#[derive(Debug, Default, Clone)]
pub struct MemoryLog {
    events: Vec<Event>,
}

impl MemoryLog {
    /// An empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// A log that already holds `events`, numbered from 1.
    pub fn with(events: Vec<Event>) -> Self {
        Self { events }
    }
}

impl EventLog for MemoryLog {
    fn append(&mut self, events: &[Event]) -> Result<Vec<u64>, LogError> {
        let first = self.events.len() as u64 + 1;
        self.events.extend_from_slice(events);
        Ok((first..first + events.len() as u64).collect())
    }

    fn since(&self, after: Option<u64>) -> Result<Vec<(u64, Event)>, LogError> {
        let skip = usize::try_from(after.unwrap_or(0)).unwrap_or(usize::MAX);
        Ok(self
            .events
            .iter()
            .enumerate()
            .skip(skip)
            .map(|(index, event)| (index as u64 + 1, event.clone()))
            .collect())
    }

    fn last_seq(&self) -> Result<Option<u64>, LogError> {
        Ok((!self.events.is_empty()).then_some(self.events.len() as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ulid::Ulid;
    use volee_core::time::Timestamp;
    use volee_core::{AgentId, EventKind};

    fn event(n: i64) -> Event {
        Event::new(
            Timestamp::from_millis(n),
            EventKind::AgentForgotten {
                agent: AgentId::from_ulid(Ulid::from_parts(1, 1)),
            },
        )
    }

    #[test]
    fn sequence_numbers_start_at_one_and_grow() {
        let mut log = MemoryLog::new();
        assert_eq!(log.last_seq().unwrap(), None);
        assert_eq!(log.append(&[event(1), event(2)]).unwrap(), [1, 2]);
        assert_eq!(log.append(&[event(3)]).unwrap(), [3]);
        assert_eq!(log.last_seq().unwrap(), Some(3));
        let tail: Vec<u64> = log
            .since(Some(1))
            .unwrap()
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        assert_eq!(tail, [2, 3]);
        assert_eq!(log.since(None).unwrap().len(), 3);
    }
}
