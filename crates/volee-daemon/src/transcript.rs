//! Claude Code session transcripts, read for the finished-work card screen.
//!
//! This is the ONE place that knows the transcript format
//! (`~/.claude/projects/<encoded cwd>/<session>.jsonl`, one JSON value per
//! line). The format is Claude Code's, not ours, and it moves: every reader
//! here is tolerant. A line that does not parse, a line of a type nobody
//! knows, a field that is missing: all are skipped and counted, never an
//! error. When the file cannot be read at all the caller shows the hooks-only
//! facts and says "transcript unavailable".
//!
//! What is extracted, for the work since the branch left its base (or since
//! the last prompt when that moment is unknown):
//!
//! - the **prompts** the person typed;
//! - the **plan** followed: `TodoWrite`, `TaskCreate`/`TaskUpdate`,
//!   `ExitPlanMode`, and as a last resort the numbered steps of a prompt;
//! - the **files touched** with the tool that touched them and what the
//!   assistant said just before;
//! - the **commands run** with their outcome (tests included);
//! - the assistant's **final summary**.

// Style lints that cost more than they give in this parsing code.
#![allow(
    clippy::assert_is_empty,
    clippy::format_push_string,
    clippy::items_after_statements,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::case_sensitive_file_extension_comparisons
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// A line longer than this is skipped (a pasted blob, not conversation).
const MAX_LINE: usize = 4 * 1024 * 1024;
/// A transcript larger than this is not read.
const MAX_FILE: u64 = 512 * 1024 * 1024;
/// How much of a command's output is kept.
const TAIL: usize = 400;

/// A prompt the person typed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Prompt {
    pub(crate) text: String,
    /// Milliseconds since the Unix epoch, when the line says.
    pub(crate) at: Option<i64>,
}

/// How far a plan step is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum StepStatus {
    Pending,
    InProgress,
    Done,
    /// The source does not say (a plan written as text).
    Unknown,
}

/// One step of the plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Step {
    pub(crate) text: String,
    pub(crate) status: StepStatus,
}

/// Where the plan comes from, the weakest last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PlanSource {
    /// A todo or task list the agent kept up to date.
    Tasks,
    /// A plan the agent proposed in plan mode.
    PlanMode,
    /// Numbered steps found in a prompt: what was asked, not what was done.
    Prompt,
}

/// The plan followed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Plan {
    pub(crate) source: PlanSource,
    pub(crate) steps: Vec<Step>,
    /// The plan's own text, for plan mode.
    pub(crate) text: Option<String>,
}

/// A file an editing tool touched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Touch {
    /// The path as the tool call gave it (usually absolute).
    pub(crate) path: String,
    pub(crate) tool: String,
    /// What the assistant said just before the call, shortened.
    pub(crate) context: Option<String>,
}

/// How a command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Outcome {
    Ok,
    Failed,
    /// No result line was found.
    Unknown,
}

/// A shell command the agent ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Command {
    pub(crate) command: String,
    pub(crate) description: Option<String>,
    /// Whether it looks like a test run.
    pub(crate) is_test: bool,
    pub(crate) outcome: Outcome,
    /// The end of its output, shortened.
    pub(crate) tail: Option<String>,
}

/// What the transcript says of the work.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Transcript {
    pub(crate) prompts: Vec<Prompt>,
    pub(crate) plan: Option<Plan>,
    pub(crate) touches: Vec<Touch>,
    pub(crate) commands: Vec<Command>,
    /// The assistant's last words after its last tool call.
    pub(crate) summary: Option<String>,
    /// Lines read.
    pub(crate) lines: usize,
    /// Lines skipped: not JSON, too long, or of a shape nobody knows.
    pub(crate) skipped: usize,
}

/// Why a transcript could not be read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unavailable {
    NotFound,
    TooLarge,
    Io(String),
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("no transcript file was found for this session"),
            Self::TooLarge => f.write_str("the transcript is too large to read"),
            Self::Io(why) => write!(f, "the transcript could not be read: {why}"),
        }
    }
}

