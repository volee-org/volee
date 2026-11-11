//! `volee run`: the program of a volee host that a client started becomes an
//! agent of the list (ADR 0029). A real daemon and real hosts, run by these
//! tests, with a fake Claude Code listing and a fake process table.
//!
//! The agent is told apart from every other kind: nothing hands it a message
//! or resumes it, a session listed inside its host is not a second agent,
//! and once its program ended its row leaves the list.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::FakeHarness;
use tokio::net::UnixListener;
use volee_core::event::EventKind;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::message::{MessageKind, Party};
use volee_core::reconcile::ListedSession;
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::workspace::Location;
use volee_core::{AgentId, Event, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_harness::{CommandRunner, Harness, Invocation, Output, RunError};
use volee_host::{HostConfig, HostDir, HostRecord, Size};
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    AgentParams, Attach, EventsSince, Open, OpenParams, Preview, Ran, Run, RunParams, SendMessage,
    SendParams, SinceParams, Spawn, SpawnParams, Stop,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

const SIZE: Size = Size { cols: 80, rows: 24 };

/// A program that draws one word and waits.
const WAITS: &str = "printf ready; exec cat";

/// The process table, as `ps` prints it; the directories of its processes,
/// as `lsof` prints them. The test writes the table.
#[derive(Debug, Default)]
struct Table {
    ps: Mutex<String>,
}

impl CommandRunner for Table {
    fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
        let program = invocation.program.to_string_lossy().into_owned();
        if program.ends_with("ps") {
            return Ok(Output::success(self.ps.lock().unwrap().clone()));
        }
        // Every process of the table runs in a directory named after it.
        let dirs: Vec<String> = self
            .ps
            .lock()
            .unwrap()
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(|pid| format!("p{pid}\nfcwd\nn/work/oc-{pid}\n"))
            .collect();
        Ok(Output::success(dirs.concat()))
    }
}

struct Running {
    socket: PathBuf,
    hosts: HostDir,
    harness: Arc<FakeHarness>,
    table: Arc<Table>,
    clock: Arc<ManualClock>,
    dir: tempfile::TempDir,
}

/// A daemon with hosts, a Claude Code listing and a process table, both
/// empty.
fn start() -> Running {
    // A short path: the hosts' sockets live under it.
    let dir = tempfile::Builder::new()
        .prefix("vo")
        .tempdir_in("/tmp")
        .unwrap();
    let paths = Paths::rooted(dir.path());
    let hosts = HostDir::new(paths.state_dir.join("hosts"));
    std::fs::create_dir_all(paths.state_dir.join("hosts")).unwrap();
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let table = Arc::new(Table::default());
    let clock = Arc::new(ManualClock::at(10_000));
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![])),
        clock: Arc::clone(&clock) as Arc<dyn volee_daemon::Clock>,
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(10),
        max_discovery_interval: Duration::from_millis(10),
        idle_discovery_interval: Duration::from_millis(10),
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: Vec::new(),
        hosts: true,
        processes: Some(Arc::clone(&table) as Arc<dyn CommandRunner>),
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        hosts,
        harness,
        table,
        clock,
        dir,
    }
}

impl Running {
    /// Starts a host as a client does: `command` in the directory `folder`,
    /// under the hosts directory of the daemon. Returns once it answers.
    fn host(&self, id: &str, folder: &str, command: &[&str], name: Option<&str>) -> HostRecord {
        let cwd = self.dir.path().join(folder);
        std::fs::create_dir_all(&cwd).unwrap();
        let config = HostConfig {
            id: id.to_owned(),
            dir: self.hosts.clone(),
            command: command.iter().map(|part| (*part).to_owned()).collect(),
            cwd,
            env: Vec::new(),
            env_remove: Vec::new(),
            size: SIZE,
            profile: "generic".into(),
            name: name.map(str::to_owned),
            scrollback: 100,
            linger: Duration::from_secs(30),
            idle_exit: None,
        };
        std::thread::spawn(move || volee_host::run(config));
        let socket = self.hosts.socket(id);
        let deadline = Instant::now() + Duration::from_secs(10);
        while volee_host::snapshot(&socket, "test", Duration::from_secs(3)).is_err() {
            assert!(Instant::now() < deadline, "the host {id} never answered");
            std::thread::sleep(Duration::from_millis(20));
        }
        HostRecord::read(&self.hosts.record(id)).unwrap()
    }

    fn folder(&self, folder: &str) -> PathBuf {
        self.dir.path().join(folder)
    }

    async fn client(&self) -> Client {
        Client::connect(&self.socket).await.unwrap()
    }

