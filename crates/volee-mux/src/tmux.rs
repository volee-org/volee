//! [tmux](https://github.com/tmux/tmux), through its command line, as
//! measured with tmux 3.8.
//!
//! - **Server**: every command names its server: the socket that `TMUX`
//!   names in the daemon's environment (`-S`), as tmux itself would use, or
//!   else the default server (`-L default`).
//! - **Locate**: `list-panes -a` gives every pane with the process it runs.
//!   A session runs in the pane whose process is the session's own process
//!   or one of its ancestors, which `ps` gives.
//! - **Capture**: `capture-pane -p -e`: the pane's rows with their colors.
//! - **Focus**: `select-window` and `select-pane` make the pane current in
//!   its session, which works with no client attached; then `switch-client`
//!   shows it in the client volee runs in. That step runs only from a client
//!   of the same server: run from anywhere else, tmux would switch a client
//!   it picks itself, perhaps in another terminal. A shell compares the
//!   server's socket, which `list-panes` reports, with the one `TMUX` names.
//!
//! Only reads and focus: nothing is typed into a pane, and no pane is
//! created, closed or moved.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use volee_core::harness::SessionKey;
use volee_harness::{CommandRunner, Invocation, Output, RunError, SystemRunner};

use crate::{
    Hosted, Located, Multiplexer, MultiplexerStatus, MuxError, process, screen, split_target,
    target,
};

/// How long one command may take.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The program, looked up on the `PATH`.
const PROGRAM: &str = "tmux";

/// Runs the command in its arguments only from a client of the tmux server
/// whose socket is its first argument: `TMUX` starts with that socket.
const ONLY_FROM_THIS_SERVER: &str = "socket=$1; shift; case \"$TMUX\" in \"$socket\",*) exec \"$@\" ;; esac; \
     echo \"this terminal is not a client of the tmux server that runs the pane\" >&2; exit 1";

/// tmux, run by a [`CommandRunner`].
///
/// ```no_run
/// use volee_mux::{Multiplexer, Tmux};
///
/// let tmux = Tmux::new();
/// println!("{:?}", tmux.status());
/// ```
#[derive(Debug)]
pub struct Tmux<R = SystemRunner> {
    runner: R,
    program: PathBuf,
    /// The server's socket; `None` for the default server.
    socket: Option<PathBuf>,
    /// The server's socket as tmux reports it, once `list-panes` answered.
    reported_socket: Mutex<Option<String>>,
    /// Sessions found outside every pane, with the process they ran then:
    /// a process never moves into a pane, so they are not asked about again.
    outside: Mutex<HashSet<(SessionKey, u32)>>,
}

impl Tmux {
    /// tmux as `tmux` on the `PATH`, on the server `TMUX` names, as `tmux`
    /// itself reads it, or else on the default server.
    pub fn new() -> Self {
        let socket = std::env::var("TMUX")
            .ok()
            .and_then(|value| socket_of(&value));
        Self::with_runner(SystemRunner).on_socket(socket)
    }
}

impl Default for Tmux {
    fn default() -> Self {
        Self::new()
    }
}

/// The socket in a `TMUX` value: `<socket>,<server pid>,<session>`.
fn socket_of(value: &str) -> Option<PathBuf> {
    let (socket, _) = value.rsplit_once(',')?;
    let (socket, _) = socket.rsplit_once(',')?;
    (!socket.is_empty()).then(|| PathBuf::from(socket))
}

impl<R: CommandRunner> Tmux<R> {
    /// tmux run by `runner`, on the default server; tests pass a fake.
    pub fn with_runner(runner: R) -> Self {
        Self {
            runner,
            program: PathBuf::from(PROGRAM),
            socket: None,
            reported_socket: Mutex::default(),
            outside: Mutex::default(),
        }
    }

    /// The same, on the server at `socket`; `None` is the default server.
    #[must_use]
    pub fn on_socket(mut self, socket: Option<PathBuf>) -> Self {
        self.socket = socket;
        self
    }

    /// The arguments that name the server.
    fn server(&self) -> Vec<String> {
        match &self.socket {
            Some(socket) => vec!["-S".to_owned(), socket.display().to_string()],
            None => vec!["-L".to_owned(), "default".to_owned()],
        }
    }

