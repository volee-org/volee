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
/// back (`Ctrl+Z` in a Claude Code session). The agent keeps running. An
/// agent in a multiplexer pane is not taken over: its pane is brought
/// forward instead (ADR 0015).
pub(crate) async fn attach(input: &str) -> Result<()> {
    let client = connect_or_start().await?;
    let (agent, name) = agent(&client, input).await?;
    let command = client.call::<Attach>(AgentParams { agent }).await?;
    if let Some(focus) = command.focus {
        let multiplexer = text::one_line(&focus.multiplexer).into_owned();
        let session = text::one_line(&focus.session).into_owned();
        let attach = text::one_line(&focus.attach.join(" ")).into_owned();
        let steps = focus.steps;
        let outcome = tokio::task::spawn_blocking(move || bring_forward(&steps))
            .await
            .context("the focus task failed")?;
        match outcome {
            Ok(()) => println!(
                "{name} runs in a {multiplexer} pane: brought forward, in session {session}."
            ),
            // Focused in its session, which no window of the multiplexer
            // shows: say how to see it.
            Err((done, _)) if done > 0 && !attach.is_empty() => println!(
                "{name} is in front in {multiplexer} session {session}: run `{attach}` to see it."
            ),
            Err((_, error)) => return Err(error),
        }
        return Ok(());
    }
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

/// Runs the steps that bring a pane forward, in order, in this process's
/// environment: from a pane, the multiplexer acts on this pane's client. A
/// failure says how many steps ran fine before it.
fn bring_forward(steps: &[Vec<String>]) -> Result<(), (usize, anyhow::Error)> {
    for (done, step) in steps.iter().enumerate() {
        let Some((program, args)) = step.split_first() else {
            continue;
        };
        let output = Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .with_context(|| format!("cannot run {program}"))
            .map_err(|error| (done, error))?;
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr);
            let said = said.lines().next().unwrap_or_default().trim();
            let error = if said.is_empty() {
                anyhow::anyhow!("{program} ended with {}", output.status)
            } else {
                anyhow::anyhow!("{program}: {said}")
            };
            return Err((done, error));
        }
    }
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
pub(crate) async fn adopt(input: &str) -> Result<()> {
    let client = connect_or_start().await?;
    let (agent, name) = agent(&client, input).await?;
    client
        .call::<volee_proto::methods::Adopt>(AgentParams { agent })
        .await?;
    println!(
        "Adopted {name}: its conversation continues in a volee host. Take it over with `volee \
         attach {name}`; Ctrl+Q comes back."
    );
    Ok(())
}

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
