//! "Open this agent's terminal": adapters for the terminal programs that can
//! show where an agent runs.
//!
//! An adapter only **locates and focuses**. It never reads a screen and never
//! types (volee observes, it does not own terminals). Today there is one,
//! Rex; tmux and `claude attach` can implement the same trait later. When no
//! adapter is available the interface hides the button.

use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// Where an agent runs, as far as the daemon knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// The agent's process id, when Claude Code lists one.
    pub(crate) pid: Option<u32>,
    /// Its working directory.
    pub(crate) cwd: PathBuf,
}

/// A way to bring an agent's terminal to the person.
pub(crate) trait Terminal: Send + Sync + Debug {
    /// A short name, sent to the interface.
    fn name(&self) -> &'static str;
    /// Whether the adapter can work now (the program is there and running).
    fn available(&self) -> bool;
    /// Focuses the terminal of `target`. Blocking: call it off the async
    /// threads. The answer is a sentence for the person.
    fn open(&self, target: &Target) -> Result<String, String>;
    /// Runs `argv` (no shell) in `cwd` in a new floating pane next to the
    /// agent's terminal: how a terminal editor opens. Blocking. The default
    /// is for adapters that cannot.
    fn open_command(
        &self,
        _target: &Target,
        _cwd: &Path,
        _argv: &[String],
    ) -> Result<String, String> {
        Err(format!("{} cannot open a command in a pane", self.name()))
    }
}

/// Runs the Rex command line. A trait so that tests need no Rex.
pub(crate) trait RexRunner: Send + Sync + Debug {
    /// Runs `rex <args>` and returns its standard output.
    fn run(&self, args: &[String]) -> Result<String, String>;
    /// The same, with `input` on its standard input.
    fn run_stdin(&self, _args: &[String], _input: &str) -> Result<String, String> {
        Err("this runner has no standard input".to_owned())
    }
}

/// The real command line.
#[derive(Debug)]
pub(crate) struct RexCli {
    bin: PathBuf,
    server: String,
}

impl RexRunner for RexCli {
    fn run(&self, args: &[String]) -> Result<String, String> {
        self.run_stdin(args, "")
    }