    /// A `tmux` command line for the client: the program, the server, then
    /// `args`.
    fn command(&self, args: &[&str]) -> Vec<String> {
        std::iter::once(PROGRAM.to_owned())
            .chain(self.server())
            .chain(args.iter().map(|arg| (*arg).to_owned()))
            .collect()
    }

    /// Runs `tmux <server> <args>` and returns what it printed.
    fn run(&self, args: &[&str]) -> Result<String, MuxError> {
        let invocation = Invocation::new(&self.program, TIMEOUT)
            .args(self.server())
            .args(args.iter().copied());
        let output = self
            .runner
            .run(&invocation)
            .map_err(|error| cannot_run(&error))?;
        if output.is_success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let said = first_line(&output);
        if no_server(&said) {
            // What `volee daemon status` shows: tmux's own words name a
            // socket and an errno.
            Err(MuxError::Unavailable(match &self.socket {
                Some(socket) => format!("no tmux server runs on {}", socket.display()),
                None => "no tmux server is running".to_owned(),
            }))
        } else if said.starts_with("error connecting to") {
            // Another reason to fail to connect, such as a permission.
            Err(MuxError::Unavailable(said))
        } else if said.starts_with("can't find") {
            Err(MuxError::Refused {
                code: "not_found".to_owned(),
                message: said,
            })
        } else {
            Err(MuxError::Refused {
                code: "failed".to_owned(),
                message: said,
            })
        }
    }

    /// Every pane's process, with the pane's address. Also learns the
    /// server's socket, for the focus steps.
    fn panes(&self) -> Result<HashMap<u32, String>, MuxError> {
        let listed = self.run(&[
            "list-panes",
            "-a",
            "-F",
            "#{pane_pid}\t#{pane_id}\t#{socket_path}\t#{session_name}",
        ])?;
        let mut socket = None;
        let panes = listed
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(4, '\t');
                let pid = fields.next()?.parse().ok()?;
                let pane = fields.next()?;
                socket = Some(fields.next()?.to_owned()).filter(|path| !path.is_empty());
                let session = fields.next()?;
                Some((pid, target(session, pane)))
            })
            .collect();
        if let Some(socket) = socket {
            *self
                .reported_socket
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(socket);
        }
        Ok(panes)
    }

    /// The server's socket: as tmux reported it, else as `TMUX` named it.
    fn known_socket(&self) -> Option<String> {
        self.reported_socket
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .or_else(|| self.socket.as_ref().map(|path| path.display().to_string()))
    }

    /// Every process's parent.
    fn parents(&self) -> Result<HashMap<u32, u32>, MuxError> {
        let invocation = Invocation::new("ps", TIMEOUT).args(["-axo", "pid=,ppid="]);
        let output = self
            .runner
            .run(&invocation)
            .map_err(|error| cannot_run(&error))?;
        if !output.is_success() {
            return Err(MuxError::Unavailable(format!(
                "ps cannot list processes: {}",
                first_line(&output)
            )));
        }
        Ok(process::parents(&String::from_utf8_lossy(&output.stdout)))
    }
}

