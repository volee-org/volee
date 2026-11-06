//! Integration tests of the event log.

mod common;

use rusqlite::Connection;
use volee_core::Registry;
use volee_store::{Seq, Store, StoreError};

use common::{agent, one_of_each_kind};

#[test]
fn the_fixture_covers_every_event_kind() {
    let events = one_of_each_kind();
    let mut kinds: Vec<String> = events
        .iter()
        .map(|e| {
            serde_json::to_value(e).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    kinds.sort();
    kinds.dedup();
    // Update this number together with `EventKind`.
    assert_eq!(kinds.len(), 15);
}

#[test]
fn every_event_kind_round_trips() {
    let events = one_of_each_kind();
    let mut store = Store::open_in_memory().unwrap();
    let seqs = store.append(&events).unwrap();
    assert_eq!(seqs.len(), events.len());
    let back = store.events_since(None).unwrap();
    assert_eq!(back.len(), events.len());
    for ((seq, event), (expected_seq, expected)) in back.iter().zip(seqs.iter().zip(&events)) {
        assert_eq!(seq, expected_seq);
        assert_eq!(event, expected);
    }
}

#[test]
fn sequence_numbers_increase_across_appends() {
    let mut store = Store::open_in_memory().unwrap();
    let events = one_of_each_kind();
    let first = store.append(&events[..3]).unwrap();
    let second = store.append(&events[3..6]).unwrap();
    let all: Vec<Seq> = first.iter().chain(&second).copied().collect();
    assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(all[0], Seq::new(1));
    assert_eq!(store.last_seq().unwrap(), Some(Seq::new(6)));
    assert_eq!(store.count().unwrap(), 6);
}

#[test]
fn an_empty_log_has_no_last_seq() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.last_seq().unwrap(), None);
    assert_eq!(store.count().unwrap(), 0);
    assert_eq!(store.events_since(None).unwrap().len(), 0);
}

#[test]
fn events_since_returns_only_newer_events() {
    let mut store = Store::open_in_memory().unwrap();
    let events = one_of_each_kind();
    let seqs = store.append(&events).unwrap();
    let tail = store.events_since(Some(seqs[9])).unwrap();
    assert_eq!(tail.len(), events.len() - 10);
    assert_eq!(tail[0].0, seqs[10]);
    assert_eq!(store.events_since(seqs.last().copied()).unwrap().len(), 0);
}

#[test]
fn append_of_nothing_is_a_no_op() {
    let mut store = Store::open_in_memory().unwrap();
    assert_eq!(store.append(&[]).unwrap().len(), 0);
    assert_eq!(store.count().unwrap(), 0);
}

#[test]
fn load_registry_matches_applying_in_memory() {
    let events = one_of_each_kind();
    // Stop before the agent is forgotten so the registry has content.
    let events = &events[..events.len() - 1];
    let mut store = Store::open_in_memory().unwrap();
    store.append(events).unwrap();
    let (loaded, last) = store.load_registry().unwrap();
    let expected = Registry::from_events(events).unwrap();
    assert_eq!(last, Some(Seq::new(events.len() as u64)));
    assert_eq!(
        loaded.agents().collect::<Vec<_>>(),
        expected.agents().collect::<Vec<_>>()
    );
    assert_eq!(
        loaded.missions().collect::<Vec<_>>(),
        expected.missions().collect::<Vec<_>>()
    );
    assert_eq!(
        loaded.threads().collect::<Vec<_>>(),
        expected.threads().collect::<Vec<_>>()
    );
    assert!(loaded.agent(agent()).is_some());
}

#[test]
fn load_registry_of_an_empty_log_is_empty() {
    let store = Store::open_in_memory().unwrap();
    let (registry, last) = store.load_registry().unwrap();
    assert_eq!(last, None);
    assert_eq!(registry.agents().count(), 0);
}

