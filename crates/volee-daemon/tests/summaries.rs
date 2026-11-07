//! Thread summaries end to end: off by default, a model's answer when the
//! person turned them on, and never an agent in the listing.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::FakeHarness;
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::message::{MessageKind, Party};
use volee_core::reconcile::ListedSession;
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_harness::Harness;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    EventsSince, SendMessage, SendParams, SinceParams, Summarize, SummarizeParams,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

fn agent() -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, 1))
}

struct Running {
    socket: PathBuf,
    paths: Paths,
    harness: Arc<FakeHarness>,
    _dir: tempfile::TempDir,
}

fn start(summaries: bool) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path().canonicalize().unwrap());
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let settings = volee_config::Config::parse(
        &format!("[messages]\nsummaries = {summaries}\n"),
        Path::new("config.toml"),
        Path::new("/home/nobody"),
    )
    .unwrap();
    let registered = Event::new(
        Timestamp::from_millis(1),
        EventKind::AgentRegistered {
            agent: agent(),
            name: "billing".into(),
            role: Role::Lead,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent: None,
            mission: None,
            workspace: Workspace::at("/work/billing"),
            location: Location::Background,
        },
    );
    let config = Config {
        paths: paths.clone(),
        log: Box::new(MemoryLog::with(vec![registered])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(20),
        evidence: EvidenceSources {
            settings,
            ..EvidenceSources::default()
        },
        volee_bin: None,
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        paths,
        harness,
        _dir: dir,
    }
}

/// A short exchange between the human and the agent; returns its thread.
async fn exchange(client: &Client) -> volee_core::ThreadId {
    let mut thread = None;
    for (from, to, kind, body) in [
        (
            Party::Human,
            Party::Agent(agent()),
            MessageKind::Note,
            "Use cents, not floats.",
        ),
        (
            Party::Agent(agent()),
            Party::Human,
            MessageKind::Question,
            "Should refunds be negative invoices?",
        ),
    ] {
        let sent = client
            .call::<SendMessage>(SendParams {
                from,
                to: vec![to],
                kind,
                body: body.into(),
                refs: vec![],
                in_reply_to: None,
                thread,
            })
            .await
            .unwrap();
        thread = Some(sent.thread);
    }
    thread.unwrap()
}

#[tokio::test]
async fn summaries_are_off_until_the_person_turns_them_on() {
    let daemon = start(false);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let thread = exchange(&client).await;
    let error = client
        .call::<Summarize>(SummarizeParams { thread })
        .await
        .unwrap_err();
    let ClientError::Rpc(error) = error else {
        panic!("expected a refusal, got {error:?}");
    };
    assert_eq!(error.code, rpc::REFUSED);
    assert!(
        error.message.contains("summaries = true"),
        "{}",
        error.message
    );
    assert_eq!(daemon.harness.completions().len(), 0, "nothing ran");
}

#[tokio::test]
async fn a_summary_reads_the_thread_from_volee_own_directory() {
    let daemon = start(true);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let thread = exchange(&client).await;
    let summary = client
        .call::<Summarize>(SummarizeParams { thread })
        .await
        .unwrap();
    assert!(summary.summary.contains("invoice model is merged"));
    assert_eq!(summary.messages, 2);
    assert_eq!(summary.model, "haiku");

    let request = daemon.harness.completions().pop().unwrap();
    assert!(
        request.cwd.starts_with(&daemon.paths.state_dir),
        "{}",
        request.cwd.display()
    );
    assert!(request.prompt.contains("Use cents, not floats."));
    assert!(request.prompt.contains("billing to the human (question)"));
    assert!(request.system_prompt.contains("Do not invent anything"));
}

#[tokio::test]
async fn volee_own_runs_never_become_agents() {
    let daemon = start(true);
    let mut run = ListedSession::new(
        SessionKey::claude("summary-run"),
        daemon.paths.state_dir.join("summaries"),
        AgentState::Working,
    );
    run.alive = true;
    daemon.harness.set(vec![run]);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let client = Client::connect(&daemon.socket).await.unwrap();
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    let registry = Registry::from_events(&events).unwrap();
    assert_eq!(
        registry.agents().count(),
        1,
        "only the agent the test registered"
    );
}