/// How Claude Code names a project directory: every character that is not
/// a letter or a digit becomes `-`.
pub(crate) fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Where the transcript of `session` is: under the project directory of
/// `cwd` first, then in any project directory (a session can be resumed
/// from another directory).
pub(crate) fn locate(projects: &Path, cwd: &Path, session: &str) -> Option<PathBuf> {
    // A session id is a uuid; refuse anything that could leave the directory.
    if session.is_empty()
        || !session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    let file = format!("{session}.jsonl");
    let direct = projects.join(encode_cwd(cwd)).join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(projects)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(&file))
        .find(|path| path.is_file())
}

/// Reads the transcript at `path`. `since` is the moment the branch left its
/// base, in milliseconds: the window starts at the last prompt typed before
/// it. Without it the window starts at the last prompt.
pub(crate) fn read(path: &Path, since: Option<i64>) -> Result<Transcript, Unavailable> {
    let file = std::fs::File::open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => Unavailable::NotFound,
        _ => Unavailable::Io(error.to_string()),
    })?;
    let size = file
        .metadata()
        .map_err(|e| Unavailable::Io(e.to_string()))?
        .len();
    if size > MAX_FILE {
        return Err(Unavailable::TooLarge);
    }
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    let mut events = Vec::new();
    let mut lines = 0;
    let mut skipped = 0;
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => return Err(Unavailable::Io(error.to_string())),
        }
        lines += 1;
        if buffer.len() > MAX_LINE {
            skipped += 1;
            continue;
        }
        match serde_json::from_slice::<Value>(&buffer) {
            Ok(value) => {
                if !collect(&value, &mut events) {
                    skipped += 1;
                }
            }
            Err(_) => skipped += 1,
        }
    }
    let mut transcript = reduce(&events, since);
    transcript.lines = lines;
    transcript.skipped = skipped;
    Ok(transcript)
}

/// Reads a transcript held in memory (tests and fixtures).
#[cfg(test)]
pub(crate) fn read_str(text: &str, since: Option<i64>) -> Transcript {
    let mut events = Vec::new();
    let mut skipped = 0;
    let mut lines = 0;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        lines += 1;
        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                if !collect(&value, &mut events) {
                    skipped += 1;
                }
            }
            Err(_) => skipped += 1,
        }
    }
    let mut transcript = reduce(&events, since);
    transcript.lines = lines;
    transcript.skipped = skipped;
    transcript
}

/// One line, boiled down to what the screen needs. Large tool inputs (a
/// written file) are dropped here so a long session stays small in memory.
#[derive(Debug)]
enum Event {
    Prompt {
        text: String,
        at: Option<i64>,
    },
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        path: Option<String>,
        command: Option<String>,
        description: Option<String>,
        /// Kept only for the tools that carry the plan.
        input: Option<Value>,
    },
    ToolResult {
        id: String,
        is_error: bool,
        text: String,
    },
}

/// Turns one line into events. Returns `false` for a line that is neither a
/// conversation line nor a known bookkeeping line (it is counted as
/// skipped); the many bookkeeping types Claude Code writes are not an error.
fn collect(value: &Value, events: &mut Vec<Event>) -> bool {
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return false;
    };
    match kind {
        "user" | "assistant" => {}
        // Lines that exist and say nothing we need.
        "system"
        | "attachment"
        | "summary"
        | "ai-title"
        | "custom-title"
        | "agent-name"
        | "tools-used"
        | "mode"
        | "permission-mode"
        | "last-prompt"
        | "file-history-snapshot"
        | "file-history-delta"
        | "session-owner"
        | "bridge-session"
        | "worktree-state"
        | "cost-state"
        | "derived-from"
        | "data-flow-records"
        | "atis-latch"
        | "api-request"
        | "api-request-blob"
        | "api-request-shape"
        | "queue-operation" => {
            return true;
        }
        _ => return false,
    }
    if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    let Some(message) = value.get("message") else {
        return false;
    };
    let at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_time);
    let content = message.get("content");
    if kind == "user" {
        collect_user(value, content, at, events)
    } else {
        collect_assistant(content, events)
    }
}

