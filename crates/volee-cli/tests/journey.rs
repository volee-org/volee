//! The base trip, with the real `volee` binary and a stand-in for Claude
//! Code: the interface starts its daemon, lists a background session,
//! opens it by Enter where it runs, types into it, and comes back
//! (ADR 0027).
//!
//! The interface runs in a pseudo-terminal, driven by keys and read as a
//! screen, like a person's terminal. The stand-in is a shell script named
//! `claude`: nothing here reaches a real Claude Code, the network or the
//! person's files. The journeys in `journeys/` make the same trip with the
//! real thing, outside CI.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]
#![allow(missing_docs)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use volee_host::{Connection, HostConfig, HostDir, HostSender, KeyPress, Size, Watch};

const SIZE: Size = Size {
    cols: 120,
    rows: 40,
};

/// What a Claude Code session leaves in the environment of its children: a
/// test run from inside one must not pass them on.
const SESSION_VARIABLES: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
];

/// One run: a directory that holds everything, and the environment that
/// keeps the binary inside it.
struct World {
    root: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        // Short: Unix socket paths are limited.
        let root = tempfile::Builder::new()
            .prefix("vc")
            .tempdir_in("/tmp")
            .unwrap();
        for dir in ["bin", "home", "work", "config"] {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    /// The environment of the binary: its own directories, and a `PATH`
    /// that holds the stand-in and the system's tools, nothing of the
    /// person's.
    fn env(&self) -> Vec<(String, String)> {
        let text = |relative: &str| self.path(relative).display().to_string();
        vec![
            ("PATH".into(), format!("{}:/usr/bin:/bin", text("bin"))),
            ("HOME".into(), text("home")),
            ("VOLEE_STATE_DIR".into(), text("state")),
            ("VOLEE_CONFIG_DIR".into(), text("config")),
            ("VOLEE_DATA_DIR".into(), text("data")),
            ("VOLEE_SOCKET".into(), text("state/volee.sock")),
            ("TERM".into(), "xterm-256color".into()),
        ]
    }

    /// Installs the stand-in for Claude Code: it lists one background
    /// session, running, and `attach` opens it as a program that repeats
    /// what is typed.
    fn install_claude(&self) {
        let listing = serde_json::json!([{
            "pid": std::process::id(),
            "id": "7d3c1f0a",
            "cwd": self.path("work"),
            "kind": "background",
            "startedAt": 1_790_000_000_000_i64,
            "sessionId": "7d3c1f0a-2b4e-4c6d-9e8f-0a1b2c3d4e5f",
            "name": "invoices",
            "status": "idle",
            "state": "done",
        }]);
        std::fs::write(self.path("listing.json"), listing.to_string()).unwrap();
        let script = format!(
            "#!/bin/sh\n\
             case \"$1\" in\n\
             --version) echo '2.1.322 (Claude Code)' ;;\n\
             agents) cat '{listing}' ;;\n\
             attach) echo \"session $2 of the stand-in\"; exec cat ;;\n\
             logs) printf 'what the session printed\\r\\n' ;;\n\
             *) echo \"unexpected: $*\" >&2; exit 1 ;;\n\
             esac\n",
            listing = self.path("listing.json").display()
        );
        let program = self.path("bin/claude");
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Runs `volee` with `args` to its end, in this world.
    fn volee(&self, args: &[&str]) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_volee"));
        command.args(args).env_clear().envs(self.env());
        command.output().unwrap()
    }
}

impl Drop for World {
    /// The daemon the interface started runs on its own: a test that fails
    /// must not leave it behind.
    fn drop(&mut self) {
        let _ = self.volee(&["daemon", "stop"]);
    }
}

/// Waits until `ready` holds, for at most `seconds`.
fn wait(seconds: u64, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

/// The interface in a pseudo-terminal: its screen, and its keyboard.
struct Terminal {
    watch: Watch,
    keys: HostSender,
    _owner: Connection,
}

impl Terminal {
    fn screen(&mut self) -> String {
        self.watch.pump();
        self.watch.text().unwrap_or_default()
    }

    /// Whether `text` shows on the screen within half a minute.
    #[track_caller]
    fn sees(&mut self, text: &str) {
        let shown = wait(30, || self.screen().contains(text));
        assert!(shown, "never saw {text:?} on:\n{}", self.screen());
    }

    fn key(&self, key: &str, ctrl: bool) {
        let press = KeyPress {
            key: key.to_owned(),
            shift: false,
            alt: false,
            ctrl,
        };
        self.keys.key(&press).unwrap();
        thread::sleep(Duration::from_millis(60));
    }

    fn text(&self, text: &str) {
        for letter in text.chars() {
            let name = if letter == ' ' {
                "space".to_owned()
            } else {
                letter.to_string()
            };
            self.key(&name, false);
        }
    }
}

/// Starts the interface in a pseudo-terminal of `SIZE` and connects to it.
fn interface(world: &World) -> (thread::JoinHandle<()>, Terminal) {
    let dir = HostDir::new(world.path("terminal"));
    let config = HostConfig {
        id: "tui".to_owned(),
        dir: dir.clone(),
        command: vec![env!("CARGO_BIN_EXE_volee").to_owned()],
        cwd: world.path("work"),
        env: world.env(),
        env_remove: SESSION_VARIABLES
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
        size: SIZE,
        profile: "generic".to_owned(),
        name: None,
        scrollback: 100,
        linger: Duration::from_millis(200),
        idle_exit: None,
    };
    let host = thread::spawn(move || {
        volee_host::run(config).unwrap();
    });
    let socket = dir.socket("tui");
    assert!(
        wait(10, || std::os::unix::net::UnixStream::connect(&socket)
            .is_ok()),
        "the pseudo-terminal never answered"
    );
    let owner = Connection::open_embedded(&socket, SIZE, "journey").unwrap();
    let keys = owner.sender().unwrap();
    let watch = Watch::start(&socket, "journey").unwrap();
    (
        host,
        Terminal {
            watch,
            keys,
            _owner: owner,
        },
    )
}

fn views(world: &World) -> Vec<PathBuf> {
    files_of(&world.path("state/views"))
}

fn files_of(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

#[test]
fn a_background_session_is_listed_opened_typed_into_and_left() {
    let world = World::new();
    world.install_claude();
    let (host, mut terminal) = interface(&world);

    // The interface starts its daemon, which lists the stand-in's session.
    terminal.sees("invoices");
    terminal.key("/", false);
    terminal.text("invoices");
    terminal.key("enter", false);
    terminal.sees("\u{258c}");

    // Enter opens the session where it runs: the harness's own command, in
    // a terminal inside the interface. No prompt, no adoption.
    terminal.key("enter", false);
    terminal.sees("session 7d3c1f0a of the stand-in");
    terminal.sees("your keys go to invoices");
    assert!(
        !terminal.screen().contains("Move invoices"),
        "opening never asks to adopt:\n{}",
        terminal.screen()
    );
    assert_eq!(
        views(&world).len(),
        2,
        "one view: its socket and its record"
    );

    // What is typed goes to the session.
    terminal.text("hello volee");
    terminal.key("enter", false);
    terminal.sees("hello volee");

    // The leave key comes back to the list; the session is none the wiser.
    terminal.key("q", true);
    terminal.sees("Left invoices: it keeps running");
    terminal.sees("Agents");

    // Opened again while its view still runs: the same view.
    terminal.key("enter", false);
    terminal.sees("your keys go to invoices");
    terminal.sees("hello volee");
    assert_eq!(views(&world).len(), 2, "still one view");
    terminal.key("q", true);
    terminal.sees("Left invoices");

    // `volee ls` shows it as what it is: a background session someone else
    // started, still in the background.
    let listed = world.volee(&["ls", "--json"]);
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let agents = listed
        .as_array()
        .or_else(|| listed["agents"].as_array())
        .unwrap();
    let invoices = agents
        .iter()
        .find(|agent| agent["name"] == "invoices")
        .expect("the session is listed");
    assert_eq!(invoices["location"]["kind"], "background");
    assert_eq!(invoices["launched_by"]["by"], "outside");
    assert_eq!(invoices["sessions"].as_array().unwrap().len(), 1);

    // The interface quits; the daemon stops and takes its view with it.
    terminal.key("q", false);
    assert!(
        wait(15, || host.is_finished()),
        "the interface never quit:\n{}",
        terminal.screen()
    );
    let stopped = world.volee(&["daemon", "stop"]);
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(
        wait(15, || views(&world).is_empty()),
        "a view outlived its daemon: {:?}",
        views(&world)
    );
}
