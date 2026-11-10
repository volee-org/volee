//! The files volee keeps for each agent (its hook settings, its MCP
//! configuration, the log of its setup commands), removed when the agent
//! leaves the registry: a person forgot it, or it expired with its session
//! (ADR 0027). Agents come and go all day; what they leave must go with
//! them.

use std::path::{Path, PathBuf};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use volee_core::{AgentId, Registry};
use volee_proto::methods::SeqEvent;
use volee_proto::paths::Paths;

use std::sync::Arc;

use crate::server::Shared;

/// Removes the directories of agents `registry` does not know: those that
/// left while no daemon ran, or before volee removed anything. Returns how
/// many it removed. Only directories named like an agent's id are touched.
pub(crate) fn sweep(paths: &Paths, registry: &Registry) -> usize {
    let Some(root) = agents_root(paths) else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(agent) = name.to_str().and_then(|name| name.parse::<AgentId>().ok()) else {
            continue;
        };
        if registry.agent(agent).is_none() && remove(&entry.path()) {
            removed += 1;
        }
    }
    removed
}

/// Removes an agent's directory as soon as the agent leaves the registry.
/// `events` must be subscribed before anything can commit, so that no
/// removal is missed.
pub(crate) fn spawn(
    shared: &Arc<Shared>,
    mut events: broadcast::Receiver<SeqEvent>,
) -> JoinHandle<()> {
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    if let Some(agent) = event.event.kind.removed_agent() {
                        let dir = shared.paths.agent_dir(agent);
                        let _ = tokio::task::spawn_blocking(move || remove(&dir)).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Removals may have been missed: look at every directory.
                    let registry = shared.state.lock().await.registry().clone();
                    let paths = shared.paths.clone();
                    let _ = tokio::task::spawn_blocking(move || sweep(&paths, &registry)).await;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

/// The directory that holds one directory per agent.
fn agents_root(paths: &Paths) -> Option<PathBuf> {
    // Any agent's directory has it as its parent.
    let probe = paths.agent_dir(AgentId::from_ulid(ulid::Ulid::nil()));
    probe.parent().map(Path::to_path_buf)
}

/// Removes a directory and says whether it was there and is gone.
fn remove(dir: &Path) -> bool {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            tracing::warn!(%error, dir = %dir.display(), "cannot remove an agent's files");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use volee_core::harness::HarnessKind;
    use volee_core::role::{LaunchedBy, Role};
    use volee_core::time::Timestamp;
    use volee_core::workspace::{Location, Workspace};
    use volee_core::{Event, EventKind};

    fn agent(n: u128) -> AgentId {
        AgentId::from_ulid(ulid::Ulid::from_parts(1_000, n))
    }

    #[test]
    fn only_the_directories_of_agents_that_left_are_removed() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(root.path());
        let mut registry = Registry::new();
        registry
            .apply(&Event::new(
                Timestamp::from_millis(1),
                EventKind::AgentRegistered {
                    agent: agent(1),
                    name: "kept".into(),
                    role: Role::Worker,
                    harness: HarnessKind::ClaudeCode,
                    launched_by: LaunchedBy::Human,
                    parent: None,
                    mission: None,
                    workspace: Workspace::at("/work"),
                    location: Location::Background,
                },
            ))
            .unwrap();
        for dir in [paths.agent_dir(agent(1)), paths.agent_dir(agent(2))] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("settings.json"), "{}").unwrap();
        }
        // Not an agent's directory: never touched.
        let other = agents_root(&paths).unwrap().join("notes");
        std::fs::create_dir_all(&other).unwrap();

        assert_eq!(sweep(&paths, &registry), 1);
        assert!(paths.agent_dir(agent(1)).join("settings.json").exists());
        assert!(!paths.agent_dir(agent(2)).exists());
        assert!(other.exists());
        assert_eq!(sweep(&paths, &registry), 0, "nothing left to remove");
    }

    #[test]
    fn a_state_directory_without_agents_is_fine() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(sweep(&Paths::rooted(root.path()), &Registry::new()), 0);
    }
}