    fn run_stdin(&self, args: &[String], input: &str) -> Result<String, String> {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new(&self.bin)
            .arg("-S")
            .arg(&self.server)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot run rex: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            // A command that reads nothing may exit first: not an error.
            let _ = stdin.write_all(input.as_bytes());
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("cannot run rex: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "rex {}: {}",
                args.first().map_or("", String::as_str),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// The Rex adapter.
#[derive(Debug)]
pub(crate) struct Rex {
    runner: Box<dyn RexRunner>,
    /// The socket file of the Rex server: its absence means Rex is not
    /// running.
    socket: PathBuf,
    bin: PathBuf,
}

impl Rex {
    /// Looks for Rex on this machine: `REX_BIN`, else the app's `Current`
    /// build, else its highest numbered one; the server is `REX_SERVER`, else
    /// the app's default socket. `None` when either is missing at start.
    pub(crate) fn detect(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let bin = find_binary(home, env)?;
        let (server, socket) = if let Some(server) = env("REX_SERVER") {
            let socket = server.strip_prefix("unix://").map(unescape)?;
            (server, PathBuf::from(socket))
        } else {
            let socket = home.join("Library/Application Support/rex/server.sock");
            (format!("unix://{}", escape(&socket)), socket)
        };
        let runner = Box::new(RexCli {
            bin: bin.clone(),
            server,
        });
        Some(Self {
            runner,
            socket,
            bin,
        })
    }

    #[cfg(test)]
    fn with_runner(runner: Box<dyn RexRunner>) -> Self {
        Self {
            runner,
            socket: PathBuf::new(),
            bin: PathBuf::new(),
        }
    }

    fn json(&self, args: &[&str]) -> Result<Value, String> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let out = self.runner.run(&args)?;
        serde_json::from_str(&out).map_err(|error| format!("rex answered with bad JSON: {error}"))
    }

    /// Every terminal pane of every session, with the process ids and the
    /// directories Rex knows for it.
    fn panes(&self) -> Result<Vec<Pane>, String> {
        let sessions = self.json(&["ls", "--json"])?;
        let mut panes = Vec::new();
        for session in array(&sessions, "sessions") {
            let Some(session_id) = text(session, "session_id") else {
                continue;
            };
            let label = text(session, "label").unwrap_or_else(|| session_id.clone());
            let Ok(windows) = self.json(&["window", "ls", "-s", &session_id, "--json"]) else {
                continue;
            };
            for window in array(&windows, "windows") {
                let window_id = text(window, "window_id").unwrap_or_default();
                for layer in array(window, "layers") {
                    for block in array(layer, "blocks") {
                        if !is_terminal(block) {
                            continue;
                        }
                        let Some(block_id) = text(block, "block_id") else {
                            continue;
                        };
                        panes.push(Pane {
                            session: session_id.clone(),
                            session_label: label.clone(),
                            window: window_id.clone(),
                            block: block_id,
                            pids: Vec::new(),
                            cwds: Vec::new(),
                        });
                    }
                }
            }
        }
        Ok(panes)
    }

    /// Fills in what Rex says of the programs running in a pane.
    fn inspect(&self, pane: &mut Pane) {
        let (session, block) = (pane.session.clone(), pane.block.clone());
        if let Ok(status) = self.json(&[
            "block",
            "call",
            "-s",
            &session,
            "-b",
            &block,
            "program_status",
        ]) {
            for record in array(&status, "records") {
                if let Some(pid) = record.pointer("/owner/pid").and_then(Value::as_u64) {
                    pane.pids.push(pid);
                }
            }
        }
        if let Ok(process) = self.json(&["block", "call", "-s", &session, "-b", &block, "process"])
        {
            let mut procs: Vec<&Value> = Vec::new();
            procs.extend(process.get("child"));
            procs.extend(process.get("foreground"));
            procs.extend(array(&process, "ancestors"));
            for proc in procs {
                if let Some(pid) = proc.get("pid").and_then(Value::as_u64) {
                    pane.pids.push(pid);
                }
                if let Some(cwd) = text(proc, "cwd") {
                    pane.cwds.push(cwd);
                }
            }
        }
    }

    /// Asks a connected Rex app to show the pane's session. Rex only does it
    /// with Remote Control on; the failure is the caller's to explain.
    fn show_session(&self, pane: &Pane) -> Result<(), String> {
        let clients = self.json(&["client", "ls", "--json"])?;
        let app = array(&clients, "clients")
            .iter()
            .find(|client| {
                client.pointer("/info/kind").and_then(Value::as_str) == Some("app")
                    && text(client, "connection_state").as_deref() == Some("connected")
                    && client
                        .get("session_ids")
                        .and_then(Value::as_array)
                        .is_none_or(|ids| ids.iter().any(|id| id.as_str() == Some(&pane.session)))
            })
            .and_then(|client| text(client, "client_id"))
            .ok_or("no Rex app shows this session")?;
        let session = format!("session_id={}", pane.session);
        let window = format!("window_id={}", pane.window);
        let mut args = vec![
            "do",
            "-C",
            &app,
            "--timeout",
            "5s",
            "session.select",
            &session,
        ];
        if !pane.window.is_empty() {
            args.push(&window);
        }
        let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
        self.runner.run(&args).map(|_| ())
    }
}

impl Terminal for Rex {
    fn name(&self) -> &'static str {
        "rex"
    }

    fn available(&self) -> bool {
        self.bin.as_os_str().is_empty() || (self.bin.is_file() && self.socket.exists())
    }

    fn open_command(&self, target: &Target, cwd: &Path, argv: &[String]) -> Result<String, String> {
        if argv.is_empty() {
            return Err("nothing to run".to_owned());
        }
        let pane = self.locate(target)?;
        let inspected = self.json(&[
            "block",
            "inspect",
            "-s",
            &pane.session,
            &pane.block,
            "--json",
        ])?;
        let window = inspected
            .pointer("/block/window_id")
            .and_then(Value::as_str)
            .ok_or("Rex did not say the window of the pane")?
            .to_owned();
        // `env PATH=...` first: the Rex server's own PATH has neither Homebrew
        // nor the person's tools.
        let mut command = vec![
            "/usr/bin/env".to_owned(),
            format!(
                "PATH={}",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned())
            ),
        ];
        command.extend(argv.iter().cloned());
        let layer = serde_json::json!({
            "window_id": window,
            "bounds": [0.05, 0.08, 0.9, 0.84],
            "focus": true,
            "layout": { "block": {
                "flavor": "com.superlogical.terminal.shell",
                "label": format!("edit {}", argv.last().map_or("", String::as_str)),
                "options": { "cwd": cwd.to_string_lossy(), "command": command },
            } },
        });
        self.runner.run_stdin(
            &[
                "api".to_owned(),
                "call".to_owned(),
                "-s".to_owned(),
                pane.session.clone(),
                "session.new_layer".to_owned(),
                "-".to_owned(),
            ],
            &layer.to_string(),
        )?;
        // Best effort: bring the session to the person, as `open` does.
        let _ = self.show_session(&pane);
        Ok(format!(
            "Opened {} in a floating pane of the Rex session \"{}\"",
            argv.first().map_or("", String::as_str),
            pane.session_label
        ))
    }

