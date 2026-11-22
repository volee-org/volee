//! Held decisions (v2, steps 4 and 5): the permission requests and the
//! questions Claude Code asks through `http` hooks, kept open until the
//! person decides in the browser.
//!
//! A hook request that asks for a decision is **held**: the handler waits on
//! a channel, a card appears in the inbox, and the request is answered with
//! whatever the person chose. Claude Code shows its own dialog at the same
//! time for a permission (the hook is a concurrent decider), so a held
//! request can become pointless at any moment. It is dropped, and the card
//! with it, when:
//!
//! - the person decides (the answer is sent),
//! - the hold runs out (nothing is answered: Claude Code asks in its
//!   terminal, which is also what it does when its own hook timeout is hit),
//! - Claude Code closes the connection (the handler future is dropped),
//! - a later hook event of the same session shows the terminal was answered
//!   first (`PostToolUse`, `PermissionDenied`, the next `PreToolUse`, `Stop`...).
//!
//! Claude Code gives a permission request no id, so the match is made on the
//! session, the tool name and the tool input.
//!
//! When the daemon stops while holding, the connection drops: Claude Code
//! treats it like a failed hook and asks in the terminal. Nothing is lost,
//! nothing is decided.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};

/// How long each kind of request is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldSettings {
    /// A permission request: the hook's own timeout is the same value.
    pub permission_timeout: Duration,
    /// A question, before the terminal asks it.
    pub question_delay: Duration,
}

impl Default for HoldSettings {
    fn default() -> Self {
        Self {
            permission_timeout: Duration::from_secs(volee_config::DEFAULT_PERMISSION_TIMEOUT),
            question_delay: volee_rules::Settings::default().held_question_timeout(),
        }
    }
}

/// What a held request asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DecisionKind {
    /// A tool wants permission.
    Permission,
    /// The agent asks the person to choose or write.
    Question,
}

/// A permission rule Claude Code suggests ("always allow ...").
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Suggestion {
    pub(crate) index: usize,
    pub(crate) label: String,
}

/// A held request as the interface shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct DecisionCard {
    pub(crate) id: String,
    pub(crate) kind: DecisionKind,
    pub(crate) session_id: String,
    pub(crate) tool_name: String,
    /// One line that says what is asked (the command, the file).
    pub(crate) summary: String,
    pub(crate) tool_input: Value,
    pub(crate) suggestions: Vec<Suggestion>,
    /// For a question, the questions as the agent wrote them.
    pub(crate) questions: Value,
    /// When the request arrived, in milliseconds since the Unix epoch.
    pub(crate) created_at: i64,
    /// When the hold ends and the terminal takes over.
    pub(crate) expires_at: i64,
}

/// What a hook handler asks to hold.
#[derive(Clone, Debug)]
pub(crate) struct Held {
    pub(crate) kind: DecisionKind,
    pub(crate) session_id: String,
    pub(crate) tool_name: String,
    pub(crate) tool_input: Value,
    /// The `permission_suggestions` of the request, verbatim.
    pub(crate) suggestions: Vec<Value>,
}

/// What the person chose, as the interface posts it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum Decision {
    /// Allow a permission, optionally with one of the suggested rules.
    Allow { suggestion: Option<usize> },
    /// Refuse a permission or a question, with a reason the model sees.
    Deny { message: Option<String> },
    /// Answer a question: the answer to each question, by its text.
    Answer { answers: BTreeMap<String, String> },
    /// Let the terminal ask.
    Terminal,
}

/// Why a decision was not applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DecideError {
    /// Decided elsewhere, expired or gone.
    Gone,
    /// The decision does not fit the kind of card, or names something the
    /// card does not have.
    Invalid(String),
}

/// What the held request is answered with.
enum Outcome {
    /// A JSON body for Claude Code.
    Answer(Value),
    /// Nothing: Claude Code asks in the terminal.
    HandBack,
}

struct Pending {
    card: DecisionCard,
    raw_suggestions: Vec<Value>,
    reply: oneshot::Sender<Outcome>,
}