fn collect_user(
    line: &Value,
    content: Option<&Value>,
    at: Option<i64>,
    events: &mut Vec<Event>,
) -> bool {
    match content {
        Some(Value::String(text)) => {
            if is_human(line, text) {
                events.push(Event::Prompt {
                    text: text.clone(),
                    at,
                });
            }
            true
        }
        Some(Value::Array(blocks)) => {
            let mut typed = String::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_result") => {
                        let id = str_of(block, "tool_use_id").unwrap_or_default();
                        events.push(Event::ToolResult {
                            id,
                            is_error: block.get("is_error").and_then(Value::as_bool) == Some(true),
                            text: result_text(block.get("content")),
                        });
                    }
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            if !typed.is_empty() {
                                typed.push('\n');
                            }
                            typed.push_str(text);
                        }
                    }
                    _ => {}
                }
            }
            if !typed.is_empty() && is_human(line, &typed) {
                events.push(Event::Prompt { text: typed, at });
            }
            true
        }
        _ => false,
    }
}

fn collect_assistant(content: Option<&Value>, events: &mut Vec<Event>) -> bool {
    let Some(Value::Array(blocks)) = content else {
        // A bare string is an assistant line too.
        if let Some(Value::String(text)) = content {
            events.push(Event::Text { text: text.clone() });
            return true;
        }
        return false;
    };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    events.push(Event::Text {
                        text: text.to_owned(),
                    });
                }
            }
            Some("tool_use") => {
                let name = str_of(block, "name").unwrap_or_default();
                let input = block.get("input");
                let field = |key: &str| input.and_then(|i| str_of(i, key));
                let carries_plan = matches!(
                    name.as_str(),
                    "TodoWrite" | "TaskCreate" | "TaskUpdate" | "ExitPlanMode"
                );
                events.push(Event::ToolUse {
                    id: str_of(block, "id").unwrap_or_default(),
                    path: field("file_path").or_else(|| field("notebook_path")),
                    command: field("command"),
                    description: field("description"),
                    input: input.filter(|_| carries_plan).cloned(),
                    name,
                });
            }
            _ => {}
        }
    }
    true
}

/// Whether a user line is something the person typed: not a hook's or the
/// harness's own message.
fn is_human(line: &Value, text: &str) -> bool {
    if line.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    if let Some(kind) = line.pointer("/origin/kind").and_then(Value::as_str)
        && kind != "human"
    {
        return false;
    }
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    const HARNESS: [&str; 6] = [
        "<command-name>",
        "<command-message>",
        "<local-command",
        "<system-reminder>",
        "<task-notification",
        "[Request interrupted",
    ];
    !HARNESS.iter().any(|tag| trimmed.starts_with(tag))
}

