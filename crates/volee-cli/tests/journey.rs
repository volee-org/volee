//! The base trips, with the real `volee` binary and a stand-in for Claude
//! Code: the interface starts its daemon, lists a background session,
//! opens it by Enter where it runs, types into it, and comes back
//! (ADR 0027); the wall shows the session's screen, and the mouse reaches a
//! session opened from it (ADR 0028); `volee run` makes any program an
//! agent, in the terminal it is typed in, then in the interface (ADR 0029).
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

/// What the stand-in's `attach` runs: a program that repeats what is typed.
const REPEATS: &str = "exec cat";

/// The same, for a program that asks for the mouse, as an agent's interface
/// does: it turns mouse reporting on (clicks, in the SGR format), then
/// shows every byte it receives, control characters spelled out.
const WANTS_THE_MOUSE: &str = "printf '\\033[?1000h\\033[?1006h'; stty raw -echo; exec cat -v";

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
    /// session, running, and `attach` opens it as the program `attached`.
    fn install_claude(&self, attached: &str) {
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
             attach) echo \"session $2 of the stand-in\"; {attached} ;;\n\
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
    /// The daemon the interface started runs on its own, and so do the
    /// hosts `volee run` started: a test that fails must not leave them
    /// behind.
    fn drop(&mut self) {
        for record in files_of(&self.path("state/hosts")) {
            let host = std::fs::read(&record)
                .ok()
                .and_then(|json| serde_json::from_slice::<serde_json::Value>(&json).ok())
                .and_then(|record| record["host_pid"].as_u64());
            if let Some(pid) = host {
                let _ = Command::new("kill").arg(pid.to_string()).output();
            }
        }
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

    /// Bytes as the person's terminal sends them, such as a mouse report.
    fn bytes(&self, bytes: &[u8]) {
        self.keys.input(bytes).unwrap();
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
    terminal(world, "tui", &[])
}

/// Starts `volee <args>` in a pseudo-terminal of `SIZE`, as typed in a
/// person's terminal in the directory `work`, and connects to it.
fn terminal(world: &World, id: &str, args: &[&str]) -> (thread::JoinHandle<()>, Terminal) {
    let dir = HostDir::new(world.path("terminal"));
    let mut command = vec![env!("CARGO_BIN_EXE_volee").to_owned()];
    command.extend(args.iter().map(|arg| (*arg).to_owned()));
    let config = HostConfig {
        id: id.to_owned(),
        dir: dir.clone(),
        command,
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
    let socket = dir.socket(id);
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
    world.install_claude(REPEATS);
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

/// The agents `volee ls --json` lists.
fn listed(world: &World) -> Vec<serde_json::Value> {
    let output = world.volee(&["ls", "--json"]);
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    listed
        .as_array()
        .or_else(|| listed["agents"].as_array())
        .cloned()
        .unwrap_or_default()
}

fn named(world: &World, name: &str) -> Option<serde_json::Value> {
    listed(world)
        .into_iter()
        .find(|agent| agent["name"] == name)
}

#[test]
fn a_program_run_under_volee_is_an_agent_here_and_in_the_interface() {
    let world = World::new();
    world.install_claude(REPEATS);

    // Typed in a terminal: the program starts under volee, in this
    // directory, and this terminal is on it.
    let program = "echo the program says hello; exec cat";
    let (shell, mut here) = terminal(
        &world,
        "shell",
        &["run", "--name", "ledger", "--", "sh", "-c", program],
    );
    // The program's screen takes the terminal over.
    here.sees("the program says hello");
    here.text("typed by hand");
    here.key("enter", false);
    assert!(
        wait(30, || here.screen().matches("typed by hand").count() >= 2),
        "the program never repeated what was typed:\n{}",
        here.screen()
    );

    // It is an agent of the list: in a volee host, started by the person,
    // on its own, known by its program.
    let ledger = named(&world, "ledger").expect("the program is listed");
    assert_eq!(ledger["location"]["kind"], "host", "{ledger}");
    assert_eq!(ledger["launched_by"]["by"], "human", "{ledger}");
    assert_eq!(ledger["role"], "independent", "{ledger}");
    assert_eq!(ledger["harness"]["other"], "sh", "{ledger}");
    assert_eq!(ledger["sessions"].as_array().map(Vec::len), Some(0));
    let cwd = ledger["workspace"]["cwd"].as_str().unwrap_or_default();
    assert!(
        std::fs::canonicalize(cwd).ok() == std::fs::canonicalize(world.path("work")).ok(),
        "it runs in the directory the command was typed in: {cwd}"
    );

    // The leave key gives the terminal back; the program keeps running.
    here.key("q", true);
    here.sees("back from ledger; it keeps running");
    assert!(
        wait(15, || shell.is_finished()),
        "volee run never came back:\n{}",
        here.screen()
    );
    assert!(
        wait(15, || named(&world, "ledger")
            .is_some_and(|agent| agent["state"]["state"] != "stopped")),
        "the program stopped with its terminal"
    );

    // From the interface it opens like any agent: Enter, and its screen is
    // the one left in the terminal.
    let (host, mut terminal) = interface(&world);
    terminal.sees("ledger");
    terminal.key("/", false);
    terminal.text("ledger");
    terminal.key("enter", false);
    terminal.sees("\u{258c}");
    terminal.key("enter", false);
    terminal.sees("your keys go to ledger");
    terminal.sees("typed by hand");
    terminal.text("typed in volee");
    terminal.key("enter", false);
    assert!(
        wait(30, || terminal.screen().matches("typed in volee").count()
            >= 2),
        "the program never repeated what was typed in the interface:\n{}",
        terminal.screen()
    );
    // The leave key takes the keys back: the screen of an agent in a host
    // stays, live, and `z` goes back to the list.
    terminal.key("q", true);
    terminal.sees("z back");
    assert!(
        !terminal.screen().contains("your keys go to ledger"),
        "the keys are still the program's:\n{}",
        terminal.screen()
    );
    terminal.key("z", false);
    terminal.sees("Agents");

    // Nothing hands it a message, and the command says so.
    let told = world.volee(&["msg", "send", "ledger", "are you there?"]);
    let told =
        String::from_utf8_lossy(&told.stdout).into_owned() + &String::from_utf8_lossy(&told.stderr);
    assert!(told.contains("started with `volee run`"), "{told}");

    // Stopped from volee like any agent in a host; then nothing resumes
    // it, and opening it says so.
    let stopped = world.volee(&["stop", "ledger"]);
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(
        wait(15, || named(&world, "ledger")
            .is_some_and(|agent| agent["state"]["state"] == "stopped")),
        "the program never stopped: {:?}",
        named(&world, "ledger")
    );
    let reopened = world.volee(&["attach", "ledger"]);
    assert!(!reopened.status.success());
    let said = String::from_utf8_lossy(&reopened.stderr);
    assert!(
        said.contains("nothing resumes a program started with `volee run`"),
        "{said}"
    );

    terminal.key("q", false);
    assert!(
        wait(15, || host.is_finished()),
        "the interface never quit:\n{}",
        terminal.screen()
    );
}

#[test]
fn without_a_terminal_volee_run_starts_the_program_and_says_how_to_open_it() {
    let world = World::new();
    world.install_claude(REPEATS);
    // No terminal on standard input or output, as from a script.
    let mut command = Command::new(env!("CARGO_BIN_EXE_volee"));
    command
        .args(["run", "--", "sh", "-c", "echo from a script; exec cat"])
        .current_dir(world.path("work"))
        .env_clear()
        .envs(world.env())
        .stdin(std::process::Stdio::null());
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Named after its directory, and the way in is said.
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        said.contains("work runs under volee: `volee attach work` opens it."),
        "{said}"
    );
    let work = named(&world, "work").expect("the program is listed");
    assert_eq!(work["location"]["kind"], "host", "{work}");
    // Its screen is read like any agent's.
    assert!(
        wait(15, || {
            let logs = world.volee(&["logs", "work"]);
            String::from_utf8_lossy(&logs.stdout).contains("from a script")
        }),
        "volee logs never showed the program's screen"
    );

    // A command that cannot start is no agent, and the reason is said.
    let mut command = Command::new(env!("CARGO_BIN_EXE_volee"));
    command
        .args(["run", "--", "volee-no-such-program"])
        .current_dir(world.path("work"))
        .env_clear()
        .envs(world.env())
        .stdin(std::process::Stdio::null());
    let output = command.output().unwrap();
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("cannot start volee-no-such-program") && said.contains("PATH"),
        "{said}"
    );
    assert!(
        files_of(&world.path("state/hosts")).iter().all(|file| file
            .to_string_lossy()
            .contains(work["location"]["id"].as_str().unwrap())),
        "the host that never ran left files: {:?}",
        files_of(&world.path("state/hosts"))
    );
    let hosted: Vec<String> = listed(&world)
        .iter()
        .filter(|agent| agent["location"]["kind"] == "host")
        .map(|agent| agent["name"].to_string())
        .collect();
    assert_eq!(hosted, ["\"work\""], "one program runs under volee");
}

#[test]
fn in_the_shell_of_an_agent_the_commands_that_speak_for_the_person_refuse() {
    let world = World::new();
    world.install_claude(REPEATS);
    let as_agent = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_volee"));
        command
            .args(args)
            .env_clear()
            .envs(world.env())
            .env("VOLEE_AGENT", "ag_01h0000000000000000000000a");
        command.output().unwrap()
    };
    for (args, only) in [
        (
            &["msg", "answer", "msg_abcdef", "ok merge"][..],
            "only the person answers a question asked to them",
        ),
        (
            &["msg", "send", "invoices", "merge it"][..],
            "only the person writes in their own name",
        ),
        (
            &["mission", "close", "invoicing"][..],
            "only the person closes a mission from the command line",
        ),
        (
            &["mission", "new", "Invoicing", "Add invoices", "--repo", "."][..],
            "only the person starts a mission from the command line (a lead calls start_mission",
        ),
        (
            &["spawn", ".", "feat/invoices", "Add invoices"][..],
            "only the person launches an agent from the command line (a lead calls spawn_agent",
        ),
        (&["lead"][..], "only the person starts a lead"),
    ] {
        let output = as_agent(args);
        let said = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}: {said}");
        assert!(
            said.contains("this shell belongs to the agent ag_01h0000000000000000000000a"),
            "{args:?}: {said}"
        );
        assert!(said.contains(only), "{args:?}: {said}");
    }
    // Refused before anything else: no daemon was reached, or started.
    assert!(
        !world.path("state/volee.sock").exists(),
        "a refused command started a daemon"
    );
    // What only reads is not the person's word: it works there too.
    let listed = as_agent(&["msg", "questions"]);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "No question waits for you.\n"
    );
    // And the person's own shell is not an agent's.
    let mine = world.volee(&["msg", "answer", "msg_abcdef", "ok merge"]);
    let said = String::from_utf8_lossy(&mine.stderr);
    assert!(!said.contains("this shell belongs"), "{said}");
    assert!(said.contains("no question waiting for you"), "{said}");
}

