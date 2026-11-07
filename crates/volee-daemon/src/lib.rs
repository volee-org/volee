//! The volee daemon.
//!
//! One daemon per user owns all of volee's state (ADR 0003): it validates
//! events, appends them to the log, applies them to the registry and streams
//! them to subscribers. Clients (the interface, the command line, the MCP
//! server, hook commands) talk to it over a Unix socket with the protocol in
//! [`volee_proto`].
//!
//! Start it with [`Daemon::run`]; build its dependencies with [`Config`].

mod clock;
mod control;
mod discovery;
mod error;
mod evidence;
mod handlers;
mod launch;
mod log;
mod missions;
mod resume;
mod server;
mod sqlite;
mod state;
mod tidy;
mod watchers;

pub use clock::{Clock, ManualClock, SystemClock};
pub use discovery::DEFAULT_IDLE_INTERVAL as DEFAULT_IDLE_DISCOVERY_INTERVAL;
pub use discovery::DEFAULT_INTERVAL as DEFAULT_DISCOVERY_INTERVAL;
pub use error::DaemonError;
pub use evidence::EvidenceSources;
pub use log::{EventLog, LogError, MemoryLog};
pub use server::{Config, Daemon};
pub use sqlite::SqliteLog;