fn str_of(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A tool result's text: a string, or an array of text blocks.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// `2026-11-22T01:08:29.395Z` as milliseconds since the Unix epoch.
pub(crate) fn parse_time(text: &str) -> Option<i64> {
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let time = time.trim_end_matches('Z');
    let time = time.split(['+', ' ']).next()?;
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = clock.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let minute: i64 = t.next()?.parse().ok()?;
    let second: i64 = t.next()?.parse().ok()?;
    let millis: i64 = format!("{fraction:0<3}")[..3].parse().ok()?;
    // Days since the epoch (Howard Hinnant's civil-from-days, inverted).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis)
}

/// Applies the window and extracts the facts.
fn reduce(events: &[Event], since: Option<i64>) -> Transcript {
    let start = window_start(events, since);
    let window = &events[start..];

    let mut transcript = Transcript::default();
    let mut results: HashMap<&str, (bool, &str)> = HashMap::new();
    for event in window {
        if let Event::ToolResult { id, is_error, text } = event {
            results.insert(id, (*is_error, text));
        }
    }

    let mut last_text: Option<&str> = None;
    let mut todos: Option<Vec<Step>> = None;
    let mut tasks: Vec<(String, Step)> = Vec::new();
    let mut plan_text: Option<String> = None;
    for event in window {
        match event {
            Event::Prompt { text, at } => transcript.prompts.push(Prompt {
                text: clip(text, 600),
                at: *at,
            }),
            Event::Text { text } => last_text = Some(text),
            Event::ToolResult { .. } => {}
            Event::ToolUse {
                id,
                name,
                path,
                command,
                description,
                input,
            } => {
                match name.as_str() {
                    "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
                        if let Some(path) = path {
                            transcript.touches.push(Touch {
                                path: path.clone(),
                                tool: name.clone(),
                                context: last_text.map(|text| clip(text.trim(), 240)),
                            });
                        }
                    }
                    "Bash" => {
                        if let Some(command) = command {
                            transcript.commands.push(command_of(
                                command,
                                description.as_deref(),
                                results.get(id.as_str()).copied(),
                            ));
                        }
                    }
                    "TodoWrite" => {
                        if let Some(list) = input.as_ref().and_then(todo_steps) {
                            todos = Some(list);
                        }
                    }
                    "TaskCreate" => {
                        if let Some(input) = input {
                            let subject = str_of(input, "subject")
                                .or_else(|| str_of(input, "content"))
                                .or_else(|| str_of(input, "description"));
                            if let Some(text) = subject {
                                let made = results.get(id.as_str()).map(|(_, t)| *t);
                                let task_id = made
                                    .and_then(task_number)
                                    .unwrap_or_else(|| (tasks.len() + 1).to_string());
                                tasks.push((
                                    task_id,
                                    Step {
                                        text,
                                        status: StepStatus::Pending,
                                    },
                                ));
                            }
                        }
                    }
                    "TaskUpdate" => {
                        if let Some(input) = input {
                            update_task(&mut tasks, input);
                        }
                    }
                    "ExitPlanMode" => {
                        if let Some(text) = input.as_ref().and_then(|i| str_of(i, "plan")) {
                            plan_text = Some(text);
                        }
                    }
                    _ => {}
                }
                // What the assistant said before a call explains that call
                // only; the next text starts a fresh explanation.
                last_text = None;
            }
        }
    }

    transcript.summary = final_text(window);
    transcript.plan = if let Some(steps) = todos.filter(|steps| !steps.is_empty()) {
        Some(Plan {
            source: PlanSource::Tasks,
            steps,
            text: plan_text,
        })
    } else if !tasks.is_empty() {
        Some(Plan {
            source: PlanSource::Tasks,
            steps: tasks.into_iter().map(|(_, step)| step).collect(),
            text: plan_text,
        })
    } else if let Some(text) = plan_text {
        let steps = numbered_steps(&text)
            .or_else(|| bullet_steps(&text))
            .unwrap_or_default();
        Some(Plan {
            source: PlanSource::PlanMode,
            steps: steps
                .into_iter()
                .map(|text| Step {
                    text,
                    status: StepStatus::Unknown,
                })
                .collect(),
            text: Some(text),
        })
    } else {
        transcript
            .prompts
            .iter()
            .rev()
            .find_map(|prompt| numbered_steps(&prompt.text))
            .map(|steps| Plan {
                source: PlanSource::Prompt,
                steps: steps
                    .into_iter()
                    .map(|text| Step {
                        text,
                        status: StepStatus::Unknown,
                    })
                    .collect(),
                text: None,
            })
    };
    transcript
}

/// Index of the first event of the window.
fn window_start(events: &[Event], since: Option<i64>) -> usize {
    let prompts = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            Event::Prompt { at, .. } => Some((index, *at)),
            _ => None,
        });
    match since {
        Some(since) => prompts
            .filter(|(_, at)| at.is_some_and(|at| at <= since))
            .map(|(index, _)| index)
            .next_back()
            .unwrap_or(0),
        None => prompts.map(|(index, _)| index).next_back().unwrap_or(0),
    }
}

/// The assistant's words after its last tool call or result.
fn final_text(window: &[Event]) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for event in window.iter().rev() {
        match event {
            Event::Text { text } => parts.push(text),
            Event::Prompt { .. } if parts.is_empty() => {}
            _ => break,
        }
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.join("\n\n").trim().to_owned()).filter(|text| !text.is_empty())
}

