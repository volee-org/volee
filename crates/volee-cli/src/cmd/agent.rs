//! `volee attach`, `volee logs`, `volee diff` and `volee stop`: acting on one
//! agent, named as a person types it (its name, short id or id).

use std::process::Command;

use anyhow::{Context, Result, bail};
use volee_core::AgentId;
use volee_proto::client::Client;
use volee_proto::methods::{AgentParams, Attach, Diff, DiffParams, Forget, Preview, Stop};

use crate::connect::connect_or_start;
use crate::{resolve, text};

/// The agent `input` names, with its display name, safe to print: an agent
/// that a coordinator launched got its name from a model.
async fn agent(client: &Client, input: &str) -> Result<(AgentId, String)> {
    let registry = resolve::registry(client).await?;
    let id = resolve::agent(&registry, input)?;
    let name = registry.agent(id).map_or_else(
        || id.short(),
        |agent| text::one_line(&agent.name).into_owned(),
    );
    Ok((id, name))
}

/// Takes the agent over in this terminal, and returns when the person comes
/// back (`Ctrl+Z` in a Claude Code session). The agent keeps running.
pub(crate) async fn attach(input: &str) -> Result<()> {
    let client = connect_or_start().await?;
    let (agent, name) = agent(&client, input).await?;
    let command = client.call::<Attach>(AgentParams { agent }).await?;
    println!("Taking over {name}. {}", command.hint);
    let program = command.program.clone();
    let mut process = Command::new(&command.program);
    process.args(&command.args);
    if let Some(cwd) = &command.cwd {
        process.current_dir(cwd);
    }
    for key in &command.env_remove {
        process.env_remove(key);
    }
    // The terminal belongs to the agent until it exits.
    let status = tokio::task::spawn_blocking(move || process.status())
        .await
        .context("the take-over task failed")?
        .with_context(|| format!("cannot run {program}"))?;
    if !status.success() {
        bail!("{program} ended with {status}");
    }
    println!("Back from {name}.");
    Ok(())
}

/// Prints the agent's recent terminal output, with its colors and cursor
/// moves, as the harness recorded it.
pub(crate) async fn logs(input: &str) -> Result<()> {
    let client = connect_or_start().await?;
    let (agent, name) = agent(&client, input).await?;
    let preview = client.call::<Preview>(AgentParams { agent }).await?;
    // The screen as it looks now, not the raw output that drew it.
    let screen = volee_tui::screen_text(&preview.screen);
    if screen.is_empty() {
        println!("{name} has shown nothing yet.");
    } else {
        println!("{screen}");
    }
    Ok(())
}

/// Prints the agent's changes against the branch it started from: a summary
/// by file, or the full patch.
pub(crate) async fn diff(input: &str, full: bool) -> Result<()> {
    let client = connect_or_start().await?;
    let (agent, name) = agent(&client, input).await?;
    let diff = client.call::<Diff>(DiffParams { agent, full }).await?;
    if diff.text.trim().is_empty() {
        println!("{name} has no changes against the branch it started from.");
        return Ok(());
    }
    print!("{}", diff.text);
    if !diff.text.ends_with('\n') {
        println!();
    }
    if diff.truncated {
        eprintln!("volee: the diff of {name} is longer than this; see the rest in its worktree.");
    }
    Ok(())
}

/// Stops listing an agent that no longer runs.
pub(crate) async fn forget(input: &str) -> Result<()> {
    let client = connect_or_start().await?;
    let (agent, name) = agent(&client, input).await?;
    client.call::<Forget>(AgentParams { agent }).await?;
    println!(
        "Forgot {name}: volee no longer lists it. Nothing else changed: its sessions, worktree \
         and branch stay as they are."
    );
    Ok(())
}

/// Stops an agent volee launched. Its conversation is kept, and volee does
/// not resume it until the human writes to it.
pub(crate) async fn stop(input: &str) -> Result<()> {
    let client = connect_or_start().await?;
    let (agent, name) = agent(&client, input).await?;
    client.call::<Stop>(AgentParams { agent }).await?;
    println!(
        "Stopped {name}. Its conversation is kept, and volee does not resume it until you send \
         it a message (`volee msg send {name} ...`)."
    );
    Ok(())
}
