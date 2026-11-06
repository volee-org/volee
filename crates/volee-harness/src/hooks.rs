//! Claude Code hooks: what volee's hooks receive, what it means, and the
//! per-agent configuration that installs them.
//!
//! Every agent volee launches gets a settings file (passed with
//! `--settings`, which adds to the user's own settings) whose hooks run
//! `volee hook <event> --agent <id>` and which allows volee's MCP tools, see
//! [`agent_settings`]. The command
//! receives one JSON object on standard input: a [`HookPayload`].
//! [`interpret`] turns it into a [`HookSignal`] that the daemon folds into
//! the agent's state.
//!
//! Messages reach agents through the same hooks
//! ([ADR 0006](https://volee-org.github.io/volee/adr/0006-messaging.html)): a
//! `Stop` hook prints [`stop_block_output`] to make a working agent continue
//! with its messages, and an `asyncRewake` `Stop` hook wakes an idle agent by
//! exiting with [`REWAKE_EXIT_CODE`] and the messages on standard error. Both
//! carry the text of [`format_messages_for_agent`].
//!
//! Payload fields were measured on Claude Code 2.1.319. Unknown fields are
//! ignored, every event-specific field is optional, and a field with an
//! unexpected type is treated as missing, so a hook event is never lost to a
//! format change.

mod delivery;
mod settings;
mod summary;

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer};
use serde_json::Value;
use volee_core::state::{AgentState, InputRequest};

pub use delivery::{
    MAX_DELIVERY_CHARS, RenderedMessages, format_messages_for_agent, stop_block_output,
};
pub use settings::{
    HOOK_TIMEOUT_SECS, MAX_REWAKE_SECS, REWAKE_GRACE_SECS, VOLEE_TOOLS_RULE, agent_settings,
    mcp_config,
};
pub use summary::{tool_summary, written_files};

use crate::lenient;

/// The exit status with which an `asyncRewake` hook wakes an idle agent; the
/// hook's standard error becomes the agent's next input.
pub const REWAKE_EXIT_CODE: i32 = 2;

/// A Claude Code hook event, by the name Claude Code gives it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HookEventName {
    /// A session started, resumed, forked, or restarted after `/clear` or a
    /// compaction.
    SessionStart,
    /// A prompt was submitted.
    UserPromptSubmit,
    /// A tool is about to run.
    PreToolUse,
    /// A tool ran successfully.
    PostToolUse,
    /// Claude Code notified the user, for example of a permission prompt.
    Notification,
    /// The agent finished its turn.
    Stop,
    /// The session ended.
    SessionEnd,
    /// A subagent started.
    SubagentStart,
    /// A subagent finished.
    SubagentStop,
    /// Any other event.
    Other(String),
}

impl HookEventName {
    /// The events volee installs a hook for, in a stable order.
    pub const OBSERVED: [Self; 9] = [
        Self::SessionStart,
        Self::UserPromptSubmit,
        Self::PreToolUse,
        Self::PostToolUse,
        Self::Notification,
        Self::Stop,
        Self::SessionEnd,
        Self::SubagentStart,
        Self::SubagentStop,
    ];

    /// The name Claude Code uses, such as `PreToolUse`.
    pub fn as_str(&self) -> &str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::Notification => "Notification",
            Self::Stop => "Stop",
            Self::SessionEnd => "SessionEnd",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::Other(name) => name,
        }
    }

    /// The name used on volee's command line, such as `pre-tool-use` in
    /// `volee hook pre-tool-use`.
    pub fn kebab(&self) -> String {
        let mut kebab = String::with_capacity(self.as_str().len() + 4);
        for (index, c) in self.as_str().chars().enumerate() {
            if c.is_ascii_uppercase() {
                if index > 0 {
                    kebab.push('-');
                }
                kebab.push(c.to_ascii_lowercase());
            } else {
                kebab.push(c);
            }
        }
        kebab
    }

    /// The event named `name` on volee's command line: the inverse of
    /// [`kebab`](Self::kebab).
    pub fn from_kebab(name: &str) -> Self {
        let camel: String = name
            .split('-')
            .map(|word| {
                let mut chars = word.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_ascii_uppercase().to_string() + chars.as_str()
                })
            })
            .collect();
        Self::from(camel)
    }
}

