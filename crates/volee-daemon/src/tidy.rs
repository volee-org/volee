//! Tidying (ADR 0011): removing the worktree of an agent whose work is
//! delivered, then forgetting the agent.
//!
//! Nothing is removed without evidence. [`tidy_check`] decides from the
//! registry alone (an agent volee launched, a worktree volee created, fresh
//! delivery evidence that the work landed, a clean tree, an agent that is
//! not running), and it is checked again right before git removes anything.
//! The harness deletes the session first; git then removes the worktree
//! without ever forcing, and refuses a worktree with changes or with commits
//! no branch keeps. The branch is never deleted.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use volee_core::delivery::{TidyRefusal, tidy_check};
use volee_core::harness::SessionKey;
use volee_core::reconcile::ListedSession;
use volee_core::workspace::Workspace;
use volee_core::{AgentId, Event, EventKind};
use volee_git::Git;
use volee_harness::{Harness, HarnessError, WorktreeRemoval};
use volee_proto::methods::{TidyOutcome, TidyParams};

use crate::error::DaemonError;
use crate::server::Shared;
use crate::support::{blocking, git};

/// What tidying an agent needs, read from the registry once the verdict
/// allows it.
struct Plan {
    worktree: PathBuf,
    repo_root: PathBuf,
    branch: Option<String>,
    /// The agent's session, to look up in a fresh listing before the harness
    /// is asked to delete it.
    session_key: Option<SessionKey>,
    session: Option<ListedSession>,
}

/// How the harness answered the request to delete the session.
enum Deleted {
    /// The session is gone, or there was none to delete.
    Done,
    /// The harness kept the session (for Claude Code: a worktree it did not
    /// create, or commits it cannot see on a remote); the worktree is left
    /// to git.
    Kept,
}

/// `agents.tidy`: removes the agent's worktree when the evidence allows it,
/// keeps the branch, and forgets the agent. A dry run only gives the
/// verdict.
pub(crate) async fn tidy(
    shared: &Arc<Shared>,
    params: TidyParams,
) -> Result<TidyOutcome, DaemonError> {
    let mut plan = match plan(shared, params.agent).await? {
        Ok(plan) => plan,
        Err(reasons) => return Ok(TidyOutcome::Kept { reasons }),
    };
    if params.dry_run {
        return Ok(TidyOutcome::WouldRemove {
            worktree: plan.worktree.display().to_string(),
            branch: plan.branch,
        });
    }
    let git = git(shared)?;

    let harness = shared.harness.clone();
    if let (Some(harness), Some(key)) = (&harness, plan.session_key.take()) {
        plan.session = listed_session(harness, key).await?;
    }
    let deleted = match (&harness, &plan.session) {
        (Some(harness), Some(session)) => match delete_session(harness, session).await? {
            Ok(deleted) => deleted,
            Err(refusal) => {
                return Ok(TidyOutcome::Kept {
                    reasons: vec![refusal],
                });
            }
        },
        _ => Deleted::Done,
    };

    // The agent may have moved while the session was deleted (a message can
    // resume it): judge again right before git removes anything.
    if let Err(reasons) = plan_again(shared, params.agent).await? {
        return Ok(TidyOutcome::Kept { reasons });
    }
    if let Err(refusal) = remove_worktree(&git, &plan.repo_root, &plan.worktree).await? {
        return Ok(TidyOutcome::Kept {
            reasons: vec![refusal],
        });
    }
    record(shared, params.agent, &plan).await?;

    if let (Deleted::Kept, Some(harness), Some(session)) = (deleted, harness, plan.session.clone())
    {
        // Now that its worktree is gone, the harness has nothing left to keep
        // the session for. Best effort: the row only clutters its own list.
        let retried =
            blocking(move || harness.remove(&session, WorktreeRemoval::Delivered)).await?;
        if let Err(error) = retried {
            tracing::info!(agent = %params.agent, %error, "the harness keeps the tidied session");
        }
    }
    Ok(TidyOutcome::Removed {
        worktree: plan.worktree.display().to_string(),
        branch: plan.branch,
    })
}