/// The held requests, and who wants to know when they change.
pub(crate) struct Decisions {
    pending: Mutex<Vec<Pending>>,
    changes: watch::Sender<u64>,
    settings: HoldSettings,
}

impl std::fmt::Debug for Decisions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decisions").finish_non_exhaustive()
    }
}

/// Removes a held request when its handler is dropped, whatever the reason.
struct Guard {
    decisions: Arc<Decisions>,
    id: String,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.decisions.remove(&self.id);
    }
}

/// A hook event, as far as the held requests care.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Observed<'a> {
    pub(crate) event: &'a str,
    pub(crate) session_id: Option<&'a str>,
    pub(crate) tool_name: Option<&'a str>,
    pub(crate) tool_input: Option<&'a Value>,
}

impl Decisions {
    pub(crate) fn new(settings: HoldSettings) -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(Vec::new()),
            changes: watch::channel(0).0,
            settings,
        })
    }

    /// A receiver that wakes each time a card appears or goes.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// The cards, oldest first.
    pub(crate) fn cards(&self) -> Vec<DecisionCard> {
        let pending = self.lock();
        pending.iter().map(|p| p.card.clone()).collect()
    }

    /// Holds a request until the person decides, the hold runs out, or the
    /// caller is dropped. `Some(body)` is what to answer; `None` is "nothing
    /// to say", an empty `200`.
    pub(crate) async fn hold(self: &Arc<Self>, held: Held) -> Option<Value> {
        let limit = match held.kind {
            DecisionKind::Permission => self.settings.permission_timeout,
            DecisionKind::Question => self.settings.question_delay,
        };
        let now = now_millis();
        let id = ulid::Ulid::generate().to_string();
        let (reply, answer) = oneshot::channel();
        let card = DecisionCard {
            id: id.clone(),
            kind: held.kind,
            session_id: held.session_id,
            summary: summarize(&held.tool_name, &held.tool_input),
            suggestions: held
                .suggestions
                .iter()
                .enumerate()
                .map(|(index, entry)| Suggestion {
                    index,
                    label: suggestion_label(entry),
                })
                .collect(),
            questions: match held.kind {
                DecisionKind::Question => held
                    .tool_input
                    .get("questions")
                    .cloned()
                    .unwrap_or(Value::Null),
                DecisionKind::Permission => Value::Null,
            },
            tool_name: held.tool_name,
            tool_input: held.tool_input,
            created_at: now,
            expires_at: now.saturating_add(i64::try_from(limit.as_millis()).unwrap_or(i64::MAX)),
        };
        self.lock().push(Pending {
            card,
            raw_suggestions: held.suggestions,
            reply,
        });
        self.changed();
        let _guard = Guard {
            decisions: Arc::clone(self),
            id,
        };
        match tokio::time::timeout(limit, answer).await {
            Ok(Ok(Outcome::Answer(body))) => Some(body),
            _ => None,
        }
    }

    /// Applies the person's decision to the card `id`.
    pub(crate) fn decide(&self, id: &str, decision: Decision) -> Result<(), DecideError> {
        let mut pending = self.lock();
        let position = pending
            .iter()
            .position(|p| p.card.id == id)
            .ok_or(DecideError::Gone)?;
        // Checked before the card leaves: a wrong decision keeps it open.
        let outcome = outcome_of(&pending[position], decision)?;
        let taken = pending.remove(position);
        drop(pending);
        self.changed();
        taken.reply.send(outcome).map_err(|_| DecideError::Gone)
    }

    /// Drops the held requests a hook event shows to be pointless: the
    /// terminal was answered first, or the turn moved on.
    pub(crate) fn observe(&self, seen: &Observed<'_>) {
        let Some(session) = seen.session_id else {
            return;
        };
        let mut gone = Vec::new();
        {
            let mut pending = self.lock();
            let mut index = 0;
            while index < pending.len() {
                if pending[index].card.session_id == session && settles(&pending[index], seen) {
                    gone.push(pending.remove(index));
                } else {
                    index += 1;
                }
            }
        }
        if gone.is_empty() {
            return;
        }
        self.changed();
        for taken in gone {
            // The terminal decided: answer nothing.
            let _ = taken.reply.send(Outcome::HandBack);
        }
    }

    fn remove(&self, id: &str) {
        let removed = {
            let mut pending = self.lock();
            let before = pending.len();
            pending.retain(|p| p.card.id != id);
            pending.len() != before
        };
        if removed {
            self.changed();
        }
    }

    fn changed(&self) {
        self.changes
            .send_modify(|count| *count = count.wrapping_add(1));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether `seen` makes the held request moot.
fn settles(pending: &Pending, seen: &Observed<'_>) -> bool {
    let card = &pending.card;
    let same_call = seen.tool_name == Some(card.tool_name.as_str())
        && seen.tool_input == Some(&card.tool_input);
    match seen.event {
        // The tool ran, failed or was refused: someone answered the dialog.
        "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => same_call,
        // The turn ended or the person typed in the terminal.
        "Stop" | "StopFailure" | "SessionEnd" | "UserPromptSubmit" => true,
        // Another tool is about to run: the dialog is behind us. A question
        // is held by a `PreToolUse` itself, so only permissions end here.
        "PreToolUse" => card.kind == DecisionKind::Permission && !same_call,
        _ => false,
    }
}

/// What a decision answers, or why it does not fit the card.
fn outcome_of(pending: &Pending, decision: Decision) -> Result<Outcome, DecideError> {
    let card = &pending.card;
    let invalid = |text: &str| Err(DecideError::Invalid(text.to_owned()));
    match (card.kind, decision) {
        (_, Decision::Terminal) => Ok(Outcome::HandBack),
        (DecisionKind::Permission, Decision::Allow { suggestion }) => {
            let mut decision = json!({ "behavior": "allow" });
            if let Some(index) = suggestion {
                let Some(entry) = pending.raw_suggestions.get(index) else {
                    return invalid("this request has no such suggestion");
                };
                decision["updatedPermissions"] = json!([entry]);
            }
            Ok(Outcome::Answer(json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": decision,
                }
            })))
        }
        (DecisionKind::Permission, Decision::Deny { message }) => Ok(Outcome::Answer(json!({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {
                    "behavior": "deny",
                    "message": message
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or_else(|| "Denied by the user in volee.".to_owned()),
                },
            }
        }))),
        (DecisionKind::Question, Decision::Deny { message }) => Ok(Outcome::Answer(json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": message
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| {
                        "The user declined to answer this question in volee.".to_owned()
                    }),
            }
        }))),
        (DecisionKind::Question, Decision::Answer { answers }) => {
            let asked = question_texts(&card.questions);
            for text in &asked {
                if answers.get(text).is_none_or(|a| a.trim().is_empty()) {
                    return Err(DecideError::Invalid(format!("no answer to: {text}")));
                }
            }
            if answers.keys().any(|key| !asked.contains(key)) {
                return invalid("an answer names a question that was not asked");
            }
            Ok(Outcome::Answer(json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "updatedInput": {
                        "questions": card.questions,
                        "answers": answers,
                    },
                }
            })))
        }
        (DecisionKind::Question, Decision::Allow { .. }) => {
            invalid("a question is answered, not allowed")
        }
        (DecisionKind::Permission, Decision::Answer { .. }) => {
            invalid("a permission is allowed or denied, not answered")
        }
    }
}

