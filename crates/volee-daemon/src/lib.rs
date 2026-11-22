//! The volee daemon.
//!
//! One daemon per user owns all of volee's state (ADR 0003): it validates
//! events, appends them to the log, applies them to the registry and streams
//! them to subscribers. Clients (the interface, the command line, the MCP
//! server, hook commands) talk to it over a Unix socket with the protocol in
//! [`volee_proto`].
//!
//! Start it with [`Daemon::run`]; build its dependencies with [`Config`].

mod actions;
mod agents_view;
mod assess;
mod clock;
mod control;
mod decisions;
mod error;
mod evidence;
mod files;
mod handlers;
pub mod hook_server;
mod join;
mod log;
mod missions;
mod review;
mod rules;
mod server;
mod sqlite;
mod state;
mod support;
mod tidy;
mod transcript;
mod web;
mod work;

pub use actions::FixSources;
pub use agents_view::AgentsViewConfig;
pub use clock::{Clock, ManualClock, SystemClock};
pub use decisions::HoldSettings;
pub use error::DaemonError;
pub use evidence::EvidenceSources;
pub use hook_server::HookServerConfig;
pub use log::{EventLog, LogError, MemoryLog};
pub use server::{Config, Daemon};
pub use sqlite::SqliteLog;
