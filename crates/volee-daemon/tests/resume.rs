//! Resuming an agent whose session ended, to deliver its messages: a real
//! repository and worktree, a fake harness that records resumes and lists
//! sessions, and a real daemon.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{FakeHarness, repository};
use serde_json::json;
use tokio::net::UnixListener;
use volee_core::agent::Hold;
use volee_core::harness::{SessionKey, SessionOrigin};
use volee_core::message::{MessageKind, Party};
use volee_core::reconcile::ListedSession;
use volee_core::role::Role;
use volee_core::state::AgentState;
use volee_core::{AgentId, Event, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_git::GitCli;
use volee_harness::Harness;
use volee_proto::client::Client;
use volee_proto::methods::{
    AgentParams, EventsSince, HookIngest, HookParams, SendMessage, SendParams, SinceParams, Spawn,
    SpawnParams, Stop,
};
use volee_proto::paths::Paths;

struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    repo: PathBuf,
    _dir: tempfile::TempDir,
}

fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("alpha");
    repository(&repo);
    let paths = Paths::rooted(root.join("volee"));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let settings = volee_config::Config::parse(
        &format!("[repos.alpha]\npath = \"{}\"\n", repo.display()),
        Path::new("config.toml"),
        Path::new("/home/nobody"),
    )
    .unwrap();
    let git = GitCli::new()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(20),
        idle_discovery_interval: Duration::from_millis(20),
        evidence: EvidenceSources {
            git: Some(Arc::new(git)),
            settings,
            ..EvidenceSources::default()
        },
        volee_bin: Some(PathBuf::from("/opt/volee/bin/volee")),
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        harness,
        repo,
        _dir: dir,
    }
}

async fn registry(socket: &Path) -> Registry {
    let client = Client::connect(socket).await.unwrap();
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events).unwrap()
}

