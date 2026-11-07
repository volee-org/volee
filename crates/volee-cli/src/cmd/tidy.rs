//! `volee tidy`: remove the worktrees of agents whose work landed, keeping
//! their branches (ADR 0011).

use anyhow::{Result, bail};
use volee_core::role::LaunchedBy;
use volee_core::{AgentId, Registry};
use volee_proto::methods::{Tidy, TidyOutcome, TidyParams};

use crate::connect::connect_or_start;
use crate::resolve;

/// Tidies one agent, or with `all` every agent volee launched that still has
/// a worktree; `dry_run` only says what would happen. A single agent that is
/// kept is an error, so scripts notice; with `all`, the list of what was
/// kept and why is the answer.
pub(crate) async fn run(agent: Option<&str>, all: bool, dry_run: bool) -> Result<()> {
    let client = connect_or_start().await?;
    let registry = resolve::registry(&client).await?;
    let targets = if all {
        candidates(&registry)
    } else {
        let id = resolve::agent(&registry, agent.unwrap_or_default())?;
        vec![(id, name(&registry, id))]
    };
    if targets.is_empty() {
        println!("No agent volee launched has a worktree left to tidy.");
        return Ok(());
    }
    let (mut removed, mut kept) = (0, 0);
    let mut refusal = None;
    for (agent, name) in targets {
        match client.call::<Tidy>(TidyParams { agent, dry_run }).await? {
            TidyOutcome::Removed { worktree, branch } => {
                removed += 1;
                println!(
                    "Removed {worktree} ({name}){}",
                    kept_branch(branch.as_deref())
                );
            }
            TidyOutcome::WouldRemove { worktree, branch } => {
                removed += 1;
                println!(
                    "Would remove {worktree} ({name}){}",
                    kept_branch(branch.as_deref())
                );
            }
            TidyOutcome::Kept { reasons } => {
                kept += 1;
                let why = reasons
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ");
                if all {
                    println!("Kept {name}: {why}.");
                } else {
                    refusal = Some(format!("kept {name}: {why}"));
                }
            }
        }
    }
    if let Some(refusal) = refusal {
        bail!(refusal);
    }
    if all {
        let verb = if dry_run { "would remove" } else { "removed" };
        println!("{verb} {removed}, kept {kept}.");
    }
    Ok(())
}

/// Every agent volee launched that still has a worktree, oldest first.
fn candidates(registry: &Registry) -> Vec<(AgentId, String)> {
    registry
        .agents()
        .filter(|agent| {
            agent.launched_by != LaunchedBy::Outside && agent.workspace.worktree.is_some()
        })
        .map(|agent| (agent.id, agent.name.clone()))
        .collect()
}

fn name(registry: &Registry, agent: AgentId) -> String {
    registry
        .agent(agent)
        .map_or_else(|| agent.short(), |record| record.name.clone())
}

fn kept_branch(branch: Option<&str>) -> String {
    branch.map_or_else(String::new, |branch| format!("; branch {branch} kept"))
}
