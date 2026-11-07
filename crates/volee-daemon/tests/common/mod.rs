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
use volee_harness::{
    Capabilities, Harness, HarnessError, SpawnSpec, Spawned, Version, WorktreeRemoval,
};

/// A harness whose listing the test sets, and which records launches and
/// the actions on sessions (`logs`, `stop`, `attach`) with the handle each
/// one received.
#[derive(Debug, Default)]
pub(crate) struct FakeHarness {
    sessions: Mutex<Vec<ListedSession>>,
    failing: Mutex<bool>,
    launches: Mutex<Vec<SpawnSpec>>,
    resumes: Mutex<Vec<(SessionKey, SpawnSpec)>>,
    refuse_launches: Mutex<bool>,
    actions: Mutex<Vec<(String, String)>>,
    sessions_gone: Mutex<bool>,
}

impl FakeHarness {
    /// Every action on a session so far, as `(action, handle)`.
    pub(crate) fn actions(&self) -> Vec<(String, String)> {
        self.actions.lock().unwrap().clone()
    }

    /// Makes the harness forget every session, as `claude rm` would.
    pub(crate) fn forget_sessions(&self) {
        *self.sessions_gone.lock().unwrap() = true;
    }

    /// Records an action on `session` and returns its handle, or the error
    /// Claude Code gives for a session it does not know.
    fn act(&self, action: &str, session: &ListedSession) -> Result<String, HarnessError> {
        let handle = session
            .handle
            .clone()
            .ok_or_else(|| HarnessError::NotControllable {
                session: session.key.clone(),
            })?;
        if *self.sessions_gone.lock().unwrap() {
            return Err(HarnessError::UnknownSession { handle });
        }
        self.actions
            .lock()
            .unwrap()
            .push((action.to_owned(), handle.clone()));
        Ok(handle)
    }
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

    /// Every resume so far, with the session it resumed.
    pub(crate) fn resumes(&self) -> Vec<(SessionKey, SpawnSpec)> {
        self.resumes.lock().unwrap().clone()
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
            handle: "abcd1234".to_owned(),
            name: Some(spec.name.clone()),
            origin: SessionOrigin::Started,
        })
    }

    fn resume(&self, session: &SessionKey, spec: &SpawnSpec) -> Result<Spawned, HarnessError> {
        if *self.refuse_launches.lock().unwrap() {
            return Err(unsupported("--resume"));
        }
        let mut resumes = self.resumes.lock().unwrap();
        resumes.push((session.clone(), spec.clone()));
        Ok(Spawned {
            handle: format!("resumed{}", resumes.len()),
            name: Some(spec.name.clone()),
            origin: SessionOrigin::Forked,
        })
    }

    fn stop(&self, session: &ListedSession) -> Result<(), HarnessError> {
        self.act("stop", session).map(drop)
    }

    fn remove(&self, _: &ListedSession, _: WorktreeRemoval) -> Result<(), HarnessError> {
        Err(unsupported("remove"))
    }

    /// A screen with a color escape, like `claude logs` prints.
    fn logs(&self, session: &ListedSession) -> Result<Vec<u8>, HarnessError> {
        let handle = self.act("logs", session)?;
        Ok(format!("\u{1b}[2J\u{1b}[1;1H\u{1b}[32mscreen of {handle}\u{1b}[0m").into_bytes())
    }

    fn attach_command(&self, session: &ListedSession) -> Result<Command, HarnessError> {
        let handle = self.act("attach", session)?;
        let mut command = Command::new("claude");
        command.arg("attach").arg(handle).env_remove("CLAUDECODE");
        Ok(command)
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
