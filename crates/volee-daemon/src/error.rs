//! Daemon errors and their mapping to protocol errors.

use std::io;
use std::path::PathBuf;

use volee_core::registry::{ApplyError, RouteError};
use volee_proto::rpc::{self, RpcError};

use crate::log::LogError;

/// Everything that can go wrong in the daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// Another daemon already runs for this user.
    #[error("another volee daemon is already running (lock {0})")]
    AlreadyRunning(PathBuf),
    /// Setting up the socket or the state directory failed.
    #[error("{action} {path}: {source}")]
    Setup {
        /// What was attempted.
        action: &'static str,
        /// On which path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The event log failed.
    #[error(transparent)]
    Log(#[from] LogError),
    /// A stored event does not fit the registry: the log is corrupted.
    #[error("event {seq} does not fit the registry: {source}")]
    Corrupted {
        /// The sequence number of the event.
        seq: u64,
        /// Why it does not fit.
        #[source]
        source: ApplyError,
    },
    /// A request named something that does not exist.
    #[error("{0}")]
    NotFound(String),
    /// A request was refused by a rule.
    #[error("{0}")]
    Refused(String),
    /// A request's parameters are invalid.
    #[error("{0}")]
    InvalidParams(String),
    /// The client speaks another protocol version.
    #[error("{0}")]
    VersionMismatch(String),
    /// An event built from a request did not apply.
    #[error(transparent)]
    Apply(#[from] ApplyError),
}

impl From<RouteError> for DaemonError {
    fn from(error: RouteError) -> Self {
        match error {
            RouteError::UnknownAgent(_) => Self::NotFound(error.to_string()),
            RouteError::ToSelf | RouteError::NotPermitted { .. } => {
                Self::Refused(error.to_string())
            }
        }
    }
}

impl From<DaemonError> for RpcError {
    fn from(error: DaemonError) -> Self {
        let code = match &error {
            DaemonError::NotFound(_) => rpc::NOT_FOUND,
            DaemonError::Refused(_) => rpc::REFUSED,
            DaemonError::InvalidParams(_) => rpc::INVALID_PARAMS,
            DaemonError::VersionMismatch(_) => rpc::VERSION_MISMATCH,
            _ => rpc::INTERNAL_ERROR,
        };
        Self::new(code, error.to_string())
    }
}
