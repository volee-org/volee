//! The event log.

use std::fs::{self, OpenOptions};
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use volee_core::event::EventKind;
use volee_core::{AgentId, Event, Registry};

use crate::error::StoreError;
use crate::migrations;
use crate::seq::Seq;

/// How long a connection waits for another one to release a lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The append-only log of events, in SQLite.
///
/// Rows are never updated or deleted. Each row keeps the whole event as JSON
/// in `payload`, so reading back does not depend on the other columns; they
/// exist to filter and index (`kind`, `agent`, `at`).
#[derive(Debug)]
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens the database at `path`, creating it (and its parent directories)
    /// when missing, and migrates it to the current schema.
    ///
    /// On Unix the file is created readable and writable by its owner only:
    /// it holds messages and paths that belong to the user.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|source| StoreError::io_at("create directory", parent, source))?;
        }
        create_private_file(path)?;
        let connection = Connection::open(path)?;
        Self::configure(connection, true)
    }

    /// Opens a private in-memory database. Meant for tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::configure(Connection::open_in_memory()?, false)
    }

    fn configure(mut connection: Connection, durable: bool) -> Result<Self, StoreError> {
        connection.busy_timeout(BUSY_TIMEOUT)?;
        if durable {
            // WAL lets the daemon append while a reader (a CLI command) reads.
            connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| {
                row.get::<_, String>(0)
            })?;
            connection.pragma_update(None, "synchronous", "NORMAL")?;
        }
        connection.pragma_update(None, "foreign_keys", true)?;
        migrations::run(&mut connection)?;
        Ok(Self { connection })
    }

    /// Appends events in one transaction: either all of them are stored or
    /// none is. Returns their sequence numbers, in order.
    pub fn append(&mut self, events: &[Event]) -> Result<Vec<Seq>, StoreError> {
        // Serialize first so a serialization error never opens a transaction.
        let rows = events
            .iter()
            .map(Row::encode)
            .collect::<Result<Vec<_>, _>>()?;
        let transaction = self.connection.transaction()?;
        let mut seqs = Vec::with_capacity(rows.len());
        {
            let mut insert = transaction.prepare_cached(
                "INSERT INTO events (at, kind, agent, payload) VALUES (?1, ?2, ?3, ?4) \
                 RETURNING seq",
            )?;
            for row in &rows {
                let seq: Seq = insert
                    .query_row(params![row.at, row.kind, row.agent, row.payload], |r| {
                        r.get(0)
                    })?;
                seqs.push(seq);
            }
        }
        transaction.commit()?;
        Ok(seqs)
    }

    /// The events after `after` (all of them when `None`), oldest first.
    ///
    /// Fails on the first payload that cannot be read, naming its sequence
    /// number.
    pub fn events_since(&self, after: Option<Seq>) -> Result<Vec<(Seq, Event)>, StoreError> {
        let mut events = Vec::new();
        self.for_each_since(after, |seq, event| {
            events.push((seq, event));
            Ok(())
        })?;
        Ok(events)
    }

    /// The sequence number of the newest event, if any.
    pub fn last_seq(&self) -> Result<Option<Seq>, StoreError> {
        let last: Option<Option<Seq>> = self
            .connection
            .query_row("SELECT max(seq) FROM events", [], |row| row.get(0))
            .optional()?;
        Ok(last.flatten())
    }

    /// The number of events in the log.
    pub fn count(&self) -> Result<u64, StoreError> {
        let count: i64 = self
            .connection
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Rebuilds the current state by applying every event in order, and
    /// returns it with the sequence number of the last event applied.
    ///
    /// An event that does not apply means the log is corrupted: the error
    /// names it, nothing is skipped.
    pub fn load_registry(&self) -> Result<(Registry, Option<Seq>), StoreError> {
        let mut registry = Registry::new();
        let mut last = None;
        self.for_each_since(None, |seq, event| {
            registry
                .apply(&event)
                .map_err(|source| StoreError::Corrupted { seq, source })?;
            last = Some(seq);
            Ok(())
        })?;
        Ok((registry, last))
    }

    /// A small daemon setting, such as the version that created the database.
    pub fn get_meta(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .connection
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    /// Stores or replaces a small daemon setting.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    /// Streams decoded events to `visit`, stopping at the first error.
    fn for_each_since(
        &self,
        after: Option<Seq>,
        mut visit: impl FnMut(Seq, Event) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT seq, payload FROM events WHERE seq > ?1 ORDER BY seq")?;
        let mut rows = statement.query([after.unwrap_or(Seq::new(0))])?;
        while let Some(row) = rows.next()? {
            let seq: Seq = row.get(0)?;
            let payload: String = row.get(1)?;
            let event = serde_json::from_str(&payload)
                .map_err(|source| StoreError::UnreadableEvent { seq, source })?;
            visit(seq, event)?;
        }
        Ok(())
    }
}

/// An event prepared for insertion.
struct Row {
    at: i64,
    kind: String,
    agent: Option<String>,
    payload: String,
}

impl Row {
    fn encode(event: &Event) -> Result<Self, StoreError> {
        let value = serde_json::to_value(event).map_err(StoreError::Serialize)?;
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or(StoreError::MissingTag)?
            .to_owned();
        Ok(Self {
            at: event.at.as_millis(),
            kind,
            agent: agent_of(&event.kind).map(|id| id.to_string()),
            payload: value.to_string(),
        })
    }
}

/// The agent an event is about, when it names exactly one. Exhaustive on
/// purpose: a new event kind must decide whether it belongs to an agent.
fn agent_of(kind: &EventKind) -> Option<AgentId> {
    match kind {
        EventKind::AgentRegistered { agent, .. }
        | EventKind::SessionLinked { agent, .. }
        | EventKind::StateChanged { agent, .. }
        | EventKind::ActivityRecorded { agent, .. }
        | EventKind::WorkspaceUpdated { agent, .. }
        | EventKind::LocationChanged { agent, .. }
        | EventKind::ModelChanged { agent, .. }
        | EventKind::DeliveryUpdated { agent, .. }
        | EventKind::AgentForgotten { agent }
        | EventKind::AgentRenamed { agent, .. }
        | EventKind::SessionReleased { agent, .. }
        | EventKind::AgentExpired { agent }
        | EventKind::StopRequested { agent }
        | EventKind::MembershipChanged { agent, .. } => Some(*agent),
        EventKind::WorkerAssessed { worker, .. } => Some(*worker),
        EventKind::ThreadOpened { .. }
        | EventKind::MessagePosted { .. }
        | EventKind::MessageDelivered { .. }
        | EventKind::MessageRead { .. }
        | EventKind::MissionCreated { .. }
        | EventKind::MissionUpdated { .. } => None,
    }
}

/// Creates the database file with owner-only permissions before SQLite opens
/// it, so it is never readable by others, not even briefly. SQLite gives its
/// `-wal` and `-shm` files the mode of the main file.
fn create_private_file(path: &Path) -> Result<(), StoreError> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map(drop)
        .map_err(|source| StoreError::io_at("create database", path, source))?;
    #[cfg(unix)]
    {
        // Also tightens a file created by an older or foreign process.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|source| StoreError::io_at("restrict permissions of", path, source))?;
    }
    Ok(())
}