fn command_of(command: &str, description: Option<&str>, result: Option<(bool, &str)>) -> Command {
    let (outcome, tail) = match result {
        None => (Outcome::Unknown, None),
        Some((is_error, text)) => {
            let failed = is_error || exit_code(text).is_some_and(|code| code != 0);
            (
                if failed { Outcome::Failed } else { Outcome::Ok },
                Some(tail_of(text)).filter(|tail| !tail.is_empty()),
            )
        }
    };
    Command {
        command: clip(command, 400),
        description: description.map(|d| clip(d, 200)),
        is_test: looks_like_test(command),
        outcome,
        tail,
    }
}

/// `Exit code N` as Claude Code writes it at the top of a failed result.
fn exit_code(text: &str) -> Option<i32> {
    let rest = text.trim_start().strip_prefix("Exit code ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn tail_of(text: &str) -> String {
    let trimmed = text.trim_end();
    let count = trimmed.chars().count();
    if count <= TAIL {
        return trimmed.to_owned();
    }
    let skip = count - TAIL;
    format!("...{}", trimmed.chars().skip(skip).collect::<String>())
}

/// Whether a shell command looks like a test run.
fn looks_like_test(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    const WORDS: [&str; 12] = [
        "test", "pytest", "nextest", "vitest", "jest", "mocha", "rspec", "phpunit", "go vet",
        "tsc", "clippy", "eslint",
    ];
    WORDS.iter().any(|word| lower.contains(word))
}

fn todo_steps(input: &Value) -> Option<Vec<Step>> {
    let todos = input.get("todos")?.as_array()?;
    Some(
        todos
            .iter()
            .filter_map(|todo| {
                let text = str_of(todo, "content").or_else(|| str_of(todo, "subject"))?;
                Some(Step {
                    text,
                    status: status_of(todo.get("status").and_then(Value::as_str)),
                })
            })
            .collect(),
    )
}

fn status_of(status: Option<&str>) -> StepStatus {
    match status {
        Some("completed" | "done") => StepStatus::Done,
        Some("in_progress") => StepStatus::InProgress,
        Some("pending") => StepStatus::Pending,
        _ => StepStatus::Unknown,
    }
}

/// `Task #3 created...` gives `3`.
fn task_number(text: &str) -> Option<String> {
    let rest = text.split('#').nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some(digits).filter(|digits| !digits.is_empty())
}

fn update_task(tasks: &mut Vec<(String, Step)>, input: &Value) {
    let id = match input.get("taskId") {
        Some(Value::String(id)) => id.clone(),
        Some(Value::Number(id)) => id.to_string(),
        _ => return,
    };
    let status = input.get("status").and_then(Value::as_str);
    if status == Some("deleted") {
        tasks.retain(|(task, _)| *task != id);
        return;
    }
    if let Some((_, step)) = tasks.iter_mut().find(|(task, _)| *task == id) {
        if status.is_some() {
            step.status = status_of(status);
        }
        if let Some(subject) = str_of(input, "subject") {
            step.text = subject;
        }
    }
}

/// Steps written `1) first 2) second` or one per line as `1. first`; at
/// least two, numbered from 1 without gaps.
pub(crate) fn numbered_steps(text: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = text.chars().collect();
    let mut marks: Vec<(usize, usize, u32)> = Vec::new(); // (marker start, text start, number)
    let mut expect = 1;
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(';
        if boundary && chars[i].is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let number: Option<u32> = chars[i..j].iter().collect::<String>().parse().ok();
            let closes = j < chars.len() && matches!(chars[j], '.' | ')');
            let spaced = j + 1 < chars.len() && chars[j + 1].is_whitespace();
            if closes && spaced && number == Some(expect) {
                marks.push((i, j + 2, expect));
                expect += 1;
                i = j + 2;
                continue;
            }
        }
        i += 1;
    }
    if marks.len() < 2 {
        return None;
    }
    let mut steps = Vec::new();
    for (index, (_, from, _)) in marks.iter().enumerate() {
        let to = marks
            .get(index + 1)
            .map_or(chars.len(), |(start, _, _)| *start);
        let raw: String = chars[*from..to].iter().collect();
        // The last step ends at the first blank line or sentence break.
        let step = if index + 1 == marks.len() {
            raw.split("\n\n").next().unwrap_or(&raw).to_owned()
        } else {
            raw
        };
        let step = step.trim().trim_end_matches([',', ';']).trim();
        if !step.is_empty() {
            steps.push(clip(step, 200));
        }
    }
    Some(steps).filter(|steps| steps.len() >= 2)
}

