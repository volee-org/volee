//! The daemon-backed source against a real daemon on a temporary socket.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use ulid::Ulid;
use volee_core::harness::HarnessKind;
use volee_core::message::{MessageKind, Party};
use volee_core::mission::MissionKind;
use volee_core::role::{LaunchedBy, Role};
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, Registry};
use volee_daemon::{Config, Daemon, DaemonError, EvidenceSources, ManualClock, MemoryLog};
use volee_proto::client::Client;
use volee_proto::methods::{Empty, SendMessage, SendParams, Shutdown};
use volee_proto::paths::Paths;
use volee_tui::{
    DaemonSource, MissionRequest, Outgoing, Source, SourceError, SpawnRequest, Subscription,
};

fn agent(n: u128) -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, n))
}

fn registered(id: AgentId, name: &str, parent: Option<AgentId>) -> Event {
    Event::new(
        Timestamp::from_millis(1),
        EventKind::AgentRegistered {
            agent: id,
            name: name.to_owned(),
            role: if parent.is_some() {
                Role::Worker
            } else {
                Role::Lead
            },
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent,
            mission: None,
            workspace: Workspace::at("/work"),
            location: Location::Background,
        },
    )
}

struct Running {
    socket: PathBuf,
    task: JoinHandle<Result<(), DaemonError>>,
    _dir: tempfile::TempDir,
}

/// A daemon with a lead (1) and its worker "api" (2), without Claude Code
/// or a version control program.
fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![
            registered(agent(1), "lead", None),
            registered(agent(2), "api", Some(agent(1))),
        ])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: None,
        discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        max_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        idle_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        evidence: EvidenceSources::default(),
        volee_bin: None,
    };
    let task = tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        task,
        _dir: dir,
    }
}

async fn source(daemon: &Running) -> DaemonSource {
    let client = Client::connect(&daemon.socket).await.unwrap();
    DaemonSource::connect(client).await.unwrap()
}

/// The next update, within a few seconds.
async fn next(
    updates: &mut mpsc::Receiver<Result<Event, SourceError>>,
) -> Result<Event, SourceError> {
    tokio::time::timeout(Duration::from_secs(5), updates.recv())
        .await
        .expect("no update within five seconds")
        .expect("the updates stopped")
}

#[tokio::test]
async fn the_snapshot_and_the_updates_build_the_daemons_registry() {
    let daemon = start();
    let source = source(&daemon).await;
    let Subscription {
        snapshot,
        mut updates,
        claude_code_available,
    } = source.subscribe().await.unwrap();
    assert!(
        !claude_code_available,
        "the daemon's status says it found no Claude Code"
    );
    let mut registry = Registry::from_events(&snapshot).unwrap();
    assert_eq!(registry.agents().count(), 2);
    assert!(
        matches!(source.subscribe().await, Err(SourceError::Failed(_))),
        "one subscription per source"
    );

    source
        .send_message(Outgoing {
            to: agent(2),
            kind: MessageKind::Question,
            body: "Which port does the API listen on?".to_owned(),
            in_reply_to: None,
            thread: None,
        })
        .await
        .unwrap();
    // The message comes back as events, after the thread it opened.
    loop {
        let event = next(&mut updates).await.unwrap();
        registry.apply(&event).unwrap();
        if matches!(event.kind, EventKind::MessagePosted { .. }) {
            break;
        }
    }
    let thread = registry
        .thread_between(Party::Human, Party::Agent(agent(2)))
        .unwrap();
    let messages = registry.thread_messages(thread.id);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].body, "Which port does the API listen on?");
    assert_eq!(messages[0].kind, MessageKind::Question);
}

