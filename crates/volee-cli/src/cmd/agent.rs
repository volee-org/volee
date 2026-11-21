//! `volee diff` and `volee forget`: acting on one agent, named as a person
//! types it (its name, short id or id).

use anyhow::Result;
use volee_core::AgentId;
use volee_proto::client::Client;
use volee_proto::methods::{AgentParams, Diff, DiffParams, Forget};

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