/// Steps written as a bulleted or checkbox list.
fn bullet_steps(text: &str) -> Option<Vec<String>> {
    let steps: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let rest = line
                .strip_prefix("- [ ] ")
                .or_else(|| line.strip_prefix("- [x] "))
                .or_else(|| line.strip_prefix("- "))
                .or_else(|| line.strip_prefix("* "))?;
            Some(clip(rest.trim(), 200))
        })
        .collect();
    Some(steps).filter(|steps| steps.len() >= 2)
}

/// `text` cut to `max` characters, with an ellipsis when it was cut.
pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{}...", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = include_str!("../tests/fixtures/haiku-two-tasks.jsonl");

    #[test]
    fn project_directory_names_follow_claude_code() {
        assert_eq!(
            encode_cwd(Path::new("/Users/a/Dev/my.repo/.claude/worktrees/x")),
            "-Users-a-Dev-my-repo--claude-worktrees-x"
        );
    }

    #[test]
    fn time_parsing_matches_known_instants() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2000-03-01T00:00:00.5Z"), Some(951_868_800_500));
        assert_eq!(
            parse_time("2026-11-22T01:08:29.395Z"),
            Some(1_795_309_709_395)
        );
        assert_eq!(parse_time("not a time"), None);
    }

    #[test]
    fn real_transcript_gives_the_whole_story() {
        let t = read_str(REAL, None);
        // No `since`: the window starts at the last prompt (the div task).
        assert_eq!(t.prompts.len(), 1);
        assert!(t.prompts[0].text.contains("div"));
        let t = read_str(REAL, Some(0));
        // `since` before everything: the window starts at the first prompt.
        assert_eq!(t.prompts.len(), 2);
        assert!(t.prompts[0].text.contains("mul"));
        assert!(t.skipped < t.lines);
    }

    #[test]
    fn real_transcript_files_commands_and_summary() {
        let t = read_str(REAL, Some(0));
        let paths: Vec<&str> = t.touches.iter().map(|touch| touch.path.as_str()).collect();
        assert!(paths.iter().any(|p| p.ends_with("src/math.ts")));
        assert!(paths.iter().any(|p| p.ends_with("tests/math.test.ts")));
        assert!(paths.iter().any(|p| p.ends_with("README.md")));
        assert!(t.touches.iter().all(|touch| touch.tool == "Edit"));
        assert!(
            t.touches[0]
                .context
                .as_deref()
                .is_some_and(|c| !c.is_empty())
        );
        let tests: Vec<&Command> = t.commands.iter().filter(|c| c.is_test).collect();
        assert!(!tests.is_empty(), "a bun test run is a test");
        assert!(t.commands.iter().any(|c| c.command.starts_with("git add")));
        assert!(t.commands.iter().all(|c| c.outcome != Outcome::Unknown));
        let summary = t.summary.expect("a final summary");
        assert!(summary.len() > 20);
    }

    #[test]
    fn real_transcript_plan_falls_back_to_the_prompts_numbered_steps() {
        let t = read_str(REAL, Some(0));
        let plan = t.plan.expect("a plan");
        assert_eq!(plan.source, PlanSource::Prompt);
        assert_eq!(plan.steps.len(), 3);
        assert!(plan.steps[0].text.contains("mul"));
        assert!(plan.steps.iter().all(|s| s.status == StepStatus::Unknown));
    }

    fn line(value: &Value) -> String {
        value.to_string()
    }

    fn user(text: &str, at: &str) -> String {
        line(&serde_json::json!({
            "type": "user", "timestamp": at, "origin": {"kind": "human"},
            "message": {"role": "user", "content": text}
        }))
    }

    fn assistant(blocks: &Value, at: &str) -> String {
        line(&serde_json::json!({
            "type": "assistant", "timestamp": at,
            "message": {"role": "assistant", "content": blocks}
        }))
    }

    fn result(id: &str, content: &Value, is_error: bool) -> String {
        line(&serde_json::json!({
            "type": "user", "timestamp": "2026-01-01T00:00:10Z",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error}
            ]}
        }))
    }

    #[test]
    fn todo_write_keeps_the_last_list_and_its_statuses() {
        let text = [
            user("do it", "2026-01-01T00:00:00Z"),
            assistant(
                &serde_json::json!([{"type": "tool_use", "id": "a", "name": "TodoWrite", "input": {"todos": [
                    {"content": "one", "status": "pending", "activeForm": "x"},
                    {"content": "two", "status": "pending", "activeForm": "y"}]}}]),
                "2026-01-01T00:00:01Z",
            ),
            assistant(
                &serde_json::json!([{"type": "tool_use", "id": "b", "name": "TodoWrite", "input": {"todos": [
                    {"content": "one", "status": "completed", "activeForm": "x"},
                    {"content": "two", "status": "in_progress", "activeForm": "y"}]}}]),
                "2026-01-01T00:00:02Z",
            ),
        ]
        .join("\n");
        let plan = read_str(&text, None).plan.unwrap();
        assert_eq!(plan.source, PlanSource::Tasks);
        assert_eq!(plan.steps[0].status, StepStatus::Done);
        assert_eq!(plan.steps[1].status, StepStatus::InProgress);
    }

    #[test]
    fn task_create_and_update_follow_ids() {
        let text = [
            user("go", "2026-01-01T00:00:00Z"),
            assistant(
                &serde_json::json!([{"type": "tool_use", "id": "c1", "name": "TaskCreate", "input": {"subject": "first", "description": "d"}}]),
                "2026-01-01T00:00:01Z",
            ),
            result("c1", &serde_json::json!("Task #7 created successfully: first"), false),
            assistant(
                &serde_json::json!([{"type": "tool_use", "id": "c2", "name": "TaskCreate", "input": {"subject": "second", "description": "d"}}]),
                "2026-01-01T00:00:02Z",
            ),
            result("c2", &serde_json::json!("Task #8 created"), false),
            assistant(
                &serde_json::json!([{"type": "tool_use", "id": "u1", "name": "TaskUpdate", "input": {"taskId": "7", "status": "completed"}},
                                    {"type": "tool_use", "id": "u2", "name": "TaskUpdate", "input": {"taskId": 8, "status": "deleted"}}]),
                "2026-01-01T00:00:03Z",
            ),
        ]
        .join("\n");
        let plan = read_str(&text, None).plan.unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].text, "first");
        assert_eq!(plan.steps[0].status, StepStatus::Done);
    }

    #[test]
    fn exit_plan_mode_gives_the_plan_text_and_its_steps() {
        let text = [
            user("plan it", "2026-01-01T00:00:00Z"),
            assistant(
                &serde_json::json!([{"type": "tool_use", "id": "p", "name": "ExitPlanMode", "input": {"plan": "## Plan\n\n1. Add the parser\n2. Wire the route\n3. Test it\n"}}]),
                "2026-01-01T00:00:01Z",
            ),
        ]
        .join("\n");
        let plan = read_str(&text, None).plan.unwrap();
        assert_eq!(plan.source, PlanSource::PlanMode);
        assert_eq!(plan.steps.len(), 3);
        assert!(plan.text.unwrap().starts_with("## Plan"));
    }

    #[test]
    fn failed_commands_and_tests_are_recognised() {
        let text = [
            user("run", "2026-01-01T00:00:00Z"),
            assistant(
                &serde_json::json!([{"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": "cargo test", "description": "Run the tests"}},
                                    {"type": "tool_use", "id": "g", "name": "Bash", "input": {"command": "false"}}]),
                "2026-01-01T00:00:01Z",
            ),
            result("t", &serde_json::json!("Exit code 101\nfailures:\n  foo"), true),
            result("g", &serde_json::json!([{"type": "text", "text": "Exit code 1"}]), false),
        ]
        .join("\n");
        let t = read_str(&text, None);
        assert!(t.commands[0].is_test);
        assert_eq!(t.commands[0].outcome, Outcome::Failed);
        assert!(t.commands[0].tail.as_deref().unwrap().contains("failures"));
        assert_eq!(t.commands[1].outcome, Outcome::Failed);
        assert!(!t.commands[1].is_test);
    }

    #[test]
    fn odd_lines_are_skipped_never_fatal() {
        let text = [
            "not json at all".to_owned(),
            "{\"type\":\"brand-new-line-type\",\"x\":1}".to_owned(),
            "{\"no_type\":true}".to_owned(),
            "[1,2,3]".to_owned(),
            "{\"type\":\"user\"}".to_owned(),
            "{\"type\":\"user\",\"message\":{\"content\":42}}".to_owned(),
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"future_block\"},7,null]}}".to_owned(),
            "{\"type\":\"assistant\",\"timestamp\":5,\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Edit\",\"input\":\"oops\"}]}}".to_owned(),
            user("hello", "garbage"),
            assistant(&serde_json::json!([{"type": "text", "text": "done."}]), "2026-01-01T00:00:00Z"),
        ]
        .join("\n");
        let t = read_str(&text, None);
        assert_eq!(t.prompts.len(), 1);
        assert_eq!(t.summary.as_deref(), Some("done."));
        assert!(t.skipped >= 5);
        assert!(t.touches.is_empty());
    }

    #[test]
    fn harness_messages_are_not_prompts() {
        let meta = line(&serde_json::json!({
            "type": "user", "isMeta": true, "message": {"content": "caveat"}}));
        let notif = line(&serde_json::json!({
            "type": "user", "origin": {"kind": "task-notification"}, "message": {"content": "done"}}));
        let reminder = user(
            "<system-reminder>x</system-reminder>",
            "2026-01-01T00:00:00Z",
        );
        let real = user("real one", "2026-01-01T00:00:01Z");
        let t = read_str(&[meta, notif, reminder, real].join("\n"), None);
        assert_eq!(t.prompts.len(), 1);
        assert_eq!(t.prompts[0].text, "real one");
    }

    #[test]
    fn sidechain_lines_are_ignored() {
        let side = line(&serde_json::json!({
            "type": "assistant", "isSidechain": true,
            "message": {"content": [{"type": "tool_use", "id": "x", "name": "Write", "input": {"file_path": "/a/b.rs", "content": "..."}}]}}));
        let t = read_str(&[user("hi", "2026-01-01T00:00:00Z"), side].join("\n"), None);
        assert!(t.touches.is_empty());
    }

    #[test]
    fn numbered_steps_need_a_run_from_one() {
        assert_eq!(
            numbered_steps("1) add a  2) add b 3) add c. Then stop.")
                .unwrap()
                .len(),
            3
        );
        assert_eq!(numbered_steps("1. a\n2. b").unwrap(), ["a", "b"]);
        assert!(numbered_steps("version 2.0 is out, see 3) here").is_none());
        assert!(numbered_steps("1. alone").is_none());
    }

    #[test]
    fn a_missing_file_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read(&dir.path().join("none.jsonl"), None).unwrap_err(),
            Unavailable::NotFound
        );
        assert!(locate(dir.path(), Path::new("/x"), "../etc").is_none());
    }

    #[test]
    fn locate_finds_a_session_by_cwd_then_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("-work-repo");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("abc-1.jsonl"), "").unwrap();
        let direct = locate(dir.path(), Path::new("/work/repo"), "abc-1").unwrap();
        assert!(direct.ends_with("-work-repo/abc-1.jsonl"));
        let elsewhere = locate(dir.path(), Path::new("/other/place"), "abc-1").unwrap();
        assert_eq!(elsewhere, direct);
        assert!(locate(dir.path(), Path::new("/work/repo"), "zzz").is_none());
    }

    #[test]
    fn a_file_reads_like_a_string() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, REAL).unwrap();
        let from_file = read(&path, Some(0)).unwrap();
        assert_eq!(from_file.prompts, read_str(REAL, Some(0)).prompts);
        assert_eq!(from_file.lines, REAL.lines().count());
    }
}