#[tokio::test]
async fn the_person_reads_and_a_daemon_without_claude_code_says_what_it_cannot_do() {
    let daemon = start();
    let source = source(&daemon).await;
    let Subscription {
        snapshot,
        mut updates,
        ..
    } = source.subscribe().await.unwrap();
    let mut registry = Registry::from_events(&snapshot).unwrap();
    // The worker asks the person something.
    let agent_side = Client::connect(&daemon.socket).await.unwrap();
    let sent = agent_side
        .call::<SendMessage>(SendParams {
            from: Party::Agent(agent(2)),
            to: vec![Party::Human],
            kind: MessageKind::Question,
            body: "May I drop the old endpoint?".to_owned(),
            refs: vec![],
            in_reply_to: None,
            thread: None,
        })
        .await
        .unwrap();
    while registry.message(sent.message).is_none() {
        registry.apply(&next(&mut updates).await.unwrap()).unwrap();
    }
    source.mark_read(sent.message).await.unwrap();
    loop {
        let event = next(&mut updates).await.unwrap();
        registry.apply(&event).unwrap();
        if matches!(event.kind, EventKind::MessageRead { .. }) {
            break;
        }
    }
    let receipt = registry
        .message(sent.message)
        .unwrap()
        .receipt(Party::Human);
    assert!(receipt.unwrap().read_at.is_some());

    // Summaries are off unless the person turns them on: the daemon says
    // which setting to change, and the interface shows it as is.
    match source.summarize(sent.thread).await {
        Err(SourceError::Unsupported(message)) => {
            assert!(message.contains("summaries = true"), "{message}");
        }
        other => panic!("summaries should be off: {other:?}"),
    }
    let no_claude = "Claude Code is not available to this daemon; see `volee daemon status`";
    assert_eq!(
        source
            .spawn(SpawnRequest {
                repo: "/work".to_owned(),
                branch: "feat/export".to_owned(),
                task: "Export invoices.".to_owned(),
                model: None,
            })
            .await,
        Err(SourceError::Failed(no_claude.to_owned()))
    );
    // A mission is created even without Claude Code; its coordinator waits.
    let created = source
        .create_mission(MissionRequest {
            title: "Invoice export".to_owned(),
            kind: MissionKind::Change,
            repos: vec!["billing".to_owned()],
            goal: "Export invoices as CSV.".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(created.coordinator, None);
    while registry.mission(created.mission).is_none() {
        registry.apply(&next(&mut updates).await.unwrap()).unwrap();
    }
}

#[tokio::test]
async fn what_the_daemon_cannot_do_is_said_plainly() {
    let daemon = start();
    let source = source(&daemon).await;
    // This daemon has no Claude Code to show screens or attach sessions
    // with, and nothing to read changes with: the interface gets the
    // daemon's sentence to show, and may ask again later.
    let no_claude = "Claude Code is not available to this daemon; see `volee daemon status`";
    assert_eq!(
        source.preview(agent(2)).await,
        Err(SourceError::Failed(no_claude.to_owned()))
    );
    assert_eq!(
        source.attach(agent(2)).await,
        Err(SourceError::Failed(no_claude.to_owned()))
    );
    assert!(matches!(
        source.diff(agent(2), false).await,
        Err(SourceError::Failed(message)) if message.ends_with("is not available to this daemon")
    ));
    let unknown = AgentId::from_ulid(Ulid::from_parts(1_000, 99));
    assert!(
        source
            .send_message(Outgoing {
                to: unknown,
                kind: MessageKind::Note,
                body: "hello?".to_owned(),
                in_reply_to: None,
                thread: None,
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_updates_say_when_the_daemon_goes_away() {
    let daemon = start();
    let source = source(&daemon).await;
    let Subscription { mut updates, .. } = source.subscribe().await.unwrap();
    let other = Client::connect(&daemon.socket).await.unwrap();
    other.call::<Shutdown>(Empty {}).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), daemon.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        next(&mut updates).await,
        Err(SourceError::Disconnected(
            "lost the connection to the volee daemon".to_owned()
        ))
    );
    assert!(updates.recv().await.is_none(), "then the channel closes");
    assert!(matches!(
        source.preview(agent(2)).await,
        Err(SourceError::Disconnected(_))
    ));
}