    fn open(&self, target: &Target) -> Result<String, String> {
        let pane = self.locate(target)?;
        self.runner.run(&[
            "focus".to_owned(),
            "-s".to_owned(),
            pane.session.clone(),
            pane.block.clone(),
        ])?;
        match self.show_session(&pane) {
            Ok(()) => Ok(format!(
                "Showing it in Rex (session {})",
                pane.session_label
            )),
            Err(why) => Ok(format!(
                "Focused its pane; open the Rex session \"{}\" to see it ({why})",
                pane.session_label
            )),
        }
    }
}

impl Rex {
    /// The Rex terminal pane the agent of `target` runs in.
    fn locate(&self, target: &Target) -> Result<Pane, String> {
        let mut panes = self.panes()?;
        // Rex's own record of the process wins: pids are exact. A directory
        // is only a guess and counts when exactly one pane has it.
        let mut found = None;
        for (index, pane) in panes.iter_mut().enumerate() {
            self.inspect(pane);
            if target
                .pid
                .is_some_and(|pid| pane.pids.contains(&u64::from(pid)))
            {
                found = Some(index);
                break;
            }
        }
        let index = if let Some(index) = found {
            index
        } else {
            let cwd = target.cwd.to_string_lossy();
            let mut same = panes
                .iter()
                .enumerate()
                .filter(|(_, pane)| pane.cwds.iter().any(|dir| *dir == cwd));
            match (same.next(), same.next()) {
                (Some((index, _)), None) => index,
                (Some(_), Some(_)) => {
                    return Err("several Rex terminals are in this directory".to_owned());
                }
                _ => return Err("this agent does not run in a Rex terminal".to_owned()),
            }
        };
        Ok(panes.swap_remove(index))
    }
}

#[derive(Debug)]
struct Pane {
    session: String,
    session_label: String,
    window: String,
    block: String,
    pids: Vec<u64>,
    cwds: Vec<String>,
}