impl<R: CommandRunner> Multiplexer for Tmux<R> {
    fn name(&self) -> &'static str {
        PROGRAM
    }

    fn status(&self) -> MultiplexerStatus {
        let version = self
            .runner
            .run(&Invocation::new(&self.program, TIMEOUT).arg("-V"))
            .ok()
            .filter(Output::is_success)
            .and_then(|output| version(&String::from_utf8_lossy(&output.stdout)));
        let server = self.run(&["list-sessions", "-F", "#{session_name}"]);
        MultiplexerStatus {
            name: PROGRAM.to_owned(),
            available: server.is_ok(),
            version,
            detail: server.err().map(|error| error.to_string()),
        }
    }

    fn locate(&self, sessions: &[Hosted]) -> Result<Vec<Located>, MuxError> {
        let wanted: Vec<(SessionKey, u32)> = sessions
            .iter()
            .filter_map(|session| session.pid.map(|pid| (session.key.clone(), pid)))
            .collect();
        let mut outside = self.outside.lock().unwrap_or_else(PoisonError::into_inner);
        // Forget the sessions that are no longer asked about.
        outside.retain(|known| wanted.contains(known));
        let asked: Vec<(SessionKey, u32)> = wanted
            .into_iter()
            .filter(|known| !outside.contains(known))
            .collect();
        if asked.is_empty() {
            return Ok(Vec::new());
        }
        let panes = self.panes()?;
        let parents = if panes.is_empty() {
            HashMap::new()
        } else {
            self.parents()?
        };
        let mut located = Vec::new();
        for (key, pid) in asked {
            let pane = process::ancestors(pid, &parents)
                .iter()
                .find_map(|ancestor| panes.get(ancestor));
            match pane {
                Some(pane) => located.push(Located {
                    key,
                    target: pane.clone(),
                }),
                None => {
                    outside.insert((key, pid));
                }
            }
        }
        Ok(located)
    }

    fn capture(&self, target: &str) -> Result<String, MuxError> {
        let (_, pane) = split_target(target)?;
        let content = self.run(&["capture-pane", "-p", "-e", "-t", pane])?;
        Ok(screen::redraw(&content))
    }

    fn focus_steps(&self, target: &str) -> Result<Vec<Vec<String>>, MuxError> {
        let (_, pane) = split_target(target)?;
        // An unknown socket matches no `TMUX`: the switch is then left out,
        // and the person is told how to attach.
        let socket = self.known_socket().unwrap_or_default();
        let mut switch: Vec<String> = ["sh", "-c", ONLY_FROM_THIS_SERVER, "sh", &socket]
            .into_iter()
            .map(str::to_owned)
            .collect();
        switch.extend(self.command(&["switch-client", "-t", pane]));
        Ok(vec![
            self.command(&["select-window", "-t", pane]),
            self.command(&["select-pane", "-t", pane]),
            switch,
        ])
    }

    fn attach_command(&self, target: &str) -> Option<Vec<String>> {
        let (session, _) = split_target(target).ok()?;
        // A person runs it in a new terminal, which names no server: the
        // default server needs no flag.
        let mut command = vec![PROGRAM.to_owned()];
        if let Some(socket) = &self.socket {
            command.extend(["-S".to_owned(), socket.display().to_string()]);
        }
        // `=` asks for this exact session, not the first whose name starts
        // the same.
        command.extend(["attach".to_owned(), "-t".to_owned(), format!("={session}")]);
        Some(command)
    }
}

/// Whether tmux said that no server listens on its socket: `no server
/// running on <socket>`, or `error connecting to <socket> (No such file or
/// directory)`, or a socket left by a server that is gone (`Connection
/// refused`).
fn no_server(said: &str) -> bool {
    said.starts_with("no server running")
        || (said.starts_with("error connecting to")
            && (said.ends_with("(No such file or directory)")
                || said.ends_with("(Connection refused)")))
}

/// Why a program could not be run: tmux (or `ps`) is missing, or did not
/// answer in time.
fn cannot_run(error: &RunError) -> MuxError {
    MuxError::Unavailable(error.to_string())
}

/// The first line a failed program printed, on standard error or else on
/// standard output.
fn first_line(output: &Output) -> String {
    [&output.stderr, &output.stdout]
        .into_iter()
        .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
        .find(|text| !text.is_empty())
        .and_then(|text| text.lines().next().map(str::to_owned))
        .unwrap_or_else(|| match output.code {
            Some(code) => format!("tmux exited with status {code}"),
            None => "tmux was stopped by a signal".to_owned(),
        })
}

