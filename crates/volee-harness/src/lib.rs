//! Harness adapters: how volee launches, observes, previews, takes over and
//! messages coding agents.
//!
//! A harness is the program that runs the model for an agent. volee never
//! owns agent processes: the harness does ([ADR 0002]). This crate turns the
//! harness's documented commands and hooks into volee's domain types
//! ([ADR 0004], [ADR 0006]), and generates the per-agent configuration that
//! connects an agent to volee ([ADR 0005]).
//!
//! Claude Code is the first harness:
//!
//! - [`ClaudeCode`] lists every session on the machine (`claude agents --json
//!   --all`), launches and resumes background sessions (`claude --bg`), stops,
//!   removes and previews them, and builds the `claude attach` command used to
//!   take one over.
//! - [`hooks`] parses the JSON that Claude Code hooks receive, interprets it
//!   as a [`hooks::HookSignal`], and generates the per-agent settings and MCP
//!   configuration passed with `--settings` and `--mcp-config`.
//!
//! The daemon talks to every harness through the [`Harness`] trait, so a
//! second harness can be added without touching it, and the daemon's tests
//! use fakes.
//!
//! # Processes and blocking
//!
//! Every program runs through a [`CommandRunner`] with a timeout. The real one
//! is [`SystemRunner`]; tests use a runner that records calls and replays
//! fixtures (see `testing::FakeRunner`, behind the `testing` feature), so no
//! test needs a real `claude`. Calls block until the program exits: async
//! callers run them on a blocking thread, for example with
//! `tokio::task::spawn_blocking`.
//!
//! # Example
//!
//! ```
//! use volee_harness::{ClaudeCode, CommandRunner, Invocation, Output, RunError, Version};
//!
//! /// A runner that answers every command with the same output.
//! #[derive(Debug)]
//! struct Canned(&'static str);
//!
//! impl CommandRunner for Canned {
//!     fn run(&self, _invocation: &Invocation) -> Result<Output, RunError> {
//!         Ok(Output::success(self.0))
//!     }
//! }
//!
//! let claude = ClaudeCode::new(Canned("2.1.319 (Claude Code)\n"));
//! assert_eq!(claude.version()?, Version::new(2, 1, 319));
//! # Ok::<(), volee_harness::HarnessError>(())
//! ```
//!
//! [ADR 0002]: https://volee-org.github.io/volee/adr/0002-harness-owns-processes.html
//! [ADR 0004]: https://volee-org.github.io/volee/adr/0004-sources-and-reconciliation.html
//! [ADR 0005]: https://volee-org.github.io/volee/adr/0005-harness-neutral-agent-protocol.html
//! [ADR 0006]: https://volee-org.github.io/volee/adr/0006-messaging.html
#![warn(missing_docs)]

pub mod claude;
mod error;
mod harness;
pub mod hooks;
mod lenient;
pub mod opencode;
mod runner;
mod spec;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod version;

pub use claude::ClaudeCode;
pub use error::{HarnessError, ParseError, RequestError};
pub use harness::{Capabilities, Harness, WorktreeRemoval};
pub use runner::{CommandRunner, Invocation, Output, RunError, SystemRunner};
pub use spec::{Completion, HostLaunch, PermissionMode, SpawnSpec, Spawned};
pub use version::{ParseVersionError, Version};
