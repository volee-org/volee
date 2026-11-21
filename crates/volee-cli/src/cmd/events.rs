//! `volee events`: the hook events the daemon stored, oldest first.
//!
//! It reads the database directly (SQLite lets it read while the daemon
//! writes), so it works whether or not the daemon runs.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use volee_store::{HookEvent, HookFilter, Store};

use crate::connect::paths;
use crate::text;

/// Prints the stored hook events, as a table or, with `json`, as JSON.
pub(crate) fn run(session: Option<String>, limit: Option<u32>, json: bool) -> Result<()> {
    let database = paths()?.database();
    if !database.exists() {
        println!("No events yet: the daemon has not stored any. See `volee doctor`.");
        return Ok(());
    }
    let store =
        Store::open(&database).with_context(|| format!("cannot open {}", database.display()))?;
    let events = store.hook_events(&HookFilter {
        session_id: session,
        limit,
    })?;
    if json {
        let list: Vec<Value> = events.iter().map(as_json).collect();
        println!("{}", serde_json::to_string_pretty(&list)?);
    } else if events.is_empty() {
        println!("No events match.");
    } else {
        for event in &events {
            println!("{}", line(event));
        }
    }
    Ok(())
}

fn as_json(event: &HookEvent) -> Value {
    json!({
        "seq": event.seq,
        "received_at": event.received_at,
        "event": event.event,
        "session_id": event.session_id,
        "cwd": event.cwd,
        "payload": serde_json::from_str::<Value>(&event.payload).unwrap_or(Value::Null),
    })
}

/// One line: sequence, UTC time, event, short session id, and what the
/// payload says in a few words.
fn line(event: &HookEvent) -> String {
    let session = event
        .session_id
        .as_deref()
        .map_or("-".to_owned(), |id| id.chars().take(8).collect());
    let detail = detail(event);
    format!(
        "{:>4}  {}  {:<18} {}  {}",
        event.seq,
        utc(event.received_at),
        text::printable(&event.event),
        session,
        detail
    )
    .trim_end()
    .to_owned()
}

fn detail(event: &HookEvent) -> String {
    let Ok(payload) = serde_json::from_str::<Value>(&event.payload) else {
        return String::new();
    };
    let field = |key: &str| payload.get(key).and_then(Value::as_str);
    let text = match event.event.as_str() {
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "PermissionRequest"
        | "PermissionDenied" => {
            let tool = field("tool_name").unwrap_or("?");
            let input = payload.get("tool_input");
            let what = input
                .and_then(|input| {
                    ["command", "file_path", "pattern", "url", "description"]
                        .iter()
                        .find_map(|key| input.get(key).and_then(Value::as_str))
                })
                .unwrap_or("");
            format!("{tool} {what}")
        }
        "UserPromptSubmit" => field("prompt").unwrap_or("").to_owned(),
        "Notification" => field("message").unwrap_or("").to_owned(),
        "SessionStart" => field("source").unwrap_or("").to_owned(),
        "SessionEnd" => field("reason").unwrap_or("").to_owned(),
        "Stop" => field("last_assistant_message").unwrap_or("").to_owned(),
        "SubagentStart" | "SubagentStop" => field("agent_type").unwrap_or("").to_owned(),
        "TaskCreated" | "TaskCompleted" => field("task_subject").unwrap_or("").to_owned(),
        _ => String::new(),
    };
    shorten(&text::one_line(&text), 70)
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}\u{2026}")
    }
}

/// `YYYY-MM-DD HH:MM:SS` in UTC, from milliseconds since the Unix epoch.
fn utc(millis: i64) -> String {
    let seconds = millis.div_euclid(1_000);
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, payload: &str) -> HookEvent {
        HookEvent {
            seq: 3,
            received_at: 1_700_000_000_000,
            event: name.into(),
            session_id: Some("0123456789abcdef".into()),
            cwd: Some("/work".into()),
            payload: payload.into(),
        }
    }

    #[test]
    fn times_are_utc_civil_times() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(1_700_000_000_000), "2023-11-14 22:13:20");
        assert_eq!(utc(951_782_400_000), "2000-02-29 00:00:00");
        assert_eq!(utc(1_785_000_000_123), "2026-07-25 17:20:00");
    }

    #[test]
    fn a_tool_event_says_the_tool_and_what_it_acts_on() {
        let line = line(&event(
            "PreToolUse",
            r#"{"tool_name":"Bash","tool_input":{"command":"ls -la"}}"#,
        ));
        assert!(line.contains("PreToolUse"), "{line}");
        assert!(line.contains("01234567"), "{line}");
        assert!(line.ends_with("Bash ls -la"), "{line}");
        assert!(line.contains("2023-11-14 22:13:20"), "{line}");
    }

    #[test]
    fn a_prompt_is_cut_to_one_short_line() {
        let long = "word ".repeat(40);
        let line = line(&event(
            "UserPromptSubmit",
            &format!(r#"{{"prompt":"{long}"}}"#),
        ));
        assert!(line.ends_with('\u{2026}'), "{line}");
        assert!(!line.contains('\n'));
    }

    #[test]
    fn an_unreadable_payload_still_gives_a_line() {
        assert!(line(&event("Stop", "not json")).contains("Stop"));
    }
}
