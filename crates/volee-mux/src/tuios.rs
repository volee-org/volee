//! [tuios](https://github.com/Gaurav-Gosain/tuios), through its control
//! protocol (`tuios call <verb> <json>`), as measured with tuios 0.12.1.
//!
//! - **Locate**: `list-agents` across every session gives the agent panes,
//!   with the harness session id that tuios's own hooks record
//!   (`tuios integration install claude-code`). A session they do not
//!   cover is looked for by its process: its pid and ancestors go to
//!   `resolve-pane`, which names the pane that runs one of them.
//! - **Agents**: `list-agents` also names the harness tuios detects in a
//!   pane from the program in its foreground (`source: detect`), for the
//!   harnesses of its manifest. The pane's own process comes from
//!   `list-windows`, and the process in the foreground of its terminal from
//!   `ps` (`tpgid`): that one is the agent (ADR 0030).
//! - **Capture**: `capture-pane`, with colors.
//! - **Focus**: `tuios focus-window -s <session> <window>` (which also shows
//!   the pane's workspace), then `tuios switch-session <session>`, which,
//!   run from a pane, switches the client showing that pane's session.
//! - **Server**: every command names its tuios server with `--server`: the
//!   one `TUIOS_SERVER` names in the daemon's environment, or the default
//!   server, which `--server ""` selects. tuios picks a server from that
//!   flag, else from `TUIOS_SERVER`; `TUIOS_SOCKET` only says which server a
//!   pane belongs to. So the focus steps, which a client runs in its own
//!   environment, reach the server where the daemon found the pane.
//!
//! Only reads and focus: nothing is typed into a pane, and no pane is
//! created, closed or moved.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use volee_core::harness::SessionKey;
use volee_harness::{CommandRunner, Invocation, Output, RunError, SystemRunner};

use crate::{
    Hosted, Located, Multiplexer, MultiplexerStatus, MuxError, PaneAgent, process, screen,
    split_target, target,
};

/// How long one command may take. A call answers in about 11 ms on an idle
/// machine.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The program, looked up on the `PATH`.
const PROGRAM: &str = "tuios";

/// tuios's code for a process that no pane runs.
const NOT_IN_A_PANE: &str = "window_not_found";

/// tuios, run by a [`CommandRunner`].
///
/// ```no_run
/// use volee_mux::{Multiplexer, Tuios};
///
/// let tuios = Tuios::new();
/// println!("{:?}", tuios.status());
/// ```
#[derive(Debug)]
pub struct Tuios<R = SystemRunner> {
    runner: R,
    program: PathBuf,
    /// The tuios server every command names: empty for the default one.
    server: String,
    /// Sessions found outside every pane, with the process they ran then:
    /// a process never moves into a pane, so they are not asked about again.
    outside: Mutex<HashSet<(SessionKey, u32)>>,
}

impl Tuios {
    /// tuios as `tuios` on the `PATH`, on the server `TUIOS_SERVER` names, as
    /// `tuios` itself reads it, or else on the default server.
    pub fn new() -> Self {
        Self::with_runner(SystemRunner).on_server(std::env::var("TUIOS_SERVER").unwrap_or_default())
    }
}

impl Default for Tuios {
    fn default() -> Self {
        Self::new()
    }
}

/// One pane of `list-agents`.
#[derive(Debug, Deserialize)]
struct AgentPane {
    session: String,
    window_id: String,
    #[serde(default)]
    harness_id: String,
    #[serde(default)]
    agent_session_id: String,
    /// What named the harness: `detect` when tuios recognized the program
    /// in the pane's foreground, `report` when a hook said so.
    #[serde(default)]
    source: String,
    #[serde(default)]
    confidence: String,
    #[serde(default)]
    cwd: String,
}

impl AgentPane {
    /// Whether tuios recognizes, now, the program in this pane's foreground
    /// as a harness: by its name, which it calls strong evidence. A hook's
    /// report says what ran there once, not what runs there now.
    fn runs_a_harness(&self) -> bool {
        !self.harness_id.is_empty() && self.source == "detect" && self.confidence == "strong"
    }
}

