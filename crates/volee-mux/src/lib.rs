//! Location adapters ([ADR 0008], [ADR 0015]): for agents that run in a
//! terminal multiplexer pane, find the pane, show what it shows, and bring it
//! forward.
//!
//! A [`Multiplexer`] is optional: the daemon uses one only when its program is
//! installed, and a multiplexer that does not answer changes nothing. It only
//! reads panes and focuses them: it never types into a pane, and never
//! creates, closes or moves one. [`Tuios`] is the first adapter.
//!
//! Pane addresses are written `<session>:<pane>`, in the multiplexer's own
//! terms, so that a person can be told which session holds the pane
//! ([`volee_core::workspace::Location::label`]).
//!
//! [ADR 0008]: https://volee-org.github.io/volee/adr/0008-multiplexers-are-optional.html
//! [ADR 0015]: https://volee-org.github.io/volee/adr/0015-tuios-location-adapter.html
#![warn(missing_docs)]

mod process;
mod screen;
mod tuios;

use std::fmt;

use volee_core::harness::SessionKey;

pub use tuios::Tuios;

/// A session that may run in a pane: an interactive session of a harness,
/// and its process while it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hosted {
    /// The session.
    pub key: SessionKey,
    /// Its process, when the harness lists one.
    pub pid: Option<u32>,
}

/// A session found in a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    /// The session.
    pub key: SessionKey,
    /// The pane, as `<session>:<pane>`.
    pub target: String,
}

/// What is known about a multiplexer on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiplexerStatus {
    /// Its name, such as `tuios`.
    pub name: String,
    /// Whether volee can use it now: installed, with a server running.
    pub available: bool,
    /// Its version, when known.
    pub version: Option<String>,
    /// Why it is unavailable, or a note.
    pub detail: Option<String>,
}

/// Why a multiplexer could not answer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MuxError {
    /// It cannot be used now: not installed, or no server runs.
    #[error("{0}")]
    Unavailable(String),
    /// It answered with an error, such as a pane that no longer exists.
    #[error("{message}")]
    Refused {
        /// Its code for the error, such as `window_not_found`.
        code: String,
        /// What it said.
        message: String,
    },
    /// It answered something volee cannot read.
    #[error("unexpected output from `{command}`")]
    Unexpected {
        /// The command.
        command: String,
    },
    /// A pane address that is not `<session>:<pane>`.
    #[error("`{0}` is not a pane address")]
    BadTarget(String),
}

/// A terminal multiplexer whose panes volee can find, show and focus.
///
/// Implementations run programs and may block for a moment: call them off
/// the async runtime's threads.
pub trait Multiplexer: fmt::Debug + Send + Sync {
    /// Its name, as [`Location::Pane`](volee_core::workspace::Location::Pane)
    /// records it.
    fn name(&self) -> &'static str;

    /// Whether volee can use it now, and its version.
    fn status(&self) -> MultiplexerStatus;

    /// The panes some of `sessions` run in. Sessions that run in no pane of
    /// this multiplexer are left out.
    fn locate(&self, sessions: &[Hosted]) -> Result<Vec<Located>, MuxError>;

    /// What the pane at `target` shows now, as terminal output that draws
    /// it: its rows, placed with cursor moves, with their colors.
    fn capture(&self, target: &str) -> Result<String, MuxError>;

    /// The commands that bring the pane at `target` forward, in order: each
    /// is a program and its arguments. The client runs them in its own
    /// environment, so that the multiplexer acts on the person's own client.
    fn focus_steps(&self, target: &str) -> Result<Vec<Vec<String>>, MuxError>;

    /// The command a person runs to see the session that holds the pane at
    /// `target`, when no client of the multiplexer shows it; none when the
    /// multiplexer has no such command.
    fn attach_command(&self, _target: &str) -> Option<Vec<String>> {
        None
    }
}

/// Splits a pane address into its session and its pane. The pane part is the
/// text after the last colon, so a session name may hold colons.
pub fn split_target(target: &str) -> Result<(&str, &str), MuxError> {
    target
        .rsplit_once(':')
        .filter(|(session, pane)| !session.is_empty() && !pane.is_empty())
        .ok_or_else(|| MuxError::BadTarget(target.to_owned()))
}

/// The address of a pane: `<session>:<pane>`.
pub fn target(session: &str, pane: &str) -> String {
    format!("{session}:{pane}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_round_trip_with_colons_in_the_session() {
        assert_eq!(split_target("work:3f2a").unwrap(), ("work", "3f2a"));
        assert_eq!(
            split_target(&target("host:work", "3f2a")).unwrap(),
            ("host:work", "3f2a")
        );
        for bad in ["work", ":3f2a", "work:", ""] {
            assert!(
                matches!(split_target(bad), Err(MuxError::BadTarget(_))),
                "{bad}"
            );
        }
    }
}
