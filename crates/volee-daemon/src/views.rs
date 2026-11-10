//! Views (ADR 0027): terminals volee opens on sessions that run elsewhere.
//!
//! An agent volee launched runs in a volee host, which is its terminal. Any
//! other session has a door of its own: Claude Code opens a background
//! session with `claude attach`, tuios shows one pane with `tuios
//! pane-view`. To show such a session inside the interface, the daemon runs
//! that command in a host of its own, a view, and gives the interface the
//! view's socket: the interface then draws it and types into it like any
//! host. Nothing is stopped, forked or resumed, and ending a view ends only
//! the view.
//!
//! A view lives in a thread of the daemon. It ends its program once nobody
//! has held its input for a while, and leaves no file behind.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use volee_core::AgentId;
use volee_core::harness::HarnessKind;
use volee_core::workspace::Location;
use volee_host::{HostConfig, HostDir, HostRecord, Size};
use volee_proto::methods::{OpenParams, Opened};
use volee_proto::paths::Paths;

use crate::error::DaemonError;
use crate::launch::blocking;
use crate::server::Shared;

/// How long a view keeps its program after the last client that held its
/// input left: long enough to leave and come back, short enough that the
/// session it shows is not held at the view's size for nothing.
const IDLE_EXIT: Duration = Duration::from_secs(15);

/// How long a view's last screen stays once its program ended, for the
/// client to read how it ended.
const LINGER: Duration = Duration::from_secs(2);

/// How long a new view has to answer.
const START_TIMEOUT: Duration = Duration::from_secs(5);

/// The smallest and largest panel a view starts at.
const MIN_SIZE: Size = Size { cols: 20, rows: 5 };
const MAX_SIZE: Size = Size {
    cols: 500,
    rows: 200,
};

/// The views open now, by agent.
#[derive(Debug, Default)]
pub(crate) struct Views {
    open: Mutex<HashMap<AgentId, View>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct View {
    /// The view's host id.
    id: String,
    /// What it runs: a view on another door is another view.
    command: Vec<String>,
}

/// What a view runs to show an agent's session.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Door {
    command: Vec<String>,
    cwd: PathBuf,
    env_remove: Vec<String>,
}

/// Where views live: `<state>/views`. Apart from the hosts of agents, so
/// that nothing takes a view for an agent's terminal.
fn views_dir(paths: &Paths) -> HostDir {
    HostDir::new(paths.state_dir.join("views"))
}

/// Removes what the views of an earlier daemon left when it was killed:
/// they lived in its threads, so none of them runs any more.
pub(crate) fn clear(paths: &Paths) {
    let _ = std::fs::remove_dir_all(views_dir(paths).path());
}

/// How long the daemon waits, when it stops, for the programs of its views
/// to end.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

