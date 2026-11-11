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
    pub(crate) harness: Option<String>,
}

/// Launches an agent that answers to the human.
pub(crate) async fn run(request: SpawnRequest) -> Result<()> {
    resolve::as_the_person(
        "launches an agent from the command line (a lead calls spawn_agent, so that the agent \
         reports to it)",
    )?;
    let config = resolve::load_config()?;
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let repo = resolve::repository(&config, &cwd, &request.repo)?;
    if let Some(note) = setup_note(&config, &repo) {
        println!("{note}");
    }
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
            harness: request.harness,
        })
        .await?;
    let registry = resolve::registry(&client).await?;
    let short = spawned.agent.short();
    let name = registry.agent(spawned.agent).map_or_else(
        || short.clone(),
        |agent| text::one_line(&agent.name).into_owned(),
    );
    println!("Launched {name} ({short}) in {}.", spawned.worktree);
    if let Some(log) = &spawned.setup_log {
        println!("Its setup commands ran first; what they printed is in {log}.");
    }
    println!(
        "Follow it with `volee ls`, and take it over with `volee attach {}`.",
        command_word(&registry, spawned.agent, &name).unwrap_or(&short)
    );
    Ok(())
}

/// What `volee spawn` says before it waits for a repository's setup
/// commands (ADR 0017), which can take minutes; `None` without any.
fn setup_note(config: &volee_config::Config, repo: &str) -> Option<String> {
    let commands = &config.repos.get(repo)?.setup;
    match commands.as_slice() {
        [] => None,
        [one] => Some(format!(
            "Setting up the new worktree first (`{}`); this may take a while.",
            text::one_line(one)
        )),
        many => Some(format!(
            "Setting up the new worktree first ({} commands); this may take a while.",
            many.len()
        )),
    }
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn config(text: &str) -> volee_config::Config {
        volee_config::Config::parse(text, Path::new("config.toml"), Path::new("/home/ada")).unwrap()
    }

    #[test]
    fn spawn_says_it_sets_up_the_worktree_first() {
        let config = config(
            "[repos.shop]\npath = \"/srv/shop\"\nsetup = [\"bun install\"]\n\
             [repos.docs]\npath = \"/srv/docs\"\n\
             [repos.app]\npath = \"/srv/app\"\nsetup = [\"a\", \"b\"]\n",
        );
        assert_eq!(
            setup_note(&config, "shop").as_deref(),
            Some("Setting up the new worktree first (`bun install`); this may take a while.")
        );
        assert_eq!(
            setup_note(&config, "app").as_deref(),
            Some("Setting up the new worktree first (2 commands); this may take a while.")
        );
        assert_eq!(setup_note(&config, "docs"), None);
        assert_eq!(setup_note(&config, "/srv/elsewhere"), None);
    }
}