impl From<String> for HookEventName {
    fn from(name: String) -> Self {
        match name.as_str() {
            "SessionStart" => Self::SessionStart,
            "UserPromptSubmit" => Self::UserPromptSubmit,
            "PreToolUse" => Self::PreToolUse,
            "PostToolUse" => Self::PostToolUse,
            "Notification" => Self::Notification,
            "Stop" => Self::Stop,
            "SessionEnd" => Self::SessionEnd,
            "SubagentStart" => Self::SubagentStart,
            "SubagentStop" => Self::SubagentStop,
            _ => Self::Other(name),
        }
    }
}

impl From<&str> for HookEventName {
    fn from(name: &str) -> Self {
        Self::from(name.to_owned())
    }
}

impl<'de> Deserialize<'de> for HookEventName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

impl fmt::Display for HookEventName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a session started, from a `SessionStart` payload.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SessionSource {
    /// A new session.
    Startup,
    /// A stopped session continued with the same id.
    Resume,
    /// A new session started by `/clear`.
    Clear,
    /// The session continues after its context was compacted.
    Compact,
    /// A copy of a session, with a new id and the same conversation.
    Fork,
    /// Any other source.
    Other(String),
}

impl From<&str> for SessionSource {
    fn from(source: &str) -> Self {
        match source {
            "startup" => Self::Startup,
            "resume" => Self::Resume,
            "clear" => Self::Clear,
            "compact" => Self::Compact,
            "fork" => Self::Fork,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// The JSON object a Claude Code hook receives on standard input.
///
/// `session_id` and `hook_event_name` are always sent; every other field
/// depends on the event and is optional.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct HookPayload {
    /// The session's UUID.
    pub session_id: String,
    /// The event.
    pub hook_event_name: HookEventName,
    /// The session's working directory when the hook ran.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub cwd: Option<PathBuf>,
    /// The conversation file. volee may show it but never parses it
    /// ([ADR 0004](https://volee-org.github.io/volee/adr/0004-sources-and-reconciliation.html)).
    #[serde(default, deserialize_with = "lenient::optional")]
    pub transcript_path: Option<PathBuf>,
    /// The permission mode, on the events that carry it.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub permission_mode: Option<String>,
    /// `SessionStart`: `startup`, `resume`, `clear`, `compact` or `fork`.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub source: Option<String>,
    /// `SessionStart`: the model, when Claude Code sends it.
    #[serde(default, deserialize_with = "lenient::model")]
    pub model: Option<String>,
    /// `SessionStart` and `UserPromptSubmit`: the session's custom title.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub session_title: Option<String>,
    /// `UserPromptSubmit`: the submitted text.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub prompt: Option<String>,
    /// `PreToolUse` and `PostToolUse`: the tool, such as `Bash`.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub tool_name: Option<String>,
    /// `PreToolUse` and `PostToolUse`: the tool's arguments.
    #[serde(default)]
    pub tool_input: Option<Value>,
    /// `PreToolUse` and `PostToolUse`: pairs the two events of one call.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub tool_use_id: Option<String>,
    /// `Notification`: `permission_prompt`, `idle_prompt`,
    /// `elicitation_dialog` and others.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub notification_type: Option<String>,
    /// `Notification`: the text shown to the user.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub message: Option<String>,
    /// `Notification`: the title shown to the user.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub title: Option<String>,
    /// `Stop` and `SubagentStop`: the text of the final response.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub last_assistant_message: Option<String>,
    /// `Stop` and `SubagentStop`: whether the turn already continues because
    /// of a `Stop` hook. Claude Code ends the turn anyway after eight
    /// consecutive continuations.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub stop_hook_active: Option<bool>,
    /// `SessionEnd`: `clear`, `resume`, `logout`, `prompt_input_exit` or
    /// `other`.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub reason: Option<String>,
    /// `SubagentStart` and `SubagentStop`: the subagent's id.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub agent_id: Option<String>,
    /// `SubagentStart` and `SubagentStop`: the subagent's type, such as
    /// `Explore`.
    #[serde(default, deserialize_with = "lenient::optional")]
    pub agent_type: Option<String>,
}

impl HookPayload {
    /// Parses the JSON a hook received on standard input.
    pub fn from_json(json: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(json)
    }
}

/// What a hook event means for the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookSignal {
    /// A session started; the daemon links it to the agent named on the hook's
    /// command line.
    SessionStarted {
        /// The session's UUID.
        session_id: String,
        /// Why it started.
        source: Option<SessionSource>,
        /// The model, when known.
        model: Option<String>,
        /// The session's custom title, when set.
        title: Option<String>,
        /// The conversation file, for display only.
        transcript_path: Option<PathBuf>,
    },
    /// A prompt was submitted: the agent works.
    PromptSubmitted,
    /// A tool is about to run.
    ToolStarted {
        /// The tool.
        tool: String,
        /// One line for people, such as `Bash: cargo test`.
        summary: String,
    },
    /// A tool ran.
    ToolFinished {
        /// The tool.
        tool: String,
        /// One line for people, such as `Edit src/lib.rs`.
        summary: String,
        /// The files the tool wrote, as the tool received them (absolute).
        files: Vec<PathBuf>,
    },
    /// The agent waits for a person.
    NeedsInput {
        /// What it waits for.
        request: InputRequest,
        /// The question or notification text.
        detail: Option<String>,
    },
    /// The agent finished its turn.
    Stopped {
        /// The text of its final response.
        last_message: Option<String>,
    },
    /// A subagent started inside the session.
    SubagentStarted {
        /// The subagent's id.
        id: String,
        /// Its type, such as `Explore`.
        kind: Option<String>,
    },
    /// A subagent finished.
    SubagentStopped {
        /// The subagent's id.
        id: String,
        /// The text of its final response.
        last_message: Option<String>,
    },
    /// The session ended.
    SessionEnded {
        /// Why: `clear`, `resume`, `logout`, `prompt_input_exit` or `other`.
        reason: Option<String>,
    },
    /// Nothing volee tracks.
    Ignored,
}

impl HookSignal {
    /// The agent state this signal implies, if any.
    ///
    /// A prompt or a tool means working; a permission prompt, a question or a
    /// dialog means needing input; the end of a turn means done (ready for
    /// the next prompt, like Claude Code's `done`); the end of a session
    /// means stopped, except when `/clear` or `/resume` replaced it with
    /// another session in the same process.
    pub fn implied_state(&self) -> Option<AgentState> {
        match self {
            Self::PromptSubmitted | Self::ToolStarted { .. } | Self::ToolFinished { .. } => {
                Some(AgentState::Working)
            }
            Self::NeedsInput { request, .. } => Some(AgentState::NeedsInput {
                request: request.clone(),
            }),
            Self::Stopped { .. } => Some(AgentState::Done),
            Self::SessionEnded { reason } => match reason.as_deref() {
                Some("clear" | "resume") => None,
                _ => Some(AgentState::Stopped),
            },
            Self::SessionStarted { .. }
            | Self::SubagentStarted { .. }
            | Self::SubagentStopped { .. }
            | Self::Ignored => None,
        }
    }
}

/// The tool Claude Code uses to ask the user a question.
const ASK_USER_QUESTION: &str = "AskUserQuestion";

/// Interprets a hook payload.
///
/// - `SessionStart`, `UserPromptSubmit`, `Stop`, `SessionEnd`,
///   `SubagentStart` and `SubagentStop` map to the signal of the same name.
/// - `PreToolUse` and `PostToolUse` carry a one-line [`tool_summary`];
///   `PostToolUse` also lists the [`written_files`]. A `PreToolUse` of
///   `AskUserQuestion` means the agent waits for an answer.
/// - `Notification` matters only for a permission prompt or a dialog
///   (`elicitation_dialog`, `elicitation_url_dialog`). Idle reminders and the
///   rest are ignored: the `Stop` hook already reported the end of the turn.
pub fn interpret(payload: &HookPayload) -> HookSignal {
    match &payload.hook_event_name {
        HookEventName::SessionStart => HookSignal::SessionStarted {
            session_id: payload.session_id.clone(),
            source: payload.source.as_deref().map(SessionSource::from),
            model: payload.model.clone(),
            title: payload.session_title.clone(),
            transcript_path: payload.transcript_path.clone(),
        },
        HookEventName::UserPromptSubmit => HookSignal::PromptSubmitted,
        HookEventName::PreToolUse => tool_started(payload),
        HookEventName::PostToolUse => tool_finished(payload),
        HookEventName::Notification => notification(payload),
        HookEventName::Stop => HookSignal::Stopped {
            last_message: payload.last_assistant_message.clone(),
        },
        HookEventName::SessionEnd => HookSignal::SessionEnded {
            reason: payload.reason.clone(),
        },
        HookEventName::SubagentStart => match &payload.agent_id {
            Some(id) => HookSignal::SubagentStarted {
                id: id.clone(),
                kind: payload.agent_type.clone(),
            },
            None => HookSignal::Ignored,
        },
        HookEventName::SubagentStop => match &payload.agent_id {
            Some(id) => HookSignal::SubagentStopped {
                id: id.clone(),
                last_message: payload.last_assistant_message.clone(),
            },
            None => HookSignal::Ignored,
        },
        HookEventName::Other(_) => HookSignal::Ignored,
    }
}

/// The tool and its input, when the payload names a tool.
fn tool(payload: &HookPayload) -> Option<(&str, &Value)> {
    let tool = payload.tool_name.as_deref()?;
    Some((tool, payload.tool_input.as_ref().unwrap_or(&Value::Null)))
}

fn tool_started(payload: &HookPayload) -> HookSignal {
    let Some((tool, input)) = tool(payload) else {
        return HookSignal::Ignored;
    };
    if tool == ASK_USER_QUESTION {
        return HookSignal::NeedsInput {
            request: InputRequest::Question,
            detail: questions(input),
        };
    }
    HookSignal::ToolStarted {
        tool: tool.to_owned(),
        summary: tool_summary(tool, input, payload.cwd.as_deref()),
    }
}

fn tool_finished(payload: &HookPayload) -> HookSignal {
    let Some((tool, input)) = tool(payload) else {
        return HookSignal::Ignored;
    };
    HookSignal::ToolFinished {
        tool: tool.to_owned(),
        summary: tool_summary(tool, input, payload.cwd.as_deref()),
        files: written_files(tool, input),
    }
}

/// The questions of an `AskUserQuestion` call, one per line.
fn questions(input: &Value) -> Option<String> {
    let questions: Vec<&str> = input
        .get("questions")?
        .as_array()?
        .iter()
        .filter_map(|question| question.get("question")?.as_str())
        .map(str::trim)
        .filter(|question| !question.is_empty())
        .collect();
    (!questions.is_empty()).then(|| questions.join("\n"))
}

fn notification(payload: &HookPayload) -> HookSignal {
    let detail = payload
        .message
        .as_deref()
        .map(str::trim)
        .filter(|message| !message.is_empty());
    let request = match payload.notification_type.as_deref() {
        Some("permission_prompt") => InputRequest::Permission {
            tool: detail.and_then(permission_tool),
        },
        Some("elicitation_dialog" | "elicitation_url_dialog") => InputRequest::Dialog,
        _ => return HookSignal::Ignored,
    };
    HookSignal::NeedsInput {
        request,
        detail: detail.map(str::to_owned),
    }
}

/// The tool named by a permission message such as `Claude needs your
/// permission to use Bash`.
fn permission_tool(message: &str) -> Option<String> {
    const MARKER: &str = "permission to use ";
    let start = message.find(MARKER)? + MARKER.len();
    let tool: String = message[start..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
        .collect();
    (!tool.is_empty()).then_some(tool)
}

#[cfg(test)]
mod tests;