/// One window of `list-windows`: a pane and the process it started.
#[derive(Debug, Deserialize)]
struct Window {
    window_id: String,
    #[serde(default)]
    pid: u32,
}

/// The answer of `list-windows`.
#[derive(Debug, Deserialize)]
struct WindowList {
    #[serde(default)]
    windows: Vec<Window>,
}

/// The answer of `list-agents`.
#[derive(Debug, Deserialize)]
struct AgentList {
    #[serde(default)]
    agents: Vec<AgentPane>,
}

/// The answer of `resolve-pane`.
#[derive(Debug, Deserialize)]
struct Resolved {
    session: String,
    window_id: String,
}

/// The answer of `capture-pane`.
#[derive(Debug, Deserialize)]
struct Captured {
    content: String,
}

impl<R: CommandRunner> Tuios<R> {
    /// tuios run by `runner`, on the default server; tests pass a fake.
    pub fn with_runner(runner: R) -> Self {
        Self {
            runner,
            program: PathBuf::from(PROGRAM),
            server: String::new(),
            outside: Mutex::default(),
        }
    }

    /// The same, on the tuios server named `name`; an empty name is the
    /// default server.
    #[must_use]
    pub fn on_server(mut self, name: impl Into<String>) -> Self {
        self.server = name.into();
        self
    }