/// The version in `tmux -V`: `tmux 3.8`, `tmux 3.3a`.
fn version(text: &str) -> Option<String> {
    text.lines()
        .next()?
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use volee_harness::testing::FakeRunner;

    use super::*;

    fn hosted(id: &str, pid: Option<u32>) -> Hosted {
        Hosted {
            key: SessionKey::claude(id),
            pid,
        }
    }

    fn args(runner: &FakeRunner) -> Vec<Vec<String>> {
        runner
            .calls()
            .iter()
            .map(|call| {
                std::iter::once(call.program.display().to_string())
                    .chain(call.args_lossy())
                    .collect()
            })
            .collect()
    }

    fn failure(stderr: &str) -> Output {
        Output::failure(1, stderr)
    }

    /// `list-panes -a` as tmux 3.8 prints it: the shell of a pane in "my
    /// work", and a pane that runs a program of its own.
    const PANES: &str = "48809\t%0\t/private/tmp/tmux-501/default\tmy work\n\
                         48812\t%1\t/private/tmp/tmux-501/default\tmy work\n";

    /// `ps`: the session's process (950) runs under the shell of pane %0.
    const PS: &str = "48809 1\n900 48809\n950 900\n980 1\n";

    #[test]
    fn sessions_are_found_in_the_pane_that_runs_them() {
        let runner = FakeRunner::new()
            .with_reply(Ok(Output::success(PANES)))
            .with_reply(Ok(Output::success(PS)));
        let tmux = Tmux::with_runner(runner);
        let located = tmux
            .locate(&[hosted("in-pane", Some(950)), hosted("elsewhere", Some(980))])
            .unwrap();
        assert_eq!(
            located,
            [Located {
                key: SessionKey::claude("in-pane"),
                target: "my work:%0".to_owned(),
            }]
        );
        let calls = args(&tmux.runner);
        assert_eq!(
            calls[0],
            [
                "tmux",
                "-L",
                "default",
                "list-panes",
                "-a",
                "-F",
                "#{pane_pid}\t#{pane_id}\t#{socket_path}\t#{session_name}"
            ]
        );
        assert_eq!(calls[1], ["ps", "-axo", "pid=,ppid="]);
        // The session outside every pane is not asked about again, and the
        // one in a pane needs only the list of panes.
        tmux.runner.push_reply(Ok(Output::success(PANES)));
        tmux.runner.push_reply(Ok(Output::success(PS)));
        let again = tmux
            .locate(&[hosted("in-pane", Some(950)), hosted("elsewhere", Some(980))])
            .unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(tmux.runner.calls().len(), 4);
        // Nothing is asked without a session that has a process.
        let before = tmux.runner.calls().len();
        assert_eq!(tmux.locate(&[hosted("no-process", None)]).unwrap(), []);
        assert_eq!(tmux.runner.calls().len(), before);
    }

    #[test]
    fn a_missing_server_makes_tmux_unavailable() {
        let runner = FakeRunner::new()
            .with_reply(Ok(Output::success("tmux 3.8\n")))
            .with_reply(Ok(failure(
                "error connecting to /private/tmp/tmux-501/default (No such file or directory)\n",
            )));
        let tmux = Tmux::with_runner(runner);
        let status = tmux.status();
        assert!(!status.available);
        assert_eq!(status.version.as_deref(), Some("3.8"));
        assert_eq!(status.detail.as_deref(), Some("no tmux server is running"));
        assert!(matches!(
            Tmux::with_runner(FakeRunner::new().with_reply(Ok(failure(
                "no server running on /private/tmp/tmux-501/default\n"
            ))))
            .locate(&[hosted("s", Some(1))]),
            Err(MuxError::Unavailable(_))
        ));
    }

    #[test]
    fn a_capture_is_drawn_row_by_row_and_a_gone_pane_says_so() {
        let runner = FakeRunner::new()
            .with_reply(Ok(Output::success("\x1b[1mClaude Code\x1b[0m\n> \n\n")))
            .with_reply(Ok(failure("can't find pane: %9\n")));
        let tmux = Tmux::with_runner(runner);
        let screen = tmux.capture("my work:%0").unwrap();
        assert!(
            screen.starts_with("\x1b[0m\x1b[1;1H\x1b[1mClaude Code"),
            "{screen:?}"
        );
        assert_eq!(
            args(&tmux.runner)[0],
            [
                "tmux",
                "-L",
                "default",
                "capture-pane",
                "-p",
                "-e",
                "-t",
                "%0"
            ]
        );
        let error = tmux.capture("my work:%9").unwrap_err();
        assert_eq!(error.to_string(), "can't find pane: %9");
        assert!(matches!(tmux.capture("work"), Err(MuxError::BadTarget(_))));
    }

    #[test]
    fn focus_makes_the_pane_current_then_switches_a_client_of_this_server() {
        let tmux = Tmux::with_runner(FakeRunner::new().with_reply(Ok(Output::success(PANES))));
        // Before any `list-panes`, the server's socket is unknown: the
        // switch runs nowhere.
        let unknown = tmux.focus_steps("my work:%1").unwrap();
        assert_eq!(unknown[2][4], "");
        tmux.panes().unwrap();
        let steps = tmux.focus_steps("my work:%1").unwrap();
        assert_eq!(
            steps[..2],
            [
                vec!["tmux", "-L", "default", "select-window", "-t", "%1"],
                vec!["tmux", "-L", "default", "select-pane", "-t", "%1"],
            ]
        );
        assert_eq!(
            steps[2],
            [
                "sh",
                "-c",
                ONLY_FROM_THIS_SERVER,
                "sh",
                "/private/tmp/tmux-501/default",
                "tmux",
                "-L",
                "default",
                "switch-client",
                "-t",
                "%1"
            ]
        );
        assert_eq!(tmux.runner.calls().len(), 1, "the client runs the steps");
        assert_eq!(
            tmux.attach_command("my work:%1"),
            Some(vec![
                "tmux".into(),
                "attach".into(),
                "-t".into(),
                "=my work".into()
            ])
        );
    }

    #[test]
    fn the_switch_runs_only_in_a_client_of_the_same_server() {
        let run = |tmux_env: Option<&str>| {
            let mut command = std::process::Command::new("sh");
            command
                .args(["-c", ONLY_FROM_THIS_SERVER, "sh", "/tmp/tmux-501/de fault"])
                .args(["echo", "switched"]);
            match tmux_env {
                Some(value) => command.env("TMUX", value),
                None => command.env_remove("TMUX"),
            };
            command.output().unwrap()
        };
        let inside = run(Some("/tmp/tmux-501/de fault,4242,0"));
        assert!(inside.status.success());
        assert_eq!(String::from_utf8_lossy(&inside.stdout), "switched\n");
        for elsewhere in [None, Some("/tmp/tmux-501/work,4242,0"), Some("")] {
            let output = run(elsewhere);
            assert!(!output.status.success(), "{elsewhere:?}");
            assert!(output.stdout.is_empty(), "{elsewhere:?}");
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                "this terminal is not a client of the tmux server that runs the pane\n"
            );
        }
    }

    #[test]
    fn the_server_of_the_environment_is_named_everywhere() {
        let socket = PathBuf::from("/tmp/tmux-501/work");
        let tmux = Tmux::with_runner(FakeRunner::new()).on_socket(Some(socket));
        let switch = &tmux.focus_steps("w:%1").unwrap()[2];
        assert_eq!(
            switch[4..],
            [
                "/tmp/tmux-501/work",
                "tmux",
                "-S",
                "/tmp/tmux-501/work",
                "switch-client",
                "-t",
                "%1"
            ]
        );
        assert_eq!(
            tmux.attach_command("w:%1").unwrap(),
            ["tmux", "-S", "/tmp/tmux-501/work", "attach", "-t", "=w"]
        );
        assert_eq!(
            socket_of("/tmp/tmux-501/default,48809,0"),
            Some(PathBuf::from("/tmp/tmux-501/default"))
        );
        assert_eq!(
            socket_of("/tmp/a,b/sock,1,2"),
            Some(PathBuf::from("/tmp/a,b/sock"))
        );
        assert_eq!(socket_of(""), None);
        assert_eq!(version("tmux 3.3a\n").as_deref(), Some("3.3a"));
        assert_eq!(version("tmux next-3.9\n"), None);
    }

    #[test]
    fn only_a_missing_server_reads_as_no_server() {
        assert!(no_server(
            "no server running on /private/tmp/tmux-501/default"
        ));
        assert!(no_server(
            "error connecting to /private/tmp/tmux-501/default (No such file or directory)"
        ));
        assert!(no_server(
            "error connecting to /private/tmp/tmux-501/default (Connection refused)"
        ));
        assert!(!no_server(
            "error connecting to /private/tmp/tmux-501/default (Permission denied)"
        ));
        let named = Tmux::with_runner(
            FakeRunner::new().with_reply(Ok(failure("no server running on /tmp/tmux-501/work\n"))),
        )
        .on_socket(Some(PathBuf::from("/tmp/tmux-501/work")));
        assert_eq!(
            named.capture("w:%1").unwrap_err().to_string(),
            "no tmux server runs on /tmp/tmux-501/work"
        );
    }
}
