//! The hook events Claude Code posts to the daemon (v2, step 1).
//!
//! Each event is stored as it arrived: the raw JSON payload, plus the few
//! columns that filter and index it. Rows are never updated or deleted, and
//! nothing here interprets a payload: what an event means is decided later,
//! from the raw text.

use rusqlite::params;

use crate::error::StoreError;
use crate::store::Store;

/// A hook event to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewHookEvent {
    /// When the daemon received it, in milliseconds since the Unix epoch.
    pub received_at: i64,
    /// The hook event name, such as `PreToolUse`.
    pub event: String,
    /// The Claude Code session it belongs to, when the payload names one.
    pub session_id: Option<String>,
    /// The working directory the payload names, when it does.
    pub cwd: Option<String>,
    /// The request body, exactly as received.
    pub payload: String,
}

/// A stored hook event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookEvent {
    /// Its sequence number: increases with arrival order.
    pub seq: i64,
    /// When the daemon received it, in milliseconds since the Unix epoch.
    pub received_at: i64,
    /// The hook event name.
    pub event: String,
    /// The session, when known.
    pub session_id: Option<String>,
    /// The working directory, when known.
    pub cwd: Option<String>,
    /// The request body, exactly as received.
    pub payload: String,
}

/// Which stored hook events to read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookFilter {
    /// Only this session.
    pub session_id: Option<String>,
    /// Only the newest `limit` events (still returned oldest first).
    pub limit: Option<u32>,
}

impl Store {
    /// Stores a hook event and returns its sequence number.
    pub fn append_hook_event(&self, event: &NewHookEvent) -> Result<i64, StoreError> {
        let seq = self.connection.query_row(
            "INSERT INTO hook_events (received_at, event, session_id, cwd, payload) \
             VALUES (?1, ?2, ?3, ?4, ?5) RETURNING seq",
            params![
                event.received_at,
                event.event,
                event.session_id,
                event.cwd,
                event.payload
            ],
            |row| row.get(0),
        )?;
        Ok(seq)
    }

    /// The stored hook events matching `filter`, oldest first.
    pub fn hook_events(&self, filter: &HookFilter) -> Result<Vec<HookEvent>, StoreError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT seq, received_at, event, session_id, cwd, payload FROM ( \
                 SELECT * FROM hook_events \
                 WHERE (?1 IS NULL OR session_id = ?1) \
                 ORDER BY seq DESC LIMIT ?2 \
             ) ORDER BY seq ASC",
        )?;
        let limit = filter.limit.map_or(-1, i64::from);
        let rows = statement.query_map(params![filter.session_id, limit], |row| {
            Ok(HookEvent {
                seq: row.get(0)?,
                received_at: row.get(1)?,
                event: row.get(2)?,
                session_id: row.get(3)?,
                cwd: row.get(4)?,
                payload: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, session: Option<&str>, at: i64) -> NewHookEvent {
        NewHookEvent {
            received_at: at,
            event: name.into(),
            session_id: session.map(str::to_owned),
            cwd: Some("/work/alpha".into()),
            payload: format!("{{\"hook_event_name\":\"{name}\"}}"),
        }
    }

    #[test]
    fn events_come_back_in_arrival_order_with_their_raw_payload() {
        let store = Store::open_in_memory().unwrap();
        let first = store
            .append_hook_event(&event("SessionStart", Some("s1"), 10))
            .unwrap();
        let second = store
            .append_hook_event(&event("Stop", Some("s1"), 20))
            .unwrap();
        assert!(first < second);
        let stored = store.hook_events(&HookFilter::default()).unwrap();
        let names: Vec<&str> = stored.iter().map(|e| e.event.as_str()).collect();
        assert_eq!(names, ["SessionStart", "Stop"]);
        assert_eq!(stored[0].payload, "{\"hook_event_name\":\"SessionStart\"}");
        assert_eq!(stored[0].cwd.as_deref(), Some("/work/alpha"));
    }

    #[test]
    fn a_filter_keeps_one_session_and_the_newest_events() {
        let store = Store::open_in_memory().unwrap();
        for (name, session, at) in [
            ("SessionStart", Some("s1"), 1),
            ("SessionStart", Some("s2"), 2),
            ("PreToolUse", Some("s1"), 3),
            ("Stop", Some("s1"), 4),
            ("Notification", None, 5),
        ] {
            store.append_hook_event(&event(name, session, at)).unwrap();
        }
        let one = store
            .hook_events(&HookFilter {
                session_id: Some("s1".into()),
                limit: None,
            })
            .unwrap();
        assert_eq!(one.len(), 3);
        let newest = store
            .hook_events(&HookFilter {
                session_id: Some("s1".into()),
                limit: Some(2),
            })
            .unwrap();
        let names: Vec<&str> = newest.iter().map(|e| e.event.as_str()).collect();
        assert_eq!(names, ["PreToolUse", "Stop"], "newest two, oldest first");
        let all = store.hook_events(&HookFilter::default()).unwrap();
        assert_eq!(all.len(), 5);
    }
}