/// Waits until the registry satisfies `ready`, for at most ten seconds.
async fn until(socket: &Path, ready: impl Fn(&Registry) -> bool) -> Registry {
    for _ in 0..1_000 {
        let registry = registry(socket).await;
        if ready(&registry) {
            return registry;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the registry never reached the expected state");
}

fn listed(id: &str, handle: &str, cwd: &Path) -> ListedSession {
    let mut session = ListedSession::new(SessionKey::claude(id), cwd, AgentState::Working);
    session.alive = true;
    session.background = true;
    session.handle = Some(handle.to_owned());
    session
}

/// Launches a worker and lets its first session start.
async fn running_worker(daemon: &Running) -> AgentId {
    let client = Client::connect(&daemon.socket).await.unwrap();
    let spawned = client
        .call::<Spawn>(SpawnParams {
            repo: "alpha".into(),
            branch: "feat/invoices".into(),
            base: None,
            task: "Add invoices.".into(),
            role: Role::Lead,
            parent: None,
            mission: None,
            model: None,
            name: Some("invoices".into()),
        })
        .await
        .unwrap();
    hook(
        &client,
        spawned.agent,
        "session-start",
        json!({ "hook_event_name": "SessionStart", "session_id": "first-session", "source": "startup" }),
    )
    .await;
    spawned.agent
}

/// Launches a worker, lets its first session start, then end.
async fn stopped_worker(daemon: &Running) -> AgentId {
    let agent = running_worker(daemon).await;
    let client = Client::connect(&daemon.socket).await.unwrap();
    session_ends(&client, agent).await;
    agent
}

async fn session_ends(client: &Client, agent: AgentId) {
    hook(
        client,
        agent,
        "session-end",
        json!({ "hook_event_name": "SessionEnd", "session_id": "first-session", "reason": "other" }),
    )
    .await;
}

async fn hook(client: &Client, agent: AgentId, event: &str, payload: serde_json::Value) {
    client
        .call::<HookIngest>(HookParams {
            agent,
            event: event.into(),
            payload,
        })
        .await
        .unwrap();
}

fn note(to: AgentId, body: &str) -> SendParams {
    SendParams {
        from: Party::Human,
        to: vec![Party::Agent(to)],
        kind: MessageKind::Note,
        body: body.into(),
        refs: vec![],
        in_reply_to: None,
        thread: None,
    }
}

#[tokio::test]
async fn a_message_resumes_a_stopped_agent_in_its_worktree() {
    let daemon = start();
    let agent = stopped_worker(&daemon).await;
    assert_eq!(
        registry(&daemon.socket).await.agent(agent).unwrap().state,
        AgentState::Stopped
    );

    let client = Client::connect(&daemon.socket).await.unwrap();
    let sent = client
        .call::<SendMessage>(note(agent, "Also update the changelog."))
        .await
        .unwrap();
    // The resume marks the message delivered once the harness started it.
    let registry_now = until(&daemon.socket, |r| {
        r.message(sent.message)
            .and_then(|message| message.receipt(Party::Agent(agent)))
            .is_some_and(|receipt| receipt.delivered_at.is_some())
    })
    .await;
    assert_eq!(
        registry_now.agent(agent).unwrap().state,
        AgentState::Launching
    );

    let resumes = daemon.harness.resumes();
    assert_eq!(resumes.len(), 1);
    let (session, spec) = &resumes[0];
    assert_eq!(session, &SessionKey::claude("first-session"));
    assert_eq!(spec.cwd, daemon.repo, "from the trusted main checkout");
    assert_eq!(spec.worktree.as_deref(), Some("feat-invoices"));
    assert!(
        spec.prompt.contains("Also update the changelog."),
        "{}",
        spec.prompt
    );
    assert!(
        spec.append_system_prompt
            .as_deref()
            .unwrap()
            .contains("worktree of alpha")
    );
    let message = registry_now.message(sent.message).unwrap();
    assert!(
        message
            .receipt(Party::Agent(agent))
            .unwrap()
            .delivered_at
            .is_some(),
        "the prompt delivered it"
    );

    // The resumed session is a fork: the listing shows it under a new id,
    // and its handle tells whose it is.
    let worktree = daemon.repo.join(".claude/worktrees/feat-invoices");
    daemon
        .harness
        .set(vec![listed("forked-session", "resumed1", &worktree)]);
    let linked = until(&daemon.socket, |r| {
        r.agent(agent)
            .unwrap()
            .owns_session(&SessionKey::claude("forked-session"))
    })
    .await;
    assert_eq!(linked.agents().count(), 1, "no stranger for the fork");
    let link = linked.agent(agent).unwrap().current_session().unwrap();
    assert_eq!(link.origin, SessionOrigin::Forked);
}

#[tokio::test]
async fn agents_volee_did_not_launch_are_never_resumed() {
    let daemon = start();
    let outside = daemon.repo.join("elsewhere");
    daemon
        .harness
        .set(vec![listed("outside-session", "out1", &outside)]);
    let registry_now = until(&daemon.socket, |r| r.agents().count() == 1).await;
    let stranger = registry_now.agents().next().unwrap().id;
    // The session ends: it disappears from a complete listing.
    daemon.harness.set(vec![]);
    until(&daemon.socket, |r| {
        r.agent(stranger).unwrap().state == AgentState::Lost
    })
    .await;
    let client = Client::connect(&daemon.socket).await.unwrap();
    client
        .call::<SendMessage>(note(stranger, "Are you there?"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(daemon.harness.resumes().len(), 0);
}

#[tokio::test]
async fn a_failed_resume_is_told_to_the_human_once() {
    let daemon = start();
    let agent = stopped_worker(&daemon).await;
    daemon.harness.refuse_launches(true);
    let client = Client::connect(&daemon.socket).await.unwrap();
    for body in ["First.", "Second."] {
        client.call::<SendMessage>(note(agent, body)).await.unwrap();
    }
    until(&daemon.socket, |r| {
        r.messages_from(Party::System)
            .iter()
            .any(|m| m.kind == MessageKind::Alert && m.body.contains("could not resume"))
    })
    .await;
    // The second message may try once more (a new message lifts the pause
    // between attempts); every failed attempt puts the agent back where it
    // was, and the human is told only once.
    let settled = until(&daemon.socket, |r| {
        r.agent(agent).unwrap().state == AgentState::Stopped
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let alerts = registry(&daemon.socket)
        .await
        .messages_from(Party::System)
        .into_iter()
        .filter(|m| m.body.contains("could not resume"))
        .count();
    assert_eq!(alerts, 1, "told once");
    assert_eq!(
        settled.pending_for(agent).len(),
        2,
        "both messages stay pending"
    );
}

#[tokio::test]
async fn a_stopped_agent_waits_for_the_human_to_write_to_it() {
    let daemon = start();
    let agent = stopped_worker(&daemon).await;
    let client = Client::connect(&daemon.socket).await.unwrap();
    // It is not running: the stop only holds it.
    client.call::<Stop>(AgentParams { agent }).await.unwrap();
    assert!(
        daemon
            .harness
            .actions()
            .iter()
            .all(|(action, _)| action != "stop")
    );
    let mut notice = note(agent, "A notice from volee.");
    notice.from = Party::System;
    client.call::<SendMessage>(notice).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(daemon.harness.resumes().len(), 0, "held");
    assert!(matches!(
        registry(&daemon.socket).await.agent(agent).unwrap().hold,
        Some(Hold::StoppedByHuman { .. })
    ));

    // The human writes to it: the hold lifts and every waiting message goes.
    let sent = client
        .call::<SendMessage>(note(agent, "Carry on."))
        .await
        .unwrap();
    let delivered = until(&daemon.socket, |r| {
        r.message(sent.message)
            .and_then(|message| message.receipt(Party::Agent(agent)))
            .is_some_and(|receipt| receipt.delivered_at.is_some())
    })
    .await;
    assert_eq!(delivered.agent(agent).unwrap().hold, None);
    let resumes = daemon.harness.resumes();
    assert_eq!(resumes.len(), 1);
    let prompt = &resumes[0].1.prompt;
    assert!(
        prompt.contains("A notice from volee.") && prompt.contains("Carry on."),
        "{prompt}"
    );
}

#[tokio::test]
async fn a_message_waiting_for_a_stopped_agent_does_not_restart_it() {
    let daemon = start();
    let agent = running_worker(&daemon).await;
    let worktree = daemon.repo.join(".claude/worktrees/feat-invoices");
    daemon
        .harness
        .set(vec![listed("first-session", "abcd1234", &worktree)]);
    until(&daemon.socket, |r| r.agent(agent).unwrap().state.is_live()).await;
    let client = Client::connect(&daemon.socket).await.unwrap();
    // Sent while it works: it waits for the end of the turn.
    client
        .call::<SendMessage>(note(agent, "Also update the changelog."))
        .await
        .unwrap();
    client.call::<Stop>(AgentParams { agent }).await.unwrap();
    assert!(
        daemon
            .harness
            .actions()
            .contains(&("stop".to_owned(), "abcd1234".to_owned()))
    );

    // The session ends; the message still waits, and nothing resumes it.
    daemon.harness.set(vec![]);
    session_ends(&client, agent).await;
    let ended = until(&daemon.socket, |r| !r.agent(agent).unwrap().state.is_live()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(daemon.harness.resumes().len(), 0);
    assert_eq!(ended.pending_for(agent).len(), 1);
}