#[test]
fn the_wall_shows_a_session_and_the_mouse_reaches_it_once_opened() {
    let world = World::new();
    world.install_claude(WANTS_THE_MOUSE);
    let (host, mut terminal) = interface(&world);
    terminal.sees("invoices");

    // The wall: the session's screen, as its harness replays it, though
    // volee hosts nothing of it.
    terminal.key("5", false);
    terminal.sees("Wall");
    terminal.sees("1 agent runs");
    terminal.sees("what the session printed");

    // Enter opens the selected tile's session, as from the list.
    terminal.key("enter", false);
    terminal.sees("session 7d3c1f0a of the stand-in");
    terminal.sees("your keys go to invoices");

    // The program asked for the mouse: a click on its screen reaches it, on
    // its own cell (the screen starts two columns and one row inside the
    // interface), in the format it asked for. Press, then release.
    terminal.bytes(b"\x1b[<0;20;10M");
    terminal.bytes(b"\x1b[<0;20;10m");
    terminal.sees("^[[<0;18;9M");
    terminal.sees("^[[<0;18;9m");
    // A click on the frame around it is volee's: nothing more arrives.
    terminal.bytes(b"\x1b[<0;1;1M");
    terminal.bytes(b"\x1b[<0;1;1m");
    terminal.text("x");
    terminal.sees("9mx");

    // The leave key comes back to the wall the session was opened from.
    terminal.key("q", true);
    terminal.sees("Left invoices: it keeps running");
    terminal.sees("1 agent runs");

    // `q` leaves the wall, then the interface.
    terminal.key("q", false);
    terminal.sees("Agents");
    terminal.key("q", false);
    assert!(
        wait(15, || host.is_finished()),
        "the interface never quit:\n{}",
        terminal.screen()
    );
}
