//! The production event log: the SQLite store from `volee-store`.

use std::path::Path;

use volee_core::Event;
use volee_store::{Seq, Store};

use crate::log::{EventLog, LogError};

/// The SQLite event log.
#[derive(Debug)]
pub struct SqliteLog(Store);

impl SqliteLog {
    /// Opens (or creates) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, LogError> {
        Store::open(path)
            .map(Self)
            .map_err(|error| LogError(error.to_string()))
    }
}

impl EventLog for SqliteLog {
    fn append(&mut self, events: &[Event]) -> Result<Vec<u64>, LogError> {
        self.0
            .append(events)
            .map(|seqs| seqs.into_iter().map(Seq::get).collect())
            .map_err(|error| LogError(error.to_string()))
    }

    fn since(&self, after: Option<u64>) -> Result<Vec<(u64, Event)>, LogError> {
        self.0
            .events_since(after.map(Seq::new))
            .map(|events| {
                events
                    .into_iter()
                    .map(|(seq, event)| (seq.get(), event))
                    .collect()
            })
            .map_err(|error| LogError(error.to_string()))
    }

    fn last_seq(&self) -> Result<Option<u64>, LogError> {
        self.0
            .last_seq()
            .map(|seq| seq.map(Seq::get))
            .map_err(|error| LogError(error.to_string()))
    }
}
