//! Durable storage of volee (ADR 0003).
//!
//! Two pieces live here:
//!
//! - [`Store`]: the append-only event log, kept in SQLite (WAL mode). The
//!   daemon appends [`volee_core::Event`]s and replays them into a
//!   [`volee_core::Registry`] when it starts.
//! - [`Spool`]: a directory of newline-delimited JSON files that `volee hook`
//!   appends to when the daemon is unreachable, so an agent is never slowed
//!   down or blocked and nothing is lost. The daemon drains it at start.
//!
//! This crate performs no network IO and knows nothing about harnesses: the
//! meaning of an event stays in `volee-core`.

mod error;
mod migrations;
mod seq;
pub mod spool;
mod store;

pub use error::StoreError;
pub use seq::Seq;
pub use spool::{Drained, Spool};
pub use store::Store;
