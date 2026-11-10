//! `volee hook <event> --agent <id>`: called by the Claude Code hooks of the
//! agents volee launched.
//!
//! Rules: a hook never slows an agent down and never makes it fail. It sends
//! its event with a short timeout, prints only what the harness must read on
//! stdout, and falls back to the spool when the daemon is unreachable.

use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

use serde_json::Value;
use volee_core::AgentId;
use volee_proto::blocking::{self, BlockingError};
use volee_proto::methods::{HookIngest, HookParams, HookWait, WaitParams, WaitReply};
use volee_proto::paths::Paths;
use volee_store::Spool;

/// How long a hook waits for the daemon to acknowledge an event.
const INGEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Exit status that makes an `asyncRewake` hook wake the agent with stderr.
const WAKE: u8 = 2;

/// Runs one hook invocation.
pub(crate) fn run(event: &str, agent: AgentId, timeout_secs: u64) -> ExitCode {
    let mut input = String::new();
    // The harness always sends a payload; a missing one is not worth failing for.
    let _ = std::io::stdin().read_to_string(&mut input);
    let Ok(paths) = Paths::from_env() else {
        return ExitCode::SUCCESS;
    };
    if event == "wait" {
        return wait(&paths, agent, timeout_secs);
    }
    let payload = serde_json::from_str(&input).unwrap_or(Value::Null);
    let params = HookParams {
        agent,
        event: event.to_owned(),
        payload,
        pid: Some(std::os::unix::process::parent_id()),
    };
    match blocking::call::<HookIngest>(&paths.socket, params.clone(), INGEST_TIMEOUT) {
        Ok(reply) => {
            if let Some(stdout) = reply.stdout {
                print!("{stdout}");
            }
        }
        Err(BlockingError::Unreachable(_)) => spool(&paths, &params),
        Err(_) => {}
    }
    ExitCode::SUCCESS
}

/// Waits for a message for an idle agent; exits with status 2 and the
/// messages on stderr to wake it up.
fn wait(paths: &Paths, agent: AgentId, timeout_secs: u64) -> ExitCode {
    let params = WaitParams {
        agent,
        timeout_ms: timeout_secs.saturating_mul(1_000),
        pid: Some(std::os::unix::process::parent_id()),
    };
    let deadline = Duration::from_secs(timeout_secs.saturating_add(5));
    match blocking::call::<HookWait>(&paths.socket, params, deadline) {
        Ok(WaitReply { text: Some(text) }) => {
            eprintln!("{text}");
            ExitCode::from(WAKE)
        }
        _ => ExitCode::SUCCESS,
    }
}

fn spool(paths: &Paths, params: &HookParams) {
    if let Ok(record) = serde_json::to_value(params) {
        let _ = Spool::new(paths.spool_dir()).append(&record);
    }
}