    async fn run(&self, host: &str, name: Option<&str>) -> Result<Ran, ClientError> {
        self.client()
            .await
            .call::<Run>(RunParams {
                host: host.to_owned(),
                name: name.map(str::to_owned),
            })
            .await
    }

    async fn events(&self) -> Vec<Event> {
        let batch = self
            .client()
            .await
            .call::<EventsSince>(SinceParams::default())
            .await
            .unwrap();
        batch.events.into_iter().map(|e| e.event).collect()
    }

    async fn registry(&self) -> Registry {
        Registry::from_events(&self.events().await).unwrap()
    }

    /// Waits until `ready` holds of the registry, for at most ten seconds.
    async fn until(&self, what: &str, ready: impl Fn(&Registry) -> bool) -> Registry {
        for _ in 0..1_000 {
            let registry = self.registry().await;
            if ready(&registry) {
                return registry;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the registry never got there: {what}");
    }
}

/// The refusal a call got: its code and its words.
fn refused<T: std::fmt::Debug>(result: Result<T, ClientError>) -> (i64, String) {
    match result {
        Err(ClientError::Rpc(refusal)) => (refusal.code, refusal.message),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn claude(id: &str, pid: u32) -> ListedSession {
    let mut session = ListedSession::new(SessionKey::claude(id), "/work/alpha", AgentState::Idle);
    session.alive = true;
    session.pid = Some(pid);
    session
}

fn opencode(pid: u32) -> SessionKey {
    SessionKey {
        harness: HarnessKind::Other("opencode".into()),
        id: format!("pid-{pid}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_program_of_a_host_becomes_an_agent_of_its_own_kind() {
    let daemon = start();
    daemon.host("r1", "billing", &["sh", "-c", WAITS], None);

    let ran = daemon.run("r1", None).await.unwrap();
    // Named after its directory when nothing names it.
    assert_eq!(ran.name, "billing");
    let registry = daemon.registry().await;
    let agent = registry.agent(ran.agent).unwrap().clone();
    assert_eq!(agent.role, Role::Independent);
    assert_eq!(agent.launched_by, LaunchedBy::Human);
    // Its harness is the program, by its name.
    assert_eq!(agent.harness, HarnessKind::Other("sh".into()));
    assert_eq!(agent.location, Location::Host { id: "r1".into() });
    assert_eq!(agent.workspace.cwd, daemon.folder("billing"));
    assert_eq!(agent.sessions.len(), 0, "no harness gave it a session");
    // An agent of its own kind: volee holds its terminal, and it is neither
    // one that joined through MCP nor a session volee only watches.
    assert!(agent.generic() && agent.managed());
    assert!(!agent.joined() && !agent.watched());

    // Running from its first event: never `launching`, the state in which a
    // session listed in the same directory would be taken for its own.
    let log = daemon.events().await;
    let own: Vec<&EventKind> =
        log.iter()
            .map(|event| &event.kind)
            .filter(|kind| match kind {
                EventKind::AgentRegistered { agent, .. }
                | EventKind::StateChanged { agent, .. } => *agent == ran.agent,
                _ => false,
            })
            .collect();
    assert!(
        matches!(own[0], EventKind::AgentRegistered { .. })
            && matches!(
                own[1],
                EventKind::StateChanged {
                    state: AgentState::Working,
                    ..
                }
            ),
        "{own:?}"
    );

    // Shown like any agent in a host: its screen, with its terminal's size.
    let client = daemon.client().await;
    let preview = client
        .call::<Preview>(AgentParams { agent: ran.agent })
        .await
        .unwrap();
    assert!(preview.screen.contains("ready"), "{:?}", preview.screen);
    assert!(preview.exact);
    // And opened there: its host is its terminal.
    let opened = client
        .call::<Open>(OpenParams {
            agent: ran.agent,
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();
    assert_eq!(opened.socket, Some(daemon.hosts.socket("r1")));
    assert!(!opened.view);

    // One host, one agent.
    let (code, message) = refused(daemon.run("r1", Some("again")).await);
    assert_eq!(code, rpc::REFUSED);
    assert!(message.contains("already runs an agent"), "{message}");
    assert_eq!(daemon.registry().await.agents().count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn its_name_is_the_one_asked_for_then_its_host_s_then_its_directory_s() {
    let daemon = start();
    daemon.host("n1", "alpha", &["sh", "-c", WAITS], Some("from-the-host"));
    daemon.host("n2", "beta", &["sh", "-c", WAITS], Some("from-the-host"));
    daemon.host("n3", "gamma", &["sh", "-c", WAITS], None);
    assert_eq!(daemon.run("n1", Some(" mine ")).await.unwrap().name, "mine");
    // A name of spaces is no name: the host's own, then its directory's.
    assert_eq!(
        daemon.run("n2", Some("  ")).await.unwrap().name,
        "from-the-host"
    );
    assert_eq!(daemon.run("n3", Some("  ")).await.unwrap().name, "gamma");
}

#[tokio::test(flavor = "multi_thread")]
async fn what_is_not_a_host_that_runs_a_program_is_refused() {
    let daemon = start();
    let (code, message) = refused(daemon.run("nope", None).await);
    assert_eq!(code, rpc::NOT_FOUND);
    assert!(message.contains("no volee host nope"), "{message}");

    // An id names files in the hosts directory, and nothing else.
    for id in ["../etc", "a/b", ".hidden", ""] {
        let (code, message) = refused(daemon.run(id, None).await);
        assert_eq!(code, rpc::INVALID_PARAMS, "{id:?}: {message}");
    }

    // A program that already ended is not an agent.
    daemon.host("gone", "ended", &["sh", "-c", "printf bye"], None);
    let record = daemon.hosts.record("gone");
    let deadline = Instant::now() + Duration::from_secs(10);
    while HostRecord::read(&record).is_ok_and(|record| record.exit.is_none()) {
        assert!(Instant::now() < deadline, "the program never ended");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (code, message) = refused(daemon.run("gone", None).await);
    assert_eq!(code, rpc::REFUSED);
    assert!(message.contains("has ended"), "{message}");
    assert_eq!(daemon.registry().await.agents().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_harness_lists_inside_its_host_is_not_a_second_agent() {
    let daemon = start();
    let record = daemon.host("r1", "billing", &["sh", "-c", WAITS], None);
    let program = record.child_pid.unwrap();
    let ran = daemon.run("r1", None).await.unwrap();

    // The process table: the program the host started, a wrapper's child
    // below it that is an OpenCode agent, a helper below that one, and an
    // OpenCode agent started by hand elsewhere. The host's own command line
    // is not volee's: the name of the volee program says nothing here.
    *daemon.table.ps.lock().unwrap() = format!(
        "{program} {host} sh -c wrapper\n 7001 {program} opencode\n 7002 7001 node helper\n \
         8001 900 opencode\n {host} 1 some-volee-build host run --id r1 -- sh -c wrapper\n",
        host = record.host_pid,
    );
    // Claude Code lists three sessions: the program itself, a process below
    // it, and one started elsewhere.
    daemon.harness.set(vec![
        claude("11111111-1111-4111-8111-111111111111", program),
        claude("22222222-2222-4222-8222-222222222222", 7002),
        claude("33333333-3333-4333-8333-333333333333", 75_441),
    ]);

    let elsewhere = SessionKey::claude("33333333-3333-4333-8333-333333333333");
    daemon
        .until("the sessions started elsewhere are agents", |registry| {
            registry.agent_for_session(&elsewhere).is_some()
                && registry.agent_for_session(&opencode(8001)).is_some()
        })
        .await;
    // Thirty rounds later, still one agent for the host and one for each
    // session started elsewhere.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let registry = daemon.registry().await;
    let names: Vec<String> = registry
        .agents()
        .map(|agent| format!("{} ({})", agent.name, agent.harness))
        .collect();
    assert_eq!(registry.agents().count(), 3, "{names:?}");
    for inside in [
        SessionKey::claude("11111111-1111-4111-8111-111111111111"),
        SessionKey::claude("22222222-2222-4222-8222-222222222222"),
        opencode(7001),
    ] {
        assert!(
            registry.agent_for_session(&inside).is_none(),
            "{inside:?} runs in the host of the agent volee already has: {names:?}"
        );
    }
    // The agent of the host is still its own: no session was given to it.
    let agent = registry.agent(ran.agent).unwrap();
    assert!(agent.generic() && agent.sessions.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn once_its_program_ended_it_stops_says_so_and_leaves_the_list() {
    let daemon = start();
    // A program that ends when the test says so.
    let waits = "printf ready; while [ ! -e stop ]; do sleep 0.05; done";
    daemon.host("r1", "billing", &["sh", "-c", waits], None);
    let ran = daemon.run("r1", None).await.unwrap();

    std::fs::write(daemon.folder("billing").join("stop"), "").unwrap();
    let registry = daemon
        .until("its program ended", |registry| {
            registry
                .agent(ran.agent)
                .is_some_and(|agent| agent.state == AgentState::Stopped)
        })
        .await;
    let agent = registry.agent(ran.agent).unwrap();
    assert_eq!(agent.detail.as_deref(), Some("its program ended"));

    // Nothing resumes it, and the daemon says so instead of advising a
    // message.
    let client = daemon.client().await;
    let (code, message) = refused(
        client
            .call::<Attach>(AgentParams { agent: ran.agent })
            .await,
    );
    assert_eq!(code, rpc::REFUSED);
    assert!(
        message.contains("nothing resumes a program started with `volee run`"),
        "{message}"
    );
    let (code, message) = refused(
        client
            .call::<Open>(OpenParams {
                agent: ran.agent,
                cols: 80,
                rows: 24,
            })
            .await,
    );
    assert_eq!(code, rpc::REFUSED);
    assert!(message.contains("run it again"), "{message}");

    // Its end can be seen for a minute.
    daemon.clock.advance(59_000);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(daemon.registry().await.agent(ran.agent).is_some());
    // Then its row leaves the list: there is nothing left to do with it.
    daemon.clock.advance(2_000);
    daemon
        .until("it left the list", |registry| {
            registry.agent(ran.agent).is_none()
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_does_not_resume_it_even_when_its_program_is_one_volee_can_launch() {
    let daemon = start();
    // A program named like a harness volee has a profile for.
    let bin = daemon.folder("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join("opencode");
    std::fs::write(&program, "#!/bin/sh\nprintf ready\nexec cat\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    daemon.host("r1", "billing", &[program.to_str().unwrap()], None);
    let ran = daemon.run("r1", None).await.unwrap();
    let registry = daemon.registry().await;
    let agent = registry.agent(ran.agent).unwrap();
    assert_eq!(agent.harness, HarnessKind::Other("opencode".into()));
    assert!(agent.generic());

    // Stopped from volee, like any agent in a host.
    let client = daemon.client().await;
    client
        .call::<Stop>(AgentParams { agent: ran.agent })
        .await
        .unwrap();
    daemon
        .until("it stopped", |registry| {
            registry
                .agent(ran.agent)
                .is_some_and(|agent| agent.state == AgentState::Stopped)
        })
        .await;

    // A message from the human waits: nothing launches anything for it.
    client
        .call::<SendMessage>(SendParams {
            from: Party::Human,
            to: vec![Party::Agent(ran.agent)],
            kind: MessageKind::Note,
            body: "are you there?".into(),
            refs: Vec::new(),
            in_reply_to: None,
            thread: None,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let log = daemon.events().await;
    let registry = Registry::from_events(&log).unwrap();
    let agent = registry.agent(ran.agent).unwrap();
    assert_eq!(agent.state, AgentState::Stopped, "{:?}", agent.detail);
    assert_eq!(registry.pending_for(ran.agent).len(), 1);
    let relaunched = log.iter().any(|event| {
        matches!(
            &event.kind,
            EventKind::StateChanged { agent, state: AgentState::Launching, .. } if *agent == ran.agent
        )
    });
    assert!(
        !relaunched,
        "nothing resumes a program started with volee run"
    );
    assert!(daemon.harness.resumes().is_empty() && daemon.harness.launches().is_empty());
    // One host: the one the client started.
    assert_eq!(daemon.hosts.list().unwrap().len(), 1);
}

/// What tells a program started with `volee run` is that the human started
/// it, on its own, on a harness other than Claude Code. No launch may make
/// such an agent: it would be taken for one while its session is unknown.
#[tokio::test(flavor = "multi_thread")]
async fn no_launch_makes_an_agent_that_looks_like_a_run() {
    let daemon = start();
    let client = daemon.client().await;
    let launch = |role| SpawnParams {
        repo: daemon.folder("billing").display().to_string(),
        branch: "feat/x".into(),
        base: None,
        task: "Do it.".into(),
        role,
        parent: None,
        mission: None,
        model: None,
        name: None,
        harness: Some("opencode".into()),
    };
    for role in [Role::Independent, Role::Coordinator, Role::Subagent] {
        let (code, message) = refused(client.call::<Spawn>(launch(role)).await);
        assert_eq!(code, rpc::INVALID_PARAMS, "{role}: {message}");
        assert!(message.contains("leads and workers"), "{message}");
    }
    assert_eq!(daemon.registry().await.agents().count(), 0);
}

/// `volee run` gives its hosts ids made like agent ids: they name files in
/// the hosts directory like any other.
#[tokio::test(flavor = "multi_thread")]
async fn the_ids_the_command_gives_its_hosts_are_accepted() {
    let daemon = start();
    let id = AgentId::from_ulid(ulid::Ulid::from_parts(1_000, 1)).to_string();
    daemon.host(&id, "billing", &["sh", "-c", WAITS], None);
    let ran = daemon.run(&id, None).await.unwrap();
    let registry = daemon.registry().await;
    assert_eq!(
        registry.agent(ran.agent).unwrap().location,
        Location::Host { id }
    );
}
