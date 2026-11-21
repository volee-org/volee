//! Small helpers shared by the daemon's handlers.

use std::sync::Arc;

use volee_git::Git;

use crate::error::DaemonError;
use crate::server::Shared;

/// git, or why the daemon has none.
pub(crate) fn git(shared: &Shared) -> Result<Arc<dyn Git>, DaemonError> {
    shared
        .git
        .clone()
        .ok_or_else(|| DaemonError::Unavailable("git is not available to this daemon".into()))
}

/// Runs blocking work (git, the harness) on the blocking thread pool.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, DaemonError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| DaemonError::Unavailable(format!("a background task failed: {error}")))
}