/// The verdict and, when it allows tidying, what tidying needs.
async fn plan(
    shared: &Arc<Shared>,
    agent: AgentId,
) -> Result<Result<Plan, Vec<TidyRefusal>>, DaemonError> {
    let state = shared.state.lock().await;
    let record = state
        .registry()
        .agent(agent)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {agent}")))?;
    let worktree = match tidy_check(record) {
        Ok(worktree) => worktree,
        Err(reasons) => return Ok(Err(reasons)),
    };
    let Some(repo_root) = record.workspace.repo_root.clone() else {
        return Ok(Err(vec![TidyRefusal::NotAVoleeWorktree]));
    };
    let session_key = record.current_session().map(|link| link.key.clone());
    Ok(Ok(Plan {
        worktree,
        repo_root,
        branch: record.workspace.branch.clone(),
        session_key,
        session: None,
    }))
}

/// The agent's session as the harness lists it now, when it can be deleted.
/// Listed on demand: nothing keeps a listing between two requests.
async fn listed_session(
    harness: &Arc<dyn Harness>,
    key: SessionKey,
) -> Result<Option<ListedSession>, DaemonError> {
    let harness = Arc::clone(harness);
    let listing = blocking(move || harness.list()).await?;
    Ok(listing.ok().and_then(|listing| {
        listing
            .sessions
            .into_iter()
            .find(|session| session.key == key && session.handle.is_some())
    }))
}

/// The verdict again, from the registry as it is now.
async fn plan_again(
    shared: &Arc<Shared>,
    agent: AgentId,
) -> Result<Result<(), Vec<TidyRefusal>>, DaemonError> {
    let state = shared.state.lock().await;
    let record = state
        .registry()
        .agent(agent)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {agent}")))?;
    Ok(tidy_check(record).map(drop))
}

/// Asks the harness to delete the session, with the delivery evidence that
/// allows it to delete the worktree too.
async fn delete_session(
    harness: &Arc<dyn Harness>,
    session: &ListedSession,
) -> Result<Result<Deleted, TidyRefusal>, DaemonError> {
    let (harness, session) = (Arc::clone(harness), session.clone());
    let answer = blocking(move || harness.remove(&session, WorktreeRemoval::Delivered)).await?;
    Ok(match answer {
        Ok(())
        | Err(HarnessError::UnknownSession { .. } | HarnessError::NotControllable { .. }) => {
            Ok(Deleted::Done)
        }
        Err(HarnessError::SessionKept { reason, .. }) => {
            tracing::info!(%reason, "the harness kept the session; git removes the worktree");
            Ok(Deleted::Kept)
        }
        Err(error) => Err(TidyRefusal::KeptByHarness {
            detail: error.to_string(),
        }),
    })
}

/// Unlocks the worktree when the harness left it locked, then removes it.
/// volee-git never forces: it refuses a worktree with changes, or whose
/// commits no branch keeps, and keeps the branch.
async fn remove_worktree(
    git: &Arc<dyn Git>,
    repo_root: &Path,
    worktree: &Path,
) -> Result<Result<(), TidyRefusal>, DaemonError> {
    let (git, root, path) = (
        Arc::clone(git),
        repo_root.to_path_buf(),
        worktree.to_path_buf(),
    );
    let removed = blocking(move || {
        let locked = git
            .worktrees(&root)?
            .iter()
            .any(|known| same_path(&known.path, &path) && known.locked.is_some());
        if locked {
            git.unlock_worktree(&root, &path)?;
        }
        git.remove_worktree(&root, &path)
    })
    .await?;
    Ok(removed.map_err(|error| TidyRefusal::RefusedByGit {
        detail: error.to_string(),
    }))
}

/// Whether two paths name the same directory, through symbolic links when
/// both exist.
fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

/// Records the removal: the workspace without its worktree, then the agent
/// forgotten, so that it leaves every view. Its events stay in the log.
async fn record(shared: &Arc<Shared>, agent: AgentId, plan: &Plan) -> Result<(), DaemonError> {
    let mut state = shared.state.lock().await;
    let Some(record) = state.registry().agent(agent) else {
        // Tidied meanwhile by another request.
        return Ok(());
    };
    let workspace = Workspace {
        worktree: None,
        cwd: plan.repo_root.clone(),
        ..record.workspace.clone()
    };
    let now = state.now();
    state.commit(vec![
        Event::new(now, EventKind::WorkspaceUpdated { agent, workspace }),
        Event::new(now, EventKind::AgentForgotten { agent }),
    ])?;
    Ok(())
}