/// The text of each question, as the agent wrote it.
fn question_texts(questions: &Value) -> Vec<String> {
    questions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| q.get("question").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// One line that says what a tool call does.
pub(crate) fn summarize(tool: &str, input: &Value) -> String {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
    let line = match tool {
        "Bash" | "Monitor" => text("command"),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            text("file_path").or_else(|| text("notebook_path"))
        }
        "Glob" | "Grep" => text("pattern"),
        "WebFetch" => text("url"),
        "WebSearch" => text("query"),
        _ => None,
    };
    let line = line.unwrap_or_else(|| {
        input
            .as_object()
            .and_then(|object| object.values().find_map(Value::as_str))
            .map_or_else(|| input.to_string(), str::to_owned)
    });
    let first = line.lines().next().unwrap_or_default().trim();
    if first.chars().count() > 200 {
        let cut: String = first.chars().take(200).collect();
        format!("{cut}…")
    } else {
        first.to_owned()
    }
}

/// A suggested permission update, in words: "Always allow Bash(npm test)".
fn suggestion_label(entry: &Value) -> String {
    let kind = entry
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("update");
    let place = match entry.get("destination").and_then(Value::as_str) {
        Some("session") => " for this session",
        Some("localSettings") => " in this project (local)",
        Some("projectSettings") => " in this project",
        Some("userSettings") => " everywhere",
        _ => "",
    };
    match kind {
        "addRules" | "replaceRules" => {
            let behavior = entry
                .get("behavior")
                .and_then(Value::as_str)
                .unwrap_or("allow");
            let rules: Vec<String> = entry
                .get("rules")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|rule| {
                    let tool = rule.get("toolName").and_then(Value::as_str).unwrap_or("?");
                    match rule.get("ruleContent").and_then(Value::as_str) {
                        Some(content) => format!("{tool}({content})"),
                        None => tool.to_owned(),
                    }
                })
                .collect();
            let verb = if behavior == "allow" {
                "Always allow"
            } else {
                "Always"
            };
            format!("{verb} {}{place}", rules.join(", "))
        }
        "setMode" => format!(
            "Switch to {} mode{place}",
            entry
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("another")
        ),
        "addDirectories" => {
            let directories: Vec<&str> = entry
                .get("directories")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if directories.is_empty() {
                format!("Allow its directories{place}")
            } else {
                format!("Allow access to {}{place}", directories.join(", "))
            }
        }
        other => format!("{other}{place}"),
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    // In tests, a failed unwrap is a failed test with a clear location.
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn permission(session: &str, command: &str) -> Held {
        Held {
            kind: DecisionKind::Permission,
            session_id: session.to_owned(),
            tool_name: "Bash".to_owned(),
            tool_input: json!({ "command": command }),
            suggestions: vec![json!({
                "type": "addRules",
                "rules": [{ "toolName": "Bash", "ruleContent": command }],
                "behavior": "allow",
                "destination": "localSettings",
            })],
        }
    }

    fn question(session: &str) -> Held {
        Held {
            kind: DecisionKind::Question,
            session_id: session.to_owned(),
            tool_name: "AskUserQuestion".to_owned(),
            tool_input: json!({ "questions": [
                { "question": "Which color?", "options": [{ "label": "Red" }, { "label": "Blue" }] },
            ] }),
            suggestions: vec![],
        }
    }

    fn settings(permission_ms: u64, question_ms: u64) -> HoldSettings {
        HoldSettings {
            permission_timeout: Duration::from_millis(permission_ms),
            question_delay: Duration::from_millis(question_ms),
        }
    }

    /// Holds `held` in the background and waits until its card is there.
    async fn spawn_hold(
        decisions: &Arc<Decisions>,
        held: Held,
    ) -> tokio::task::JoinHandle<Option<Value>> {
        let before = decisions.cards().len();
        let task = tokio::spawn({
            let decisions = Arc::clone(decisions);
            async move { decisions.hold(held).await }
        });
        for _ in 0..200 {
            if decisions.cards().len() > before {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the card never appeared");
    }

    #[tokio::test]
    async fn allow_answers_the_held_request_and_clears_the_card() {
        let decisions = Decisions::new(settings(5_000, 5_000));
        let task = spawn_hold(&decisions, permission("s1", "npm test")).await;
        let card = decisions.cards().remove(0);
        assert_eq!(card.summary, "npm test");
        assert_eq!(
            card.suggestions[0].label,
            "Always allow Bash(npm test) in this project (local)"
        );
        decisions
            .decide(&card.id, Decision::Allow { suggestion: None })
            .unwrap();
        let body = task.await.unwrap().unwrap();
        assert_eq!(
            body["hookSpecificOutput"]["hookEventName"],
            "PermissionRequest"
        );
        assert_eq!(body["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert!(
            body["hookSpecificOutput"]["decision"]
                .get("updatedPermissions")
                .is_none()
        );
        assert_eq!(decisions.cards().len(), 0);
    }

    #[tokio::test]
    async fn allow_with_a_suggestion_echoes_that_suggestion() {
        let decisions = Decisions::new(settings(5_000, 5_000));
        let task = spawn_hold(&decisions, permission("s1", "ls")).await;
        let id = decisions.cards()[0].id.clone();
        assert!(matches!(
            decisions.decide(
                &id,
                Decision::Allow {
                    suggestion: Some(3)
                }
            ),
            Err(DecideError::Invalid(_))
        ));
        assert_eq!(
            decisions.cards().len(),
            1,
            "a wrong decision keeps the card"
        );
        decisions
            .decide(
                &id,
                Decision::Allow {
                    suggestion: Some(0),
                },
            )
            .unwrap();
        let body = task.await.unwrap().unwrap();
        let updated = &body["hookSpecificOutput"]["decision"]["updatedPermissions"];
        assert_eq!(updated[0]["rules"][0]["ruleContent"], "ls");
    }

    #[tokio::test]
    async fn deny_carries_the_message_the_model_sees() {
        let decisions = Decisions::new(settings(5_000, 5_000));
        let task = spawn_hold(&decisions, permission("s1", "rm -rf x")).await;
        let id = decisions.cards()[0].id.clone();
        decisions
            .decide(
                &id,
                Decision::Deny {
                    message: Some("too risky".into()),
                },
            )
            .unwrap();
        let body = task.await.unwrap().unwrap();
        assert_eq!(body["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert_eq!(
            body["hookSpecificOutput"]["decision"]["message"],
            "too risky"
        );
    }

    #[tokio::test]
    async fn a_decision_twice_or_after_the_hold_is_gone() {
        let decisions = Decisions::new(settings(5_000, 5_000));
        let task = spawn_hold(&decisions, permission("s1", "ls")).await;
        let id = decisions.cards()[0].id.clone();
        decisions.decide(&id, Decision::Terminal).unwrap();
        assert_eq!(
            task.await.unwrap(),
            None,
            "the terminal asks: nothing is answered"
        );
        assert_eq!(
            decisions.decide(&id, Decision::Terminal),
            Err(DecideError::Gone)
        );
    }

    #[tokio::test]
    async fn the_hold_runs_out_and_the_card_goes() {
        let decisions = Decisions::new(settings(60, 60));
        let task = spawn_hold(&decisions, permission("s1", "ls")).await;
        assert_eq!(task.await.unwrap(), None);
        assert_eq!(decisions.cards().len(), 0);
    }

    #[tokio::test]
    async fn a_dropped_handler_takes_its_card_with_it() {
        let decisions = Decisions::new(settings(5_000, 5_000));
        let task = spawn_hold(&decisions, permission("s1", "ls")).await;
        task.abort();
        let _ = task.await;
        assert_eq!(decisions.cards().len(), 0);
    }

    #[tokio::test]
    async fn the_terminal_answering_first_clears_the_card_and_answers_nothing() {
        let decisions = Decisions::new(settings(5_000, 5_000));
        let ran = json!({ "command": "ls" });
        for (event, tool, input, goes) in [
            ("PostToolUse", "Bash", Some(&ran), true),
            ("PostToolUseFailure", "Bash", Some(&ran), true),
            ("PermissionDenied", "Bash", Some(&ran), true),
            (
                "PostToolUse",
                "Bash",
                Some(&json!({ "command": "other" })),
                false,
            ),
            ("PostToolUse", "Read", Some(&ran), false),
            (
                "PreToolUse",
                "Bash",
                Some(&json!({ "command": "next" })),
                true,
            ),
            ("PreToolUse", "Bash", Some(&ran), false),
            ("Stop", "", None, true),
            ("UserPromptSubmit", "", None, true),
            ("SessionEnd", "", None, true),
            ("Notification", "", None, false),
        ] {
            let task = spawn_hold(&decisions, permission("s1", "ls")).await;
            decisions.observe(&Observed {
                event,
                session_id: Some("s1"),
                tool_name: Some(tool),
                tool_input: input,
            });
            assert_eq!(
                decisions.cards().is_empty(),
                goes,
                "{event} {tool} {input:?}"
            );
            if goes {
                assert_eq!(task.await.unwrap(), None);
            } else {
                let id = decisions.cards()[0].id.clone();
                decisions.decide(&id, Decision::Terminal).unwrap();
                task.await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn another_session_is_left_alone() {
        let decisions = Decisions::new(settings(5_000, 5_000));
        let task = spawn_hold(&decisions, permission("s1", "ls")).await;
        decisions.observe(&Observed {
            event: "Stop",
            session_id: Some("s2"),
            tool_name: None,
            tool_input: None,
        });
        assert_eq!(decisions.cards().len(), 1);
        let id = decisions.cards()[0].id.clone();
        decisions.decide(&id, Decision::Terminal).unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_question_is_answered_with_the_original_questions_echoed_back() {
        let decisions = Decisions::new(settings(5_000, 5_000));
        let task = spawn_hold(&decisions, question("s1")).await;
        let id = decisions.cards()[0].id.clone();
        let answer = |text: &str| Decision::Answer {
            answers: BTreeMap::from([("Which color?".to_owned(), text.to_owned())]),
        };
        assert!(matches!(
            decisions.decide(
                &id,
                Decision::Answer {
                    answers: BTreeMap::new()
                }
            ),
            Err(DecideError::Invalid(_))
        ));
        assert!(matches!(
            decisions.decide(
                &id,
                Decision::Answer {
                    answers: BTreeMap::from([("Other?".to_owned(), "x".to_owned())])
                }
            ),
            Err(DecideError::Invalid(_))
        ));
        assert!(matches!(
            decisions.decide(&id, Decision::Allow { suggestion: None }),
            Err(DecideError::Invalid(_))
        ));
        decisions.decide(&id, answer("Blue")).unwrap();
        let body = task.await.unwrap().unwrap();
        let output = &body["hookSpecificOutput"];
        assert_eq!(output["hookEventName"], "PreToolUse");
        assert_eq!(output["permissionDecision"], "allow");
        assert_eq!(output["updatedInput"]["answers"]["Which color?"], "Blue");
        assert_eq!(
            output["updatedInput"]["questions"][0]["options"][1]["label"],
            "Blue"
        );
    }

    #[tokio::test]
    async fn a_question_can_be_denied_with_a_reason_and_its_own_pre_tool_use_does_not_clear_it() {
        let decisions = Decisions::new(settings(5_000, 5_000));
        let task = spawn_hold(&decisions, question("s1")).await;
        decisions.observe(&Observed {
            event: "PreToolUse",
            session_id: Some("s1"),
            tool_name: Some("AskUserQuestion"),
            tool_input: Some(&json!({})),
        });
        assert_eq!(decisions.cards().len(), 1);
        let id = decisions.cards()[0].id.clone();
        decisions
            .decide(
                &id,
                Decision::Deny {
                    message: Some("decide yourself".into()),
                },
            )
            .unwrap();
        let body = task.await.unwrap().unwrap();
        assert_eq!(body["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            body["hookSpecificOutput"]["permissionDecisionReason"],
            "decide yourself"
        );
    }

    #[test]
    fn suggestions_are_worded_for_a_person() {
        assert_eq!(
            suggestion_label(
                &json!({ "type": "addDirectories", "directories": ["/a/b"], "destination": "session" })
            ),
            "Allow access to /a/b for this session"
        );
        assert_eq!(
            suggestion_label(
                &json!({ "type": "setMode", "mode": "acceptEdits", "destination": "session" })
            ),
            "Switch to acceptEdits mode for this session"
        );
    }

    #[test]
    fn a_tool_call_is_summarized_in_one_line() {
        assert_eq!(summarize("Bash", &json!({ "command": "a\nb" })), "a");
        assert_eq!(
            summarize("Write", &json!({ "file_path": "/x/y.rs", "content": "z" })),
            "/x/y.rs"
        );
        assert_eq!(
            summarize("Mystery", &json!({ "n": 1, "t": "text" })),
            "text"
        );
        assert_eq!(summarize("Mystery", &json!({ "n": 1 })), "{\"n\":1}");
        assert!(summarize("Bash", &json!({ "command": "x".repeat(500) })).ends_with('…'));
    }
}
