//! Schema migrations, tracked with `PRAGMA user_version`.
//!
//! # Adding a migration
//!
//! Append one SQL script to [`MIGRATIONS`]. Never edit or reorder an entry
//! that has shipped: databases in the wild already ran it. The database
//! version is the number of scripts applied, so the new script runs once, in
//! its own transaction, the next time a store is opened. Add a test in this
//! module that checks the new shape.

use rusqlite::Connection;

use crate::error::StoreError;

/// Ordered, append-only list of schema scripts. Entry `n` upgrades the
/// database from version `n` to version `n + 1`.
const MIGRATIONS: &[&str] = &[
    // 1: the event log and the settings table.
    "
    CREATE TABLE events (
        seq     INTEGER PRIMARY KEY AUTOINCREMENT,
        at      INTEGER NOT NULL,
        kind    TEXT    NOT NULL,
        agent   TEXT,
        payload TEXT    NOT NULL
    );
    CREATE INDEX events_agent ON events (agent, seq) WHERE agent IS NOT NULL;
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    ",
];

/// The newest schema version this build understands.
pub(crate) fn latest() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}

/// Brings the database to [`latest`]. Safe to call on every open.
pub(crate) fn run(connection: &mut Connection) -> Result<(), StoreError> {
    let current: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current > latest() {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported: latest(),
        });
    }
    for (index, script) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let transaction = connection.transaction()?;
        transaction.execute_batch(script)?;
        // `PRAGMA` cannot take bound parameters; `version` is a local integer.
        transaction.pragma_update(None, "user_version", version)?;
        transaction.commit()?;
        tracing::debug!(version, "applied schema migration");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(connection: &Connection) -> u32 {
        connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn a_fresh_database_reaches_the_latest_version() {
        let mut connection = Connection::open_in_memory().unwrap();
        run(&mut connection).unwrap();
        assert_eq!(version(&connection), latest());
    }

    #[test]
    fn running_twice_keeps_the_data() {
        let mut connection = Connection::open_in_memory().unwrap();
        run(&mut connection).unwrap();
        connection
            .execute("INSERT INTO meta (key, value) VALUES ('k', 'v')", [])
            .unwrap();
        run(&mut connection).unwrap();
        let value: String = connection
            .query_row("SELECT value FROM meta WHERE key = 'k'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "v");
    }

    #[test]
    fn a_newer_database_is_refused() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.pragma_update(None, "user_version", 999).unwrap();
        let error = run(&mut connection).unwrap_err();
        assert!(matches!(error, StoreError::SchemaTooNew { found: 999, .. }));
    }

    #[test]
    fn a_failed_script_rolls_back_entirely() {
        let mut connection = Connection::open_in_memory().unwrap();
        let transaction = connection.transaction().unwrap();
        transaction.execute_batch("CREATE TABLE t (x);").unwrap();
        assert!(transaction.execute_batch("CREATE TABLE t (x);").is_err());
        drop(transaction);
        let tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 't'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
    }
}
