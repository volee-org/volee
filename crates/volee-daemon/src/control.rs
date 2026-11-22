//! Reading what agents did: the last harness listing the daemon keeps, and
//! agent diffs. (Previewing, taking over and stopping were terminal
//! ownership and left with the v2 restart, `docs/v2/framing.md`.)

use std::sync::Arc;

use volee_git::DiffFormat;
use volee_proto::methods::{DiffParams, DiffText};

use crate::error::DaemonError;
use crate::server::Shared;
use crate::support::{blocking, git};

/// The most diff text one request returns, in bytes. Past it, the text is
/// cut at a line break and flagged as truncated.
pub(crate) const MAX_DIFF_BYTES: usize = 512 * 1024;

/// `agents.diff`: the agent's changes against the merge base with the branch
/// it started from, as statistics or as a patch, at most
/// [`MAX_DIFF_BYTES`].
pub(crate) async fn diff(
    shared: &Arc<Shared>,
    params: DiffParams,
) -> Result<DiffText, DaemonError> {
    let git = git(shared)?;
    let (name, dir, base) = {
        let state = shared.state.lock().await;
        let agent = state
            .registry()
            .agent(params.agent)
            .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {}", params.agent)))?;
        let workspace = &agent.workspace;
        (
            agent.name.clone(),
            workspace
                .worktree
                .clone()
                .unwrap_or_else(|| workspace.cwd.clone()),
            workspace.base.clone(),
        )
    };
    if !dir.is_dir() {
        return Err(DaemonError::Refused(format!(
            "the directory of {name} is gone: {}",
            dir.display()
        )));
    }
    let info = {
        let (git, dir) = (Arc::clone(&git), dir.clone());
        blocking(move || git.repo_info(&dir)).await??
    }
    .ok_or_else(|| DaemonError::Refused(format!("{name} does not work in a git repository")))?;
    let base = if let Some(base) = base {
        base
    } else {
        let (git, root) = (Arc::clone(&git), info.main_checkout);
        blocking(move || git.default_branch(&root))
            .await??
            .ok_or_else(|| {
                DaemonError::Refused(format!(
                    "volee does not know which branch {name} started from, and its repository \
                     has no main, master or develop branch"
                ))
            })?
    };
    let format = if params.full {
        DiffFormat::Patch
    } else {
        DiffFormat::Stat
    };
    let output = blocking(move || git.diff(&dir, &base, format, MAX_DIFF_BYTES)).await??;
    Ok(DiffText {
        text: output.text,
        truncated: output.truncated,
    })
}
