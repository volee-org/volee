//! `volee spawn`: launch an agent in a new worktree.

use anyhow::{Context, Result};
use volee_core::role::Role;
use volee_proto::methods::{Spawn, SpawnParams};

use crate::connect::connect_or_start;
use crate::resolve;

/// What `volee spawn` asks for.
#[derive(Debug)]
pub(crate) struct SpawnRequest {
    pub(crate) repo: String,
    pub(crate) branch: String,
    pub(crate) task: String,
    pub(crate) base: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) name: Option<String>,
}

/// Launches an agent that answers to the human.
pub(crate) async fn run(request: SpawnRequest) -> Result<()> {
    let config = resolve::load_config()?;
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let repo = resolve::repository(&config, &cwd, &request.repo)?;
    let client = connect_or_start().await?;
    let spawned = client
        .call::<Spawn>(SpawnParams {
            repo,
            branch: request.branch,
            base: request.base,
            task: request.task,
            role: Role::Lead,
            parent: None,
            mission: None,
            model: request.model,
            name: request.name,
        })
        .await?;
    println!(
        "Launched {} in {}. Follow it with `volee ls`.",
        spawned.agent.short(),
        spawned.worktree
    );
    Ok(())
}