impl Views {
    /// Ends every view, when the daemon stops, and removes their files.
    /// Their programs are doors, not agents: the sessions they showed keep
    /// running. Blocks for a moment.
    pub(crate) fn close_all(&self, paths: &Paths) {
        let dir = views_dir(paths);
        let open: Vec<View> = self
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .map(|(_, view)| view)
            .collect();
        for view in &open {
            let _ = volee_host::stop(&dir.socket(&view.id), "volee daemon");
        }
        let deadline = Instant::now() + CLOSE_TIMEOUT;
        while open.iter().any(|view| runs(&dir, &view.id)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        clear(paths);
    }
}

/// Whether the host `id` of `dir` still runs its program.
fn runs(dir: &HostDir, id: &str) -> bool {
    std::os::unix::net::UnixStream::connect(dir.socket(id)).is_ok()
        && HostRecord::read(&dir.record(id)).is_ok_and(|record| record.exit.is_none())
}

/// `agents.open`: a terminal on the agent, for a client that shows it
/// inside its own screen.
pub(crate) async fn open(shared: &Arc<Shared>, params: OpenParams) -> Result<Opened, DaemonError> {
    let agent = params.agent;
    let (name, location, harness, cwd) = {
        let state = shared.state.lock().await;
        let record = state
            .registry()
            .agent(agent)
            .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {agent}")))?;
        (
            record.name.clone(),
            record.location.clone(),
            record.harness.clone(),
            record.workspace.cwd.clone(),
        )
    };
    // An agent in a volee host: the host is its terminal.
    if let Some(host) = location.host() {
        if !crate::launch::host_runs(shared, host) {
            return Err(DaemonError::Refused(format!(
                "{name} is not running: send it a message to resume it in a new volee host"
            )));
        }
        return Ok(Opened {
            socket: Some(crate::launch::hosts_dir(shared).join(format!("{host}.sock"))),
            view: false,
        });
    }
    let Some(door) = door(shared, agent, &name, &location, &harness, &cwd).await? else {
        // A pane only its multiplexer can show: the client brings it
        // forward (`agents.attach`).
        return Ok(Opened {
            socket: None,
            view: false,
        });
    };
    let dir = views_dir(&shared.paths);
    let size = Size {
        cols: params.cols.clamp(MIN_SIZE.cols, MAX_SIZE.cols),
        rows: params.rows.clamp(MIN_SIZE.rows, MAX_SIZE.rows),
    };
    let id = {
        let shared = Arc::clone(shared);
        let dir = dir.clone();
        let name = name.clone();
        blocking(move || view(&shared.views, &dir, agent, &name, door, size)).await??
    };
    Ok(Opened {
        socket: Some(dir.socket(&id)),
        view: true,
    })
}

/// The door of an agent that runs in no volee host; `None` for a pane its
/// multiplexer cannot show alone.
async fn door(
    shared: &Arc<Shared>,
    agent: AgentId,
    name: &str,
    location: &Location,
    harness: &HarnessKind,
    cwd: &Path,
) -> Result<Option<Door>, DaemonError> {
    // A door runs where its session runs: a harness asked there for
    // anything more than the session (Claude Code's own agents view, one
    // key away in `claude attach`) is in a directory the person already
    // trusts it with, never in `/`. A directory that is gone leaves the
    // daemon's own.
    let cwd = if cwd.is_dir() {
        cwd.to_path_buf()
    } else {
        shared.paths.state_dir.clone()
    };
    // In a multiplexer pane: the multiplexer shows it, when it can.
    if let Location::Pane {
        multiplexer,
        target,
    } = location
    {
        let Some(adapter) = shared.panes.adapter(multiplexer) else {
            return Err(crate::control::in_a_terminal(name, harness));
        };
        return Ok(adapter.view_command(target).map(|command| Door {
            command,
            cwd,
            env_remove: Vec::new(),
        }));
    }
    // Anywhere else, the harness opens the session itself: for Claude Code,
    // `claude attach` on a background session, running or not.
    let command = crate::control::attach_command(shared, agent).await?;
    let text = |value: &std::ffi::OsStr| value.to_string_lossy().into_owned();
    Ok(Some(Door {
        command: std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(text)
            .collect(),
        cwd: command.get_current_dir().map_or(cwd, Path::to_path_buf),
        env_remove: command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| text(key))
            .collect(),
    }))
}

/// The id of the view that shows `agent` through `door`: the one that runs,
/// or a new one. Blocks until a new view answers.
fn view(
    views: &Views,
    dir: &HostDir,
    agent: AgentId,
    name: &str,
    door: Door,
    size: Size,
) -> Result<String, DaemonError> {
    // One at a time: two clients opening the same agent share its view.
    let mut open = views.open.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(known) = open.get(&agent)
        && known.command == door.command
        && runs(dir, &known.id)
    {
        return Ok(known.id.clone());
    }
    // Views that ended are forgotten, whoever they showed.
    open.retain(|_, view| runs(dir, &view.id));
    dir.ensure().map_err(|error| {
        DaemonError::Unavailable(format!("cannot create {}: {error}", dir.path().display()))
    })?;
    let suffix = ulid::Ulid::generate().to_string().to_lowercase();
    let id = format!("view-{}", &suffix[suffix.len() - 10..]);
    let command = door.command.clone();
    let config = HostConfig {
        id: id.clone(),
        dir: dir.clone(),
        command: door.command,
        cwd: door.cwd,
        env: Vec::new(),
        env_remove: door.env_remove,
        size,
        profile: "generic".to_owned(),
        name: Some(name.to_owned()),
        scrollback: 1_000,
        linger: LINGER,
        idle_exit: Some(IDLE_EXIT),
    };
    start(dir, config)
        .map_err(|reason| DaemonError::Unavailable(format!("cannot open {name}: {reason}")))?;
    open.insert(
        agent,
        View {
            id: id.clone(),
            command,
        },
    );
    Ok(id)
}