    /// A `tuios` command line for the client: the program, the server, then
    /// `args`.
    fn command(&self, args: &[&str]) -> Vec<String> {
        [PROGRAM, "--server", &self.server]
            .into_iter()
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    /// Runs `tuios call <verb> <params>` and reads its answer.
    fn call<T: serde::de::DeserializeOwned>(
        &self,
        verb: &str,
        params: &Value,
    ) -> Result<T, MuxError> {
        let invocation = Invocation::new(&self.program, TIMEOUT)
            .args(["--server", self.server.as_str(), "call", verb])
            .arg(params.to_string());
        let output = self
            .runner
            .run(&invocation)
            .map_err(|error| cannot_run(&error))?;
        let reply: Option<Value> = serde_json::from_slice(&output.stdout).ok();
        if let Some(error) = reply.as_ref().and_then(|reply| reply.get("error")) {
            let field = |name: &str| {
                error
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            return Err(MuxError::Refused {
                code: field("code"),
                message: field("message"),
            });
        }
        if !output.is_success() {
            // No server answered: tuios says why on its first line.
            return Err(MuxError::Unavailable(first_line(&output)));
        }
        reply
            .and_then(|reply| serde_json::from_value(reply).ok())
            .ok_or_else(|| MuxError::Unexpected {
                command: format!("{PROGRAM} call {verb}"),
            })
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

    /// For every process, the process group in the foreground of its
    /// terminal, whose leader is the program a person sees there.
    fn foregrounds(&self) -> Result<HashMap<u32, u32>, MuxError> {
        let invocation = Invocation::new("ps", TIMEOUT).args(["-axo", "pid=,tpgid="]);
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
        Ok(process::foregrounds(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    /// The pane that runs `pid` or one of its ancestors, if any.
    fn resolve(&self, pids: &[u32]) -> Result<Option<String>, MuxError> {
        match self.call::<Resolved>("resolve-pane", &json!({ "pids": pids })) {
            Ok(pane) => Ok(Some(target(&pane.session, &pane.window_id))),
            Err(MuxError::Refused { code, .. }) if code == NOT_IN_A_PANE => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl<R: CommandRunner> Multiplexer for Tuios<R> {
    fn name(&self) -> &'static str {
        PROGRAM
    }

    fn status(&self) -> MultiplexerStatus {
        let version = self
            .runner
            .run(&Invocation::new(&self.program, TIMEOUT).arg("--version"))
            .ok()
            .filter(Output::is_success)
            .and_then(|output| version(&String::from_utf8_lossy(&output.stdout)));
        let server = self.call::<Value>("list-sessions", &json!({}));
        MultiplexerStatus {
            name: PROGRAM.to_owned(),
            available: server.is_ok(),
            version,
            detail: server.err().map(|error| error.to_string()),
        }
    }

    fn locate(&self, sessions: &[Hosted]) -> Result<Vec<Located>, MuxError> {
        if sessions.is_empty() {
            return Ok(Vec::new());
        }
        let panes = self
            .call::<AgentList>("list-agents", &json!({ "all_sessions": true }))?
            .agents;
        let mut located = Vec::new();
        let mut by_process = Vec::new();
        for session in sessions {
            let recorded = panes.iter().find(|pane| {
                !pane.agent_session_id.is_empty()
                    && pane.agent_session_id == session.key.id
                    && pane.harness_id == session.key.harness.to_string()
            });
            match (recorded, session.pid) {
                (Some(pane), _) => located.push(Located {
                    key: session.key.clone(),
                    target: target(&pane.session, &pane.window_id),
                }),
                (None, Some(pid)) => by_process.push((session.key.clone(), pid)),
                (None, None) => {}
            }
        }
        let mut outside = self.outside.lock().unwrap_or_else(PoisonError::into_inner);
        // Forget the sessions that are no longer asked about.
        outside.retain(|known| by_process.contains(known));
        by_process.retain(|known| !outside.contains(known));
        if by_process.is_empty() {
            return Ok(located);
        }
        let parents = self.parents()?;
        for (key, pid) in by_process {
            match self.resolve(&process::ancestors(pid, &parents))? {
                Some(target) => located.push(Located { key, target }),
                None => {
                    outside.insert((key, pid));
                }
            }
        }
        Ok(located)
    }

    fn agents(&self, known: &[&str]) -> Result<Vec<PaneAgent>, MuxError> {
        let panes: Vec<AgentPane> = self
            .call::<AgentList>("list-agents", &json!({ "all_sessions": true }))?
            .agents
            .into_iter()
            .filter(|pane| pane.runs_a_harness() && !known.contains(&pane.harness_id.as_str()))
            // Without a directory there is nothing to list it under.
            .filter(|pane| !pane.cwd.is_empty())
            .collect();
        if panes.is_empty() {
            return Ok(Vec::new());
        }
        let foregrounds = self.foregrounds()?;
        // The windows of each session, asked once.
        let mut windows: HashMap<&str, Vec<Window>> = HashMap::new();
        let mut found = Vec::new();
        for pane in &panes {
            if !windows.contains_key(pane.session.as_str()) {
                let listed = self
                    .call::<WindowList>("list-windows", &json!({ "session": pane.session }))?
                    .windows;
                windows.insert(pane.session.as_str(), listed);
            }
            let started = windows
                .get(pane.session.as_str())
                .and_then(|listed| {
                    listed
                        .iter()
                        .find(|window| window.window_id == pane.window_id)
                })
                .map(|window| window.pid);
            // The agent is the process in the foreground of the terminal
            // the pane's own process holds. A pane whose process is gone,
            // or holds no terminal, makes no agent this time.
            let Some(pid) = started.and_then(|started| foregrounds.get(&started).copied()) else {
                continue;
            };
            found.push(PaneAgent {
                harness: pane.harness_id.clone(),
                pid,
                cwd: PathBuf::from(&pane.cwd),
                target: target(&pane.session, &pane.window_id),
            });
        }
        Ok(found)
    }

    fn capture(&self, target: &str) -> Result<String, MuxError> {
        let (session, window) = split_target(target)?;
        let captured: Captured = self.call(
            "capture-pane",
            &json!({ "session": session, "window": window, "styled": true }),
        )?;
        Ok(screen::redraw(&captured.content))
    }

    fn focus_steps(&self, target: &str) -> Result<Vec<Vec<String>>, MuxError> {
        let (session, window) = split_target(target)?;
        Ok(vec![
            self.command(&["focus-window", "-s", session, window]),
            self.command(&["switch-session", session]),
        ])
    }

    fn attach_command(&self, target: &str) -> Option<Vec<String>> {
        let (session, _) = split_target(target).ok()?;
        // A person runs it in a new terminal, which names no server: the
        // default server needs no flag.
        if self.server.is_empty() {
            Some(vec![
                PROGRAM.to_owned(),
                "attach".to_owned(),
                session.to_owned(),
            ])
        } else {
            Some(self.command(&["attach", session]))
        }
    }

    fn view_command(&self, target: &str) -> Option<Vec<String>> {
        let (session, window) = split_target(target).ok()?;
        // The pane takes the viewer's size while no client of tuios shows
        // its session, and goes back to its own when the viewer leaves.
        Some(self.command(&["pane-view", "-s", session, "-w", window]))
    }
}

/// Why a program could not be run: tuios (or `ps`) is missing, or did not
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
            Some(code) => format!("tuios exited with status {code}"),
            None => "tuios was stopped by a signal".to_owned(),
        })
}

/// The version in `tuios --version`: `tuios version 0.12.1 [pure-Go
/// backend]`.
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

    const CLAUDE_SESSION: &str = "e41ffe96-d588-4d61-b6a0-b75bdcebe52c";

    fn hosted(id: &str, pid: Option<u32>) -> Hosted {
        Hosted {
            key: SessionKey::claude(id),
            pid,
        }
    }

    fn reply(json: &str) -> Output {
        Output::success(json)
    }

    fn refusal(code: &str) -> Output {
        Output {
            code: Some(1),
            stdout: format!(r#"{{"error":{{"code":"{code}","message":"no pane runs it"}}}}"#)
                .into_bytes(),
            stderr: Vec::new(),
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

    /// `list-agents` as tuios 0.12.1 answers it: the agent pane its hooks
    /// recorded, and a pane nothing claimed.
    const AGENTS: &str = r#"{"agents":[
        {"session":"work","window_id":"c434b5b6","harness_id":"claude-code",
         "agent_session_id":"e41ffe96-d588-4d61-b6a0-b75bdcebe52c","state":"idle"},
        {"session":"work","window_id":"5b7420fb","harness_id":"","agent_session_id":""}
    ],"all_sessions":true,"type":"agent_list"}"#;

    #[test]
    fn sessions_are_found_by_the_id_tuios_hooks_record() {
        let tuios = Tuios::with_runner(FakeRunner::new().with_reply(Ok(reply(AGENTS))));
        let located = tuios
            .locate(&[hosted(CLAUDE_SESSION, Some(75_441))])
            .unwrap();
        assert_eq!(
            located,
            [Located {
                key: SessionKey::claude(CLAUDE_SESSION),
                target: "work:c434b5b6".into(),
            }]
        );
        assert_eq!(
            args(&tuios.runner),
            [[
                "tuios",
                "--server",
                "",
                "call",
                "list-agents",
                r#"{"all_sessions":true}"#
            ]]
        );
    }
    #[test]
    fn other_sessions_are_found_by_their_process_once() {
        let runner = FakeRunner::new()
            .with_reply(Ok(reply(r#"{"agents":[]}"#)))
            .with_reply(Ok(reply("  1 0\n 900 1\n 950 900\n 951 950\n 980 1\n")))
            .with_reply(Ok(reply(
                r#"{"by":"pid","pid":950,"session":"agents","type":"pane_resolved","window_id":"9a"}"#,
            )))
            .with_reply(Ok(refusal(NOT_IN_A_PANE)));
        let tuios = Tuios::with_runner(runner);
        let sessions = [
            hosted("in-a-pane", Some(951)),
            hosted("elsewhere", Some(980)),
        ];
        let located = tuios.locate(&sessions).unwrap();
        assert_eq!(
            located,
            [Located {
                key: SessionKey::claude("in-a-pane"),
                target: "agents:9a".into(),
            }]
        );
        let calls = args(&tuios.runner);
        assert_eq!(calls[1], ["ps", "-axo", "pid=,ppid="]);
        assert_eq!(
            calls[2],
            [
                "tuios",
                "--server",
                "",
                "call",
                "resolve-pane",
                r#"{"pids":[951,950,900]}"#
            ]
        );
        assert_eq!(calls[3][5], r#"{"pids":[980]}"#);

        // The next round asks again about the session in a pane (it may have
        // moved), and no longer about the one outside every pane.
        tuios.runner.push_reply(Ok(reply(r#"{"agents":[]}"#)));
        tuios.runner.push_reply(Ok(reply("951 950\n950 900\n")));
        tuios.runner.push_reply(Ok(reply(
            r#"{"session":"agents","window_id":"9a","type":"pane_resolved"}"#,
        )));
        assert_eq!(tuios.locate(&sessions).unwrap().len(), 1);
        assert_eq!(args(&tuios.runner).len(), 7, "{:?}", args(&tuios.runner));
        assert_eq!(tuios.runner.pending_replies(), 0);
    }

    /// `list-agents` as tuios 0.12.1 answers it for panes whose programs it
    /// recognizes by their names, with no hook installed (measured with
    /// programs named `codex` and `gemini`), next to a pane a hook reported
    /// and a pane nothing claimed.
    const DETECTED: &str = r#"{"agents":[
        {"session":"work","window_id":"w-codex","harness_id":"codex","agent_session_id":"",
         "source":"detect","confidence":"strong","identity":"manifest","foreground":"codex",
         "cwd":"/work/api","state":"working"},
        {"session":"side","window_id":"w-gemini","harness_id":"gemini-cli","agent_session_id":"",
         "source":"detect","confidence":"strong","foreground":"gemini","cwd":"/work/docs"},
        {"session":"work","window_id":"w-claude","harness_id":"claude-code",
         "agent_session_id":"e41ffe96-d588-4d61-b6a0-b75bdcebe52c","source":"report",
         "confidence":"certain","foreground":"claude","cwd":"/work/api"},
        {"session":"work","window_id":"w-opencode","harness_id":"opencode","agent_session_id":"",
         "source":"detect","confidence":"strong","foreground":"opencode","cwd":"/work/web"},
        {"session":"work","window_id":"w-stale","harness_id":"codex","agent_session_id":"s1",
         "source":"report","confidence":"certain","foreground":"fish","cwd":"/work/old"},
        {"session":"work","window_id":"w-shell","harness_id":"","agent_session_id":"",
         "source":"","confidence":"none","foreground":"fish","cwd":"/work"}
    ],"all_sessions":true,"type":"agent_list"}"#;

    /// `ps -axo pid=,tpgid=`: the pane `w-codex` was started with the program
    /// itself; in `w-gemini` a shell runs it as its foreground job.
    const FOREGROUNDS: &str = "  1     0\n 500   500\n 510   510\n 520   520\n 530   530\n 600   640\n 640   640\n 700     0\n";

    #[test]
    fn panes_whose_program_tuios_recognizes_are_agents_of_that_harness() {
        let runner = FakeRunner::new()
            .with_reply(Ok(reply(DETECTED)))
            .with_reply(Ok(reply(FOREGROUNDS)))
            .with_reply(Ok(reply(
                r#"{"windows":[{"window_id":"w-codex","pid":500},{"window_id":"w-claude","pid":510},
                    {"window_id":"w-stale","pid":520},{"window_id":"w-opencode","pid":530}]}"#,
            )))
            .with_reply(Ok(reply(
                r#"{"windows":[{"window_id":"w-gemini","pid":600}]}"#,
            )));
        let tuios = Tuios::with_runner(runner);
        let found = tuios.agents(&["claude-code", "opencode"]).unwrap();
        assert_eq!(
            found,
            [
                // The pane's own process is the agent.
                PaneAgent {
                    harness: "codex".into(),
                    pid: 500,
                    cwd: "/work/api".into(),
                    target: "work:w-codex".into(),
                },
                // The agent is the foreground job of the pane's shell.
                PaneAgent {
                    harness: "gemini-cli".into(),
                    pid: 640,
                    cwd: "/work/docs".into(),
                    target: "side:w-gemini".into(),
                },
            ],
            "not the harnesses the caller finds itself, nor a pane a hook once reported"
        );
        let calls = args(&tuios.runner);
        assert_eq!(calls[1], ["ps", "-axo", "pid=,tpgid="]);
        assert_eq!(calls[2][4..], ["list-windows", r#"{"session":"work"}"#]);
        assert_eq!(calls[3][4..], ["list-windows", r#"{"session":"side"}"#]);
        assert_eq!(calls.len(), 4, "one list of windows per session");
    }

    #[test]
    fn a_pane_whose_process_is_gone_or_holds_no_terminal_is_no_agent() {
        let agents = r#"{"agents":[
            {"session":"work","window_id":"gone","harness_id":"codex","source":"detect",
             "confidence":"strong","cwd":"/work/a"},
            {"session":"work","window_id":"no-tty","harness_id":"codex","source":"detect",
             "confidence":"strong","cwd":"/work/b"},
            {"session":"work","window_id":"closed","harness_id":"codex","source":"detect",
             "confidence":"strong","cwd":"/work/c"},
            {"session":"work","window_id":"weak","harness_id":"codex","source":"detect",
             "confidence":"weak","cwd":"/work/d"},
            {"session":"work","window_id":"nowhere","harness_id":"codex","source":"detect",
             "confidence":"strong","cwd":""}
        ]}"#;
        let runner = FakeRunner::new()
            .with_reply(Ok(reply(agents)))
            .with_reply(Ok(reply(FOREGROUNDS)))
            .with_reply(Ok(reply(
                r#"{"windows":[{"window_id":"gone","pid":999},{"window_id":"no-tty","pid":700}]}"#,
            )));
        let tuios = Tuios::with_runner(runner);
        assert_eq!(tuios.agents(&[]).unwrap(), []);
    }

    #[test]
    fn nothing_more_is_asked_when_no_pane_runs_another_harness() {
        let tuios = Tuios::with_runner(FakeRunner::new().with_reply(Ok(reply(AGENTS))));
        assert_eq!(tuios.agents(&["claude-code", "opencode"]).unwrap(), []);
        assert_eq!(
            tuios.runner.calls().len(),
            1,
            "list-agents, and nothing else"
        );
    }

    #[test]
    fn nothing_is_asked_without_sessions_or_processes() {
        let tuios = Tuios::with_runner(FakeRunner::new().with_reply(Ok(reply(r#"{"agents":[]}"#))));
        let none: [Located; 0] = [];
        assert_eq!(tuios.locate(&[]).unwrap(), none);
        assert_eq!(tuios.runner.calls().len(), 0);
        assert_eq!(tuios.locate(&[hosted("gone", None)]).unwrap(), none);
        assert_eq!(
            tuios.runner.calls().len(),
            1,
            "no ps for a session without a process"
        );
    }

    #[test]
    fn a_missing_server_or_program_makes_tuios_unavailable() {
        let not_running = Output {
            code: Some(3),
            stdout: Vec::new(),
            stderr: b"The TUIOS daemon is not running, and no sessions are saved on disk.\n\
                      Fix: run 'tuios new' to start a session."
                .to_vec(),
        };
        let tuios = Tuios::with_runner(FakeRunner::new().with_reply(Ok(not_running)));
        let error = tuios.locate(&[hosted(CLAUDE_SESSION, None)]).unwrap_err();
        assert!(
            matches!(&error, MuxError::Unavailable(text) if text.starts_with("The TUIOS daemon is not running")),
            "{error:?}"
        );

        let missing = FakeRunner::new()
            .with_reply(Err(RunError::NotFound {
                program: PROGRAM.into(),
            }))
            .with_reply(Err(RunError::NotFound {
                program: PROGRAM.into(),
            }));
        let tuios = Tuios::with_runner(missing);
        let status = tuios.status();
        assert!(!status.available);
        assert_eq!(status.version, None);
        assert_eq!(status.detail.as_deref(), Some("`tuios` was not found"));
    }

    #[test]
    fn the_status_names_the_version_of_a_running_server() {
        let runner = FakeRunner::new()
            .with_reply(Ok(reply(
                "tuios version 0.12.1 [pure-Go backend]\nCommit: 850335c\n",
            )))
            .with_reply(Ok(reply(r#"{"sessions":[],"type":"session_list"}"#)));
        let status = Tuios::with_runner(runner).status();
        assert_eq!(
            status,
            MultiplexerStatus {
                name: "tuios".into(),
                available: true,
                version: Some("0.12.1".into()),
                detail: None,
            }
        );
    }

    #[test]
    fn a_capture_is_drawn_row_by_row() {
        let runner = FakeRunner::new().with_reply(Ok(reply(
            r#"{"content":"\u001b[1mClaude Code\u001b[m\n> \n","styled":true,"type":"capture"}"#,
        )));
        let tuios = Tuios::with_runner(runner);
        let screen = tuios.capture("work:c434b5b6").unwrap();
        assert_eq!(
            screen,
            "\x1b[0m\x1b[1;1H\x1b[1mClaude Code\x1b[m\x1b[0m\x1b[2;1H> \x1b[0m\x1b[2;11H"
        );
        let calls = args(&tuios.runner);
        assert_eq!(
            calls[0][..5],
            ["tuios", "--server", "", "call", "capture-pane"]
        );
        let params: Value = serde_json::from_str(&calls[0][5]).unwrap();
        assert_eq!(
            params,
            json!({ "session": "work", "window": "c434b5b6", "styled": true })
        );
    }

    #[test]
    fn a_pane_that_is_gone_says_so() {
        let tuios =
            Tuios::with_runner(FakeRunner::new().with_reply(Ok(refusal("window_not_found"))));
        let error = tuios.capture("work:c434b5b6").unwrap_err();
        assert_eq!(error.to_string(), "no pane runs it");
        assert!(matches!(tuios.capture("work"), Err(MuxError::BadTarget(_))));
    }

    #[test]
    fn focus_shows_the_pane_then_its_session() {
        let tuios = Tuios::with_runner(FakeRunner::new());
        // The steps name the default server, so a client in a pane of
        // another server (whose TUIOS_SERVER names it) still reaches the
        // pane the daemon found.
        assert_eq!(
            tuios.focus_steps("work:0a1b2c3d").unwrap(),
            [
                vec![
                    "tuios",
                    "--server",
                    "",
                    "focus-window",
                    "-s",
                    "work",
                    "0a1b2c3d"
                ],
                vec!["tuios", "--server", "", "switch-session", "work"],
            ]
        );
        assert!(tuios.runner.calls().is_empty(), "the client runs the steps");
        assert_eq!(
            tuios.attach_command("work:0a1b2c3d"),
            Some(vec!["tuios".into(), "attach".into(), "work".into()])
        );
        // The pane alone, in a terminal of volee's (ADR 0027).
        let view: Vec<String> = ["tuios", "--server", "", "pane-view"]
            .into_iter()
            .chain(["-s", "work", "-w", "0a1b2c3d"])
            .map(str::to_owned)
            .collect();
        assert_eq!(tuios.view_command("work:0a1b2c3d"), Some(view));
        assert_eq!(tuios.view_command("no-pane"), None);
    }

    #[test]
    fn every_command_names_a_named_server() {
        let runner = FakeRunner::new().with_reply(Ok(reply(r#"{"content":"hi"}"#)));
        let tuios = Tuios::with_runner(runner).on_server("lab");
        tuios.capture("work:0a1b2c3d").unwrap();
        assert_eq!(
            args(&tuios.runner)[0][..4],
            ["tuios", "--server", "lab", "call"]
        );
        assert_eq!(
            tuios.focus_steps("work:0a1b2c3d").unwrap(),
            [
                vec![
                    "tuios",
                    "--server",
                    "lab",
                    "focus-window",
                    "-s",
                    "work",
                    "0a1b2c3d"
                ],
                vec!["tuios", "--server", "lab", "switch-session", "work"],
            ]
        );
        assert_eq!(
            tuios.attach_command("work:0a1b2c3d"),
            Some(vec![
                "tuios".into(),
                "--server".into(),
                "lab".into(),
                "attach".into(),
                "work".into()
            ])
        );
        assert_eq!(
            tuios.view_command("work:0a1b2c3d").unwrap()[..4],
            ["tuios", "--server", "lab", "pane-view"]
        );
    }
}
