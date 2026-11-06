//! Test doubles for code that drives a harness.
//!
//! Available in this crate's tests and, for other crates, behind the
//! `testing` feature:
//!
//! ```toml
//! [dev-dependencies]
//! volee-harness = { workspace = true, features = ["testing"] }
//! ```

use std::collections::VecDeque;
use std::io;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::runner::{CommandRunner, Invocation, Output, RunError};

/// A [`CommandRunner`] that records every invocation and answers with
/// scripted replies, in order.
///
/// An invocation without a scripted reply fails with [`RunError::Io`], so a
/// test notices unexpected commands.
///
/// ```
/// use std::time::Duration;
/// use volee_harness::testing::FakeRunner;
/// use volee_harness::{CommandRunner, Invocation, Output};
///
/// let runner = FakeRunner::new().with_reply(Ok(Output::success("2.1.319 (Claude Code)")));
/// let invocation = Invocation::new("claude", Duration::from_secs(5)).arg("--version");
/// assert_eq!(runner.run(&invocation)?.stdout, b"2.1.319 (Claude Code)");
/// assert_eq!(runner.calls(), [invocation]);
/// # Ok::<(), volee_harness::RunError>(())
/// ```
#[derive(Debug, Default)]
pub struct FakeRunner {
    replies: Mutex<VecDeque<Result<Output, RunError>>>,
    calls: Mutex<Vec<Invocation>>,
}

impl FakeRunner {
    /// A runner with no scripted reply.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scripts the reply to the next unanswered invocation.
    #[must_use]
    pub fn with_reply(self, reply: Result<Output, RunError>) -> Self {
        self.push_reply(reply);
        self
    }

    /// Scripts the reply to the next unanswered invocation, through a shared
    /// reference.
    pub fn push_reply(&self, reply: Result<Output, RunError>) {
        lock(&self.replies).push_back(reply);
    }

    /// Every invocation received so far, oldest first.
    pub fn calls(&self) -> Vec<Invocation> {
        lock(&self.calls).clone()
    }

    /// How many scripted replies are still unused.
    pub fn pending_replies(&self) -> usize {
        lock(&self.replies).len()
    }
}

impl CommandRunner for FakeRunner {
    fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
        lock(&self.calls).push(invocation.clone());
        lock(&self.replies).pop_front().unwrap_or_else(|| {
            Err(RunError::Io {
                program: invocation.program.clone(),
                source: io::Error::other(format!(
                    "FakeRunner has no reply scripted for {:?}",
                    invocation.args_lossy()
                )),
            })
        })
    }
}

/// Locks a mutex, ignoring poisoning: the data stays consistent because every
/// critical section is a single push, pop or clone.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn replays_in_order_and_records_calls() {
        let runner = FakeRunner::new()
            .with_reply(Ok(Output::success("first")))
            .with_reply(Ok(Output::failure(2, "second")));
        let a = Invocation::new("claude", Duration::from_secs(1)).arg("a");
        let b = Invocation::new("claude", Duration::from_secs(1)).arg("b");
        assert_eq!(runner.run(&a).unwrap().stdout, b"first");
        assert_eq!(runner.run(&b).unwrap().code, Some(2));
        assert_eq!(runner.calls(), [a, b]);
        assert_eq!(runner.pending_replies(), 0);
    }

    #[test]
    fn an_unscripted_call_fails() {
        let runner = FakeRunner::new();
        let invocation = Invocation::new("claude", Duration::from_secs(1)).arg("agents");
        let error = runner.run(&invocation).unwrap_err();
        assert!(matches!(error, RunError::Io { .. }));
        assert_eq!(runner.calls().len(), 1);
    }
}