/// Runs a view in a thread of its own and waits until it answers. The
/// thread removes what the view leaves once it has ended.
fn start(dir: &HostDir, config: HostConfig) -> Result<(), String> {
    let id = config.id.clone();
    let (ended, early) = mpsc::channel();
    let files = [dir.record(&id), dir.log(&id), dir.socket(&id)];
    std::thread::Builder::new()
        .name(format!("volee-{id}"))
        .spawn(move || {
            let result = volee_host::run(config);
            for file in &files {
                let _ = std::fs::remove_file(file);
            }
            let _ = ended.send(result.map(|_| ()).map_err(|error| error.to_string()));
        })
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        // The record is written once the program has started: a socket
        // alone may be the moment before a start that fails. A program
        // that already ended counts: the view keeps its last screen for a
        // moment, which says why (a session removed meanwhile).
        if std::os::unix::net::UnixStream::connect(dir.socket(&id)).is_ok()
            && HostRecord::read(&dir.record(&id)).is_ok()
        {
            return Ok(());
        }
        match early.try_recv() {
            Ok(Ok(())) => return Err("its program ended at once".to_owned()),
            Ok(Err(reason)) => return Err(reason),
            Err(TryRecvError::Disconnected) => return Err("the view stopped".to_owned()),
            Err(TryRecvError::Empty) => {}
        }
        if Instant::now() > deadline {
            return Err("the view did not answer in time".to_owned());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn door(command: &[&str], cwd: &Path) -> Door {
        Door {
            command: command.iter().map(|part| (*part).to_owned()).collect(),
            cwd: cwd.to_path_buf(),
            env_remove: Vec::new(),
        }
    }

    const SIZE: Size = Size { cols: 60, rows: 12 };

    fn agent(n: u128) -> AgentId {
        AgentId::from_ulid(ulid::Ulid::from_parts(1_000, n))
    }

    /// Ends a view's program, as the interface never does: tests do not wait
    /// for the idle exit.
    fn stop(dir: &HostDir, id: &str) {
        let _ = volee_host::stop(&dir.socket(id), "test");
        let deadline = Instant::now() + Duration::from_secs(10);
        while dir.record(id).exists() {
            assert!(Instant::now() < deadline, "the view never ended");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn an_agent_keeps_one_view_while_it_runs() {
        let root = tempfile::tempdir().unwrap();
        let dir = HostDir::new(root.path().join("views"));
        let views = Views::default();
        let cat = door(&["cat"], root.path());
        let first = view(&views, &dir, agent(1), "alpha", cat.clone(), SIZE).unwrap();
        assert!(first.starts_with("view-"), "{first}");
        assert!(runs(&dir, &first));
        // Opened again: the same view.
        let again = view(&views, &dir, agent(1), "alpha", cat.clone(), SIZE).unwrap();
        assert_eq!(again, first);
        // Another agent, or another door of the same agent: another view.
        let other = view(&views, &dir, agent(2), "beta", cat, SIZE).unwrap();
        assert_ne!(other, first);
        let moved = door(&["sh", "-c", "cat"], root.path());
        let elsewhere = view(&views, &dir, agent(1), "alpha", moved, SIZE).unwrap();
        assert_ne!(elsewhere, first);
        for id in [&first, &other, &elsewhere] {
            stop(&dir, id);
        }
    }

    #[test]
    fn a_view_that_ended_leaves_nothing_and_is_opened_anew() {
        let root = tempfile::tempdir().unwrap();
        let dir = HostDir::new(root.path().join("views"));
        let views = Views::default();
        let cat = door(&["cat"], root.path());
        let first = view(&views, &dir, agent(1), "alpha", cat.clone(), SIZE).unwrap();
        stop(&dir, &first);
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(left, Vec::<std::ffi::OsString>::new(), "nothing stays");
        let second = view(&views, &dir, agent(1), "alpha", cat, SIZE).unwrap();
        assert_ne!(second, first);
        stop(&dir, &second);
    }

    #[test]
    fn a_door_that_closes_at_once_still_shows_what_it_said() {
        let root = tempfile::tempdir().unwrap();
        let dir = HostDir::new(root.path().join("views"));
        let views = Views::default();
        let refused = door(
            &["sh", "-c", "echo 'No session found with that id'; exit 1"],
            root.path(),
        );
        let id = view(&views, &dir, agent(1), "alpha", refused.clone(), SIZE).unwrap();
        // What the door printed stays on the view's screen.
        let said = || {
            volee_host::snapshot(&dir.socket(&id), "test", Duration::from_secs(3)).is_ok_and(
                |(_, screen)| String::from_utf8_lossy(&screen).contains("No session found"),
            )
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !said() {
            assert!(Instant::now() < deadline, "the door's words never showed");
            std::thread::sleep(Duration::from_millis(20));
        }
        // The host records how the program ended a moment after its last
        // output.
        let ended = || {
            HostRecord::read(&dir.record(&id))
                .ok()
                .and_then(|record| record.exit)
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while ended().is_none() {
            assert!(Instant::now() < deadline, "the end was never recorded");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(ended().unwrap().code, Some(1));
        // It is not kept for the next time: opening again starts anew.
        let again = view(&views, &dir, agent(1), "alpha", refused, SIZE).unwrap();
        assert_ne!(again, id);
        let deadline = Instant::now() + Duration::from_secs(10);
        while dir.record(&id).exists() || dir.record(&again).exists() {
            assert!(Instant::now() < deadline, "the views never ended");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_door_that_cannot_open_says_why() {
        let root = tempfile::tempdir().unwrap();
        let dir = HostDir::new(root.path().join("views"));
        let views = Views::default();
        let missing = door(&["/no/such/program"], root.path());
        let error = view(&views, &dir, agent(1), "alpha", missing, SIZE).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("cannot open alpha"), "{text}");
        assert!(
            views.open.lock().unwrap().is_empty(),
            "a view that never ran is not kept"
        );
    }
}
