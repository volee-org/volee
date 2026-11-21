//! Reading what agents did: the last harness listing the daemon keeps, and
//! agent diffs. (Previewing, taking over and stopping were terminal
//! ownership and left with the v2 restart, `docs/v2/framing.md`.)

use std::collections::HashMap;
use std::sync::Arc;

use volee_core::harness::SessionKey;
use volee_core::reconcile::{ListedSession, Listing};
use volee_git::DiffFormat;
use volee_proto::methods::{DiffParams, DiffText};

use crate::error::DaemonError;
use crate::server::Shared;
use crate::support::{blocking, git};

/// The most diff text one request returns, in bytes. Past it, the text is
/// cut at a line break and flagged as truncated.
pub(crate) const MAX_DIFF_BYTES: usize = 512 * 1024;

/// The sessions of the last harness listing, which the daemon acts on.
#[derive(Debug, Default)]
pub(crate) struct Sessions {
    listed: HashMap<SessionKey, ListedSession>,
}

impl Sessions {
    /// Records a listing, which replaces the previous one of its harness.
    pub(crate) fn record(&mut self, listing: &Listing) {
        self.listed.retain(|key, _| key.harness != listing.harness);
        self.listed.extend(
            listing
                .sessions
                .iter()
                .map(|session| (session.key.clone(), session.clone())),
        );
    }

    /// The sessions the last listing of `listing`'s harness showed alive in
    /// a terminal, with their process, and that `listing` no longer shows:
    /// each one ended, or its conversation went on elsewhere (ADR 0034).
    pub(crate) fn left_terminal(&self, listing: &Listing) -> Vec<ListedSession> {
        self.listed
            .values()
            .filter(|session| {
                session.key.harness == listing.harness
                    && session.alive
                    && !session.background
                    && session.pid.is_some()
                    && !listing
                        .sessions
                        .iter()
                        .any(|listed| listed.key == session.key)
            })
            .cloned()
            .collect()
    }

    /// A session as the last listing showed it.
    pub(crate) fn listed(&self, key: &SessionKey) -> Option<&ListedSession> {
        self.listed.get(key)
    }

    /// Every session of the last listings.
    pub(crate) fn all(&self) -> impl Iterator<Item = &ListedSession> {
        self.listed.values()
    }
}

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
