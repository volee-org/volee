//! The error type of this crate.

use std::io;
use std::path::Path;

use volee_core::registry::ApplyError;

use crate::seq::Seq;

/// Everything that can go wrong while storing or reading facts.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite refused an operation.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A file operation failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being done, including the path.
        context: String,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// A value could not be turned into JSON.
    #[error("cannot serialize to JSON: {0}")]
    Serialize(#[source] serde_json::Error),
    /// A serialized event has no `type` tag. Events produced by
    /// `volee-core` always have one, so this signals a change in its serde
    /// layout.
    #[error("serialized event has no type tag")]
    MissingTag,
    /// The database was written by a newer volee than this one.
    #[error(
        "the database schema is version {found}, newer than the version {supported} this \
         volee supports: upgrade volee"
    )]
    SchemaTooNew {
        /// The version found in the database.
        found: u32,
        /// The newest version this build understands.
        supported: u32,
    },
    /// A stored payload does not deserialize, typically because a newer volee
    /// wrote an event this one does not know.
    #[error("event {seq} cannot be read (written by a newer volee?): {source}")]
    UnreadableEvent {
        /// The event's sequence number.
        seq: Seq,
        /// The underlying error.
        #[source]
        source: serde_json::Error,
    },
    /// A stored event does not apply to the state built by the events before
    /// it: the log is corrupted.
    #[error("event {seq} does not apply, the log is corrupted: {source}")]
    Corrupted {
        /// The event's sequence number.
        seq: Seq,
        /// Why it does not apply.
        #[source]
        source: ApplyError,
    },
    /// A spool record is too large to be appended atomically.
    #[error("spool record is {size} bytes, over the {max} byte limit")]
    RecordTooLarge {
        /// The size of the encoded line, newline included.
        size: usize,
        /// The largest accepted size.
        max: usize,
    },
}

impl StoreError {
    /// An IO error while doing `action` to `path`.
    pub(crate) fn io_at(action: &str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            context: format!("cannot {action} {}", path.display()),
            source,
        }
    }
}