#[test]
fn an_event_that_does_not_apply_names_its_seq() {
    let mut events = one_of_each_kind();
    // A second registration of the same agent is a duplicate.
    let duplicate = events[2].clone();
    events.insert(3, duplicate);
    let mut store = Store::open_in_memory().unwrap();
    store.append(&events).unwrap();
    let error = store.load_registry().unwrap_err();
    match error {
        StoreError::Corrupted { seq, .. } => assert_eq!(seq, Seq::new(4)),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn an_unreadable_payload_names_its_seq() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("volee.db");
    let mut store = Store::open(&path).unwrap();
    store.append(&one_of_each_kind()[..2]).unwrap();
    // Simulate an event written by a newer volee.
    Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO events (at, kind, agent, payload) VALUES (9, 'from-the-future', NULL, \
             '{\"at\":9,\"type\":\"from-the-future\"}')",
            [],
        )
        .unwrap();
    let error = store.events_since(None).unwrap_err();
    assert!(
        matches!(error, StoreError::UnreadableEvent { seq, .. } if seq == Seq::new(3)),
        "{error}"
    );
    let message = error.to_string();
    assert!(
        message.contains("#3") && message.contains("newer volee"),
        "{message}"
    );
    assert!(matches!(
        store.load_registry().unwrap_err(),
        StoreError::UnreadableEvent { .. }
    ));
}

#[test]
fn columns_index_kind_agent_and_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("volee.db");
    let mut store = Store::open(&path).unwrap();
    store.append(&one_of_each_kind()).unwrap();
    let connection = Connection::open(&path).unwrap();
    let by_agent: i64 = connection
        .query_row(
            "SELECT count(*) FROM events WHERE agent = ?1",
            [agent().to_string()],
            |row| row.get(0),
        )
        .unwrap();
    // Every kind that names an agent: nine of the fifteen.
    assert_eq!(by_agent, 9);
    let (kind, at): (String, i64) = connection
        .query_row("SELECT kind, at FROM events WHERE seq = 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(kind, "thread-opened");
    assert_eq!(at, 1);
}

#[test]
fn meta_stores_and_replaces_settings() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.get_meta("creator").unwrap(), None);
    store.set_meta("creator", "0.1.0").unwrap();
    assert_eq!(store.get_meta("creator").unwrap().as_deref(), Some("0.1.0"));
    store.set_meta("creator", "0.2.0").unwrap();
    assert_eq!(store.get_meta("creator").unwrap().as_deref(), Some("0.2.0"));
}

#[test]
fn reopening_keeps_data_and_does_not_remigrate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/deeper/volee.db");
    {
        let mut store = Store::open(&path).unwrap();
        store.append(&one_of_each_kind()).unwrap();
        store.set_meta("k", "v").unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.count().unwrap(), 15);
    assert_eq!(store.get_meta("k").unwrap().as_deref(), Some("v"));
    let version: u32 = Connection::open(&path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
}

#[test]
fn a_database_from_a_newer_volee_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("volee.db");
    drop(Store::open(&path).unwrap());
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let error = Store::open(&path).unwrap_err();
    assert!(
        matches!(error, StoreError::SchemaTooNew { found: 99, .. }),
        "{error}"
    );
}

#[test]
fn the_database_runs_in_wal_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("volee.db");
    drop(Store::open(&path).unwrap());
    let mode: String = Connection::open(&path)
        .unwrap()
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}

#[cfg(unix)]
#[test]
fn the_database_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("volee.db");
    let mut store = Store::open(&path).unwrap();
    store.append(&one_of_each_kind()).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    // A pre-existing file with loose permissions is tightened.
    drop(store);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    drop(Store::open(&path).unwrap());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_failed_batch_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("volee.db");
    let mut store = Store::open(&path).unwrap();
    store.append(&one_of_each_kind()[..1]).unwrap();
    // A trigger that refuses state changes makes the batch fail midway.
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse BEFORE INSERT ON events WHEN NEW.kind = 'state-changed' \
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
    let error = store.append(&one_of_each_kind()[1..6]).unwrap_err();
    assert!(matches!(error, StoreError::Sqlite(_)), "{error}");
    assert_eq!(store.count().unwrap(), 1);
    assert_eq!(store.last_seq().unwrap(), Some(Seq::new(1)));
}
