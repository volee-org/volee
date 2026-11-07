//! `volee spawn`: launch an agent in a new worktree.

use anyhow::{Context, Result};
use volee_core::role::Role;
use volee_core::{AgentId, Registry};
use volee_proto::methods::{Spawn, SpawnParams};

use crate::connect::connect_or_start;
use crate::{resolve, text};

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
    let registry = resolve::registry(&client).await?;
    let short = spawned.agent.short();
    let name = registry.agent(spawned.agent).map_or_else(
        || short.clone(),
        |agent| text::one_line(&agent.name).into_owned(),
    );
    println!("Launched {name} ({short}) in {}.", spawned.worktree);
    println!(
        "Follow it with `volee ls`, and take it over with `volee attach {}`.",
        command_word(&registry, spawned.agent, &name).unwrap_or(&short)
    );
    Ok(())
}

/// `name` when a person can type it as it is to reach `agent`: one word
/// for any shell, that names this agent and no other.
fn command_word<'a>(registry: &Registry, agent: AgentId, name: &'a str) -> Option<&'a str> {
    let one_word = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (one_word && resolve::agent(registry, name).ok() == Some(agent)).then_some(name)
}
