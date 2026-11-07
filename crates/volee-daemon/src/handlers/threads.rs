//! `threads.summarize`: a thread summarized by a model, when the person
//! turned summaries on.
//!
//! The model runs through the harness's one-off completion (for Claude Code,
//! `claude -p` with no tools, no MCP server, no hooks and no saved session),
//! in a directory of volee's own, so that it reads nothing of the person's
//! projects and discovery never takes it for an agent.

use std::fmt::Write as _;
use std::sync::Arc;

use volee_core::message::{Message, Party};
use volee_core::{Registry, ThreadId};
use volee_harness::Completion;
use volee_proto::methods::{SummarizeParams, ThreadSummary};

use crate::error::DaemonError;
use crate::launch;
use crate::server::Shared;

/// The most recent messages a summary covers.
const MAX_MESSAGES: usize = 200;
/// The most characters of one message's body a summary reads.
const MAX_BODY: usize = 4_000;
/// The most characters of thread a summary reads, keeping the latest.
const MAX_TRANSCRIPT: usize = 60_000;

const SYSTEM_PROMPT: &str = "You summarize a working conversation between coding agents and the \
human who supervises them, for that human. Reply in at most 8 short bullet points of plain \
text: what was decided, what was delivered (keep every link and pull request number), the \
questions still waiting for an answer and who must answer them, and what is blocked. Do not \
invent anything. If the conversation is too short to summarize, say so in one line.";

/// The subdirectory of volee's state where summaries run.
pub(crate) const SUMMARIES_DIR: &str = "summaries";

pub(super) async fn summarize(
    shared: &Arc<Shared>,
    params: SummarizeParams,
) -> Result<ThreadSummary, DaemonError> {
    let settings = &shared.settings.messages;
    if !settings.summaries {
        return Err(DaemonError::Refused(
            "thread summaries are off: set `summaries = true` under [messages] in the \
             configuration. Each summary runs Claude Code with your account, usually for a few \
             cents."
                .into(),
        ));
    }
    let harness = shared.harness.clone().ok_or_else(|| {
        DaemonError::Unavailable(
            "Claude Code is not available to this daemon; see `volee daemon status`".into(),
        )
    })?;
    let (prompt, messages) = {
        let state = shared.state.lock().await;
        transcript(state.registry(), params.thread)?
    };
    if messages == 0 {
        return Ok(ThreadSummary {
            summary: "This thread has no messages yet.".into(),
            messages,
            model: String::new(),
        });
    }
    let cwd = shared.paths.state_dir.join(SUMMARIES_DIR);
    std::fs::create_dir_all(&cwd).map_err(|source| DaemonError::Write {
        path: cwd.clone(),
        source,
    })?;
    let model = settings.summary_model().to_owned();
    let request = Completion {
        cwd,
        model: Some(model.clone()),
        system_prompt: SYSTEM_PROMPT.to_owned(),
        prompt,
    };
    let summary = launch::blocking(move || harness.complete(&request)).await??;
    Ok(ThreadSummary {
        summary,
        messages,
        model,
    })
}

/// The thread as the model reads it, latest messages kept, and how many
/// messages it shows.
fn transcript(registry: &Registry, thread: ThreadId) -> Result<(String, usize), DaemonError> {
    let record = registry
        .thread(thread)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown thread {thread}")))?;
    let messages = registry.thread_messages(thread);
    let shown = &messages[messages.len().saturating_sub(MAX_MESSAGES)..];
    let mut lines: Vec<String> = shown
        .iter()
        .map(|message| line(registry, message))
        .collect();
    // Keep the latest messages that fit.
    let mut total: usize = lines.iter().map(String::len).sum();
    while total > MAX_TRANSCRIPT && lines.len() > 1 {
        total -= lines.remove(0).len();
    }
    let mut text = format!("Thread: {}\n\n", record.title);
    for line in &lines {
        text.push_str(line);
    }
    Ok((text, lines.len()))
}

fn line(registry: &Registry, message: &Message) -> String {
    let to: Vec<String> = message
        .receipts
        .iter()
        .map(|receipt| name(registry, receipt.to))
        .collect();
    let mut body: String = message.body.chars().take(MAX_BODY).collect();
    if body.len() < message.body.len() {
        body.push_str(" [...]");
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "- {} to {} ({}): {}",
        name(registry, message.from),
        to.join(", "),
        message.kind.label(),
        body.trim()
    );
    out
}

fn name(registry: &Registry, party: Party) -> String {
    match party {
        Party::Human => "the human".to_owned(),
        Party::System => "volee".to_owned(),
        Party::Agent(id) => registry
            .agent(id)
            .map_or_else(|| id.short(), |agent| agent.name.clone()),
    }
}
