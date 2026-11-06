//! Integration tests of `GitCli` against real temporary repositories.
//!
//! Each test builds the repositories it needs in its own sandbox (see
//! [`fixture`]); a bare repository plays `origin`, so pushes and fetches
//! never leave the machine.

mod fixture;

mod delivery;
mod diff;
mod errors;
mod fetch;
mod repo_info;
mod worktrees;