fn is_terminal(block: &Value) -> bool {
    text(block, "creator_name")
        .or_else(|| text(block, "flavor"))
        .is_some_and(|name| name.starts_with("com.superlogical.terminal"))
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

const APP_BIN: &str = "Rex Server.app/Contents/MacOS/rex";

fn find_binary(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(explicit) = env("REX_BIN") {
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }
    let versions = home.join("Library/Application Support/com.superlogical.rex/RexServer/Versions");
    let current = versions.join("Current").join(APP_BIN);
    if current.is_file() {
        return Some(current);
    }
    let mut builds: Vec<u64> = std::fs::read_dir(&versions)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect();
    builds.sort_unstable_by(|a, b| b.cmp(a));
    builds
        .into_iter()
        .map(|build| versions.join(build.to_string()).join(APP_BIN))
        .find(|path| path.is_file())
}

/// A path as it appears in a `unix://` address: spaces and `%` escaped.
fn escape(path: &Path) -> String {
    path.to_string_lossy()
        .replace('%', "%25")
        .replace(' ', "%20")
}

fn unescape(address: &str) -> String {
    address.replace("%20", " ").replace("%25", "%")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// Answers `rex` calls from a table keyed by the arguments, and records
    /// every call.
    #[derive(Debug, Default)]
    struct Fake {
        answers: HashMap<String, Result<String, String>>,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn on(mut self, args: &str, answer: &str) -> Self {
            self.answers.insert(args.to_owned(), Ok(answer.to_owned()));
            self
        }
        fn failing(mut self, args: &str, why: &str) -> Self {
            self.answers.insert(args.to_owned(), Err(why.to_owned()));
            self
        }
    }

    #[derive(Debug)]
    struct Shared(std::sync::Arc<Fake>);

    impl RexRunner for Shared {
        fn run(&self, args: &[String]) -> Result<String, String> {
            let key = args.join(" ");
            self.0.calls.lock().unwrap().push(key.clone());
            self.0
                .answers
                .get(&key)
                .cloned()
                .unwrap_or_else(|| Err(format!("no answer for {key}")))
        }

        fn run_stdin(&self, args: &[String], input: &str) -> Result<String, String> {
            self.0
                .calls
                .lock()
                .unwrap()
                .push(format!("{} <<{input}", args.join(" ")));
            Ok("{}".to_owned())
        }
    }

    const SESSIONS: &str = r#"{"sessions":[{"session_id":"S1","label":"Work"}]}"#;
    const WINDOWS: &str = r#"{"windows":[{"window_id":"W1","layers":[{"blocks":[
        {"block_id":"B1","creator_name":"com.superlogical.terminal"},
        {"block_id":"B2","creator_name":"com.superlogical.terminal"},
        {"block_id":"X","creator_name":"com.superlogical.editor"}]}]}]}"#;

    fn rex(fake: Fake) -> (Rex, std::sync::Arc<Fake>) {
        let fake = std::sync::Arc::new(fake);
        (
            Rex::with_runner(Box::new(Shared(std::sync::Arc::clone(&fake)))),
            fake,
        )
    }

    fn base() -> Fake {
        Fake::default()
            .on("ls --json", SESSIONS)
            .on("window ls -s S1 --json", WINDOWS)
    }

    #[test]
    fn the_pane_whose_program_has_the_agents_pid_is_focused() {
        let fake = base()
            .on(
                "block call -s S1 -b B1 program_status",
                r#"{"records":[{"owner":{"pid":10}}]}"#,
            )
            .on(
                "block call -s S1 -b B1 process",
                r#"{"foreground":{"pid":10,"cwd":"/a"}}"#,
            )
            .on(
                "block call -s S1 -b B2 program_status",
                r#"{"records":[{"owner":{"pid":77}}]}"#,
            )
            .on(
                "block call -s S1 -b B2 process",
                r#"{"foreground":{"pid":77,"cwd":"/b"}}"#,
            )
            .on("focus -s S1 B2", "")
            .failing("client ls --json", "no clients");
        let (rex, fake) = rex(fake);
        let said = rex
            .open(&Target {
                pid: Some(77),
                cwd: "/elsewhere".into(),
            })
            .unwrap();
        assert!(said.contains("Focused its pane"), "{said}");
        assert!(
            fake.calls
                .lock()
                .unwrap()
                .contains(&"focus -s S1 B2".to_owned())
        );
    }

    #[test]
    fn without_a_pid_match_a_single_pane_in_the_directory_is_used() {
        let fake = base()
            .on("block call -s S1 -b B1 program_status", r#"{"records":[]}"#)
            .on("block call -s S1 -b B1 process", r#"{"foreground":{"pid":1,"cwd":"/a"}}"#)
            .on("block call -s S1 -b B2 program_status", r#"{"records":[]}"#)
            .on("block call -s S1 -b B2 process", r#"{"foreground":{"pid":2,"cwd":"/b"}}"#)
            .on("focus -s S1 B1", "")
            .on(
                "client ls --json",
                r#"{"clients":[{"client_id":"C1","info":{"kind":"app"},"connection_state":"connected","session_ids":["S1"]}]}"#,
            )
            .on("do -C C1 --timeout 5s session.select session_id=S1 window_id=W1", "");
        let (rex, _) = rex(fake);
        let said = rex
            .open(&Target {
                pid: None,
                cwd: "/a".into(),
            })
            .unwrap();
        assert!(said.starts_with("Showing it in Rex"), "{said}");
    }

    #[test]
    fn a_terminal_editor_opens_in_a_floating_pane_with_its_exact_argv() {
        let fake = base()
            .on(
                "block call -s S1 -b B1 program_status",
                r#"{"records":[{"owner":{"pid":10}}]}"#,
            )
            .on(
                "block call -s S1 -b B1 process",
                r#"{"foreground":{"pid":10,"cwd":"/a"}}"#,
            )
            .on("block call -s S1 -b B2 program_status", r#"{"records":[]}"#)
            .on("block call -s S1 -b B2 process", r"{}")
            .on(
                "block inspect -s S1 B1 --json",
                r#"{"block":{"window_id":"W1"}}"#,
            )
            .failing("client ls --json", "no clients");
        let (rex, fake) = rex(fake);
        let said = rex
            .open_command(
                &Target {
                    pid: Some(10),
                    cwd: "/a".into(),
                },
                Path::new("/work/tree"),
                &[
                    "nvim".to_owned(),
                    "+12".to_owned(),
                    "/work/tree/a b.rs".to_owned(),
                ],
            )
            .unwrap();
        assert!(said.contains("floating pane"), "{said}");
        let calls = fake.calls.lock().unwrap();
        let call = calls
            .iter()
            .find(|call| call.contains("api call -s S1 session.new_layer -"))
            .expect("the layer is created");
        let (_, input) = call.split_once("<<").unwrap();
        let layer: Value = serde_json::from_str(input).unwrap();
        assert_eq!(layer["window_id"], "W1");
        let options = &layer["layout"]["block"]["options"];
        assert_eq!(options["cwd"], "/work/tree");
        let command = options["command"].as_array().unwrap();
        assert_eq!(command[0], "/usr/bin/env");
        assert!(command[1].as_str().unwrap().starts_with("PATH="));
        assert_eq!(command[2], "nvim");
        assert_eq!(
            command[4], "/work/tree/a b.rs",
            "a path with a space stays one argument"
        );
    }

    #[test]
    fn two_panes_in_one_directory_are_not_guessed() {
        let fake = base()
            .on("block call -s S1 -b B1 program_status", r#"{"records":[]}"#)
            .on(
                "block call -s S1 -b B1 process",
                r#"{"foreground":{"pid":1,"cwd":"/a"}}"#,
            )
            .on("block call -s S1 -b B2 program_status", r#"{"records":[]}"#)
            .on(
                "block call -s S1 -b B2 process",
                r#"{"foreground":{"pid":2,"cwd":"/a"}}"#,
            );
        let (rex, _) = rex(fake);
        let error = rex
            .open(&Target {
                pid: Some(9),
                cwd: "/a".into(),
            })
            .unwrap_err();
        assert!(error.contains("several"), "{error}");
    }

    #[test]
    fn an_agent_outside_rex_is_said_so() {
        let fake = base()
            .on("block call -s S1 -b B1 program_status", r#"{"records":[]}"#)
            .on("block call -s S1 -b B1 process", r"{}")
            .on("block call -s S1 -b B2 program_status", r#"{"records":[]}"#)
            .on("block call -s S1 -b B2 process", r"{}");
        let (rex, _) = rex(fake);
        let error = rex
            .open(&Target {
                pid: Some(9),
                cwd: "/nowhere".into(),
            })
            .unwrap_err();
        assert!(error.contains("does not run in a Rex terminal"), "{error}");
    }

    #[test]
    fn a_rex_that_fails_is_an_error_not_a_panic() {
        let (rex, _) = rex(Fake::default().failing("ls --json", "server down"));
        assert!(
            rex.open(&Target {
                pid: Some(1),
                cwd: "/".into(),
            })
            .is_err()
        );
    }

    #[test]
    fn the_server_address_escapes_spaces() {
        let path = Path::new("/Users/me/Library/Application Support/rex/server.sock");
        let address = escape(path);
        assert_eq!(
            address,
            "/Users/me/Library/Application%20Support/rex/server.sock"
        );
        assert_eq!(unescape(&address), path.to_string_lossy());
    }

    #[test]
    fn rex_is_not_detected_where_it_is_not_installed() {
        let home = tempfile::tempdir().unwrap();
        assert!(Rex::detect(home.path(), &|_| None).is_none());
    }

    #[test]
    fn the_highest_build_is_used_when_there_is_no_current_link() {
        let home = tempfile::tempdir().unwrap();
        for build in ["900", "2061", "1000"] {
            let dir = home.path().join(format!(
                "Library/Application Support/com.superlogical.rex/RexServer/Versions/{build}/Rex Server.app/Contents/MacOS"
            ));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("rex"), "").unwrap();
        }
        let bin = find_binary(home.path(), &|_| None).unwrap();
        assert!(bin.to_string_lossy().contains("/2061/"), "{bin:?}");
    }
}
