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
    // 2: the hook events Claude Code posts to the daemon (v2).
    "
    CREATE TABLE hook_events (
        seq         INTEGER PRIMARY KEY AUTOINCREMENT,
        received_at INTEGER NOT NULL,
        event       TEXT    NOT NULL,
        session_id  TEXT,
        cwd         TEXT,
        payload     TEXT    NOT NULL
    );
    CREATE INDEX hook_events_session ON hook_events (session_id, seq)
        WHERE session_id IS NOT NULL;
    ",
    // 3: what the agents view remembers about branches.
    "
    CREATE TABLE branch_memory (
        repo         TEXT    NOT NULL,
        branch       TEXT    NOT NULL,
        unmerged_tip TEXT    NOT NULL,
        updated_at   INTEGER NOT NULL,
        PRIMARY KEY (repo, branch)
    );
    CREATE TABLE pull_request_cache (
        repo       TEXT    NOT NULL,
        branch     TEXT    NOT NULL,
        tip        TEXT    NOT NULL,
        state      TEXT    NOT NULL,
        number     INTEGER,
        url        TEXT,
        checked_at INTEGER NOT NULL,
        PRIMARY KEY (repo, branch, tip)
    );
    ",
    // 4: the AI pre-reviews of a branch tip and their findings (v2).
    "
    CREATE TABLE reviews (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        repo         TEXT    NOT NULL,
        branch       TEXT    NOT NULL,
        tip          TEXT    NOT NULL,
        base         TEXT    NOT NULL,
        status       TEXT    NOT NULL,
        reviewers    INTEGER NOT NULL,
        model        TEXT    NOT NULL,
        requested_at INTEGER NOT NULL,
        started_at   INTEGER,
        finished_at  INTEGER,
        error        TEXT,
        raw          TEXT,
        cost_usd     REAL,
        UNIQUE (repo, branch, tip)
    );
    CREATE TABLE review_findings (
        review_id        INTEGER NOT NULL REFERENCES reviews (id) ON DELETE CASCADE,
        position         INTEGER NOT NULL,
        file             TEXT    NOT NULL,
        line             INTEGER,
        severity         TEXT    NOT NULL,
        summary          TEXT    NOT NULL,
        failure_scenario TEXT,
        PRIMARY KEY (review_id, position)
    );
    ",
    // 5: the risk a person set by hand on a branch tip (v2, step 8). It is
    // per tip: a new commit is judged by the rules again.
    "
    CREATE TABLE risk_overrides (
        repo   TEXT    NOT NULL,
        tip    TEXT    NOT NULL,
        level  TEXT    NOT NULL,
        set_at INTEGER NOT NULL,
        PRIMARY KEY (repo, tip)
    );
    ",
    // 6: what the person does on a finished change (v2, step 9): the line
    // comments, per branch tip, and the journal of actions.
    "
    CREATE TABLE line_comments (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        repo       TEXT    NOT NULL,
        tip        TEXT    NOT NULL,
        file       TEXT    NOT NULL,
        line       INTEGER NOT NULL,
        text       TEXT    NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX line_comments_tip ON line_comments (repo, tip);
    CREATE TABLE journal (
        id        INTEGER PRIMARY KEY AUTOINCREMENT,
        at        INTEGER NOT NULL,
        repo      TEXT    NOT NULL,
        branch    TEXT    NOT NULL,
        tip       TEXT    NOT NULL,
        kind      TEXT    NOT NULL,
        ok        INTEGER NOT NULL,
        automatic INTEGER NOT NULL,
        message   TEXT    NOT NULL
    );
    CREATE INDEX journal_tip ON journal (repo, tip, kind);
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
