//! Helpers shared by the daemon's integration tests.

// Each test file uses its own subset of these helpers.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};
use volee_core::AgentId;
use volee_core::harness::{HarnessKind, SessionKey, SessionOrigin};
use volee_core::reconcile::{ListedSession, Listing};
use volee_harness::{Capabilities, Harness, HarnessError, SpawnSpec, Spawned, Version};

/// A harness whose listing the test sets, and which records launches.
#[derive(Debug, Default)]
pub(crate) struct FakeHarness {
    sessions: Mutex<Vec<ListedSession>>,
    failing: Mutex<bool>,
    launches: Mutex<Vec<SpawnSpec>>,
    refuse_launches: Mutex<bool>,
}

impl FakeHarness {
    /// Sets what the next listings return.
    pub(crate) fn set(&self, sessions: Vec<ListedSession>) {
        *self.sessions.lock().unwrap() = sessions;
    }

    /// Makes listings fail, or work again.
    pub(crate) fn fail(&self, failing: bool) {
        *self.failing.lock().unwrap() = failing;
    }

    /// Makes launches fail, or work again.
    pub(crate) fn refuse_launches(&self, refuse: bool) {
        *self.refuse_launches.lock().unwrap() = refuse;
    }

    /// Every launch so far.
    pub(crate) fn launches(&self) -> Vec<SpawnSpec> {
        self.launches.lock().unwrap().clone()
    }
}

pub(crate) fn unsupported(what: &str) -> HarnessError {
    HarnessError::CommandFailed {
        command: what.to_owned(),
        code: Some(1),
        message: "refused by the fake harness".to_owned(),
    }
}

impl Harness for FakeHarness {
    fn kind(&self) -> HarnessKind {
        HarnessKind::ClaudeCode
    }

    fn capabilities(&self) -> Result<Capabilities, HarnessError> {
        Ok(Capabilities {
            version: Version::new(2, 1, 319),
            list_sessions: true,
            background_sessions: true,
        })
    }

    fn list(&self) -> Result<Listing, HarnessError> {
        if *self.failing.lock().unwrap() {
            return Err(unsupported("agents"));
        }
        Ok(Listing {
            harness: HarnessKind::ClaudeCode,
            sessions: self.sessions.lock().unwrap().clone(),
            complete: true,
        })
    }

    fn spawn(&self, spec: &SpawnSpec) -> Result<Spawned, HarnessError> {
        if *self.refuse_launches.lock().unwrap() {
            return Err(unsupported("--bg"));
        }
        self.launches.lock().unwrap().push(spec.clone());
        Ok(Spawned {
            short_id: "abcd1234".to_owned(),
            name: Some(spec.name.clone()),
            origin: SessionOrigin::Started,
        })
    }

    fn resume(&self, _: &SessionKey, _: &SpawnSpec) -> Result<Spawned, HarnessError> {
        Err(unsupported("resume"))
    }

    fn stop(&self, _: &SessionKey) -> Result<(), HarnessError> {
        Err(unsupported("stop"))
    }

    fn remove(&self, _: &SessionKey) -> Result<(), HarnessError> {
        Err(unsupported("remove"))
    }

    fn logs(&self, _: &SessionKey) -> Result<Vec<u8>, HarnessError> {
        Err(unsupported("logs"))
    }

    fn attach_command(&self, _: &SessionKey) -> Result<Command, HarnessError> {
        Err(unsupported("attach"))
    }

    fn agent_settings(
        &self,
        volee_bin: &Path,
        agent: AgentId,
        _: Duration,
    ) -> Result<Value, HarnessError> {
        Ok(
            json!({ "hooks": { "Stop": format!("{} hook stop --agent {agent}", volee_bin.display()) } }),
        )
    }

    fn mcp_config(&self, volee_bin: &Path, agent: AgentId) -> Result<Value, HarnessError> {
        Ok(
            json!({ "mcpServers": { "volee": { "command": volee_bin, "args": ["mcp", "--agent", agent.to_string()] } } }),
        )
    }
}

/// Runs git in `dir` without the user's or the system's configuration.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A repository with one commit on `main`.
pub(crate) fn repository(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(dir.join("README.md"), "alpha\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "--quiet", "-m", "Initial commit"]);
}
