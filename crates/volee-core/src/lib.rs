//! Domain model of volee.
//!
//! This crate holds the types and rules shared by every part of volee:
//! agents and their sessions, roles and states, messages and threads,
//! missions and delivery evidence. It performs no IO and never reads the
//! clock, so everything in it is tested without a terminal, a database, git
//! or a coding harness.
//!
//! The state is event-sourced: [`event::Event`]s are applied in order to a
//! [`registry::Registry`], and [`reconcile::reconcile`] turns a harness's
//! session listing into new events.
//!
//! See the [architecture overview](https://volee-org.github.io/volee/architecture/overview.html)
//! for how the crates fit together.

pub mod agent;
pub mod delivery;
pub mod event;
pub mod harness;
pub mod ids;
pub mod message;
pub mod mission;
pub mod reconcile;
pub mod registry;
pub mod role;
pub mod state;
pub mod time;
pub mod workspace;

pub use agent::Agent;
pub use event::{Event, EventKind};
pub use ids::{AgentId, MessageId, MissionId, ThreadId};
pub use registry::Registry;
pub use state::AgentState;

/// The version of volee, as declared in the workspace manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_semver() {
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "unexpected version {VERSION}");
        assert!(parts.iter().all(|p| p.parse::<u64>().is_ok()));
    }
}
