//! Asking an agent for a fix (v2, step 9, ADR 0041): how the text reaches
//! the session, and how volee knows it did.
//!
//! Measured on Claude Code 2.1.330, and kept in this one module because part
//! of it is not documented:
//!
//! - **A live session** (interactive or background, with a pid in
//!   `claude agents --json`) listens on `/tmp/cc-socks/<pid>.sock`, a Unix
//!   socket owned by the user with mode 0600 (cross-session messaging,
//!   documented at code.claude.com/docs/en/cross-session-messaging.md). One
//!   JSON line is written to it, `{"type":"user","message":{"role":"user",
//!   "content":"<text>"}}`; that line is the part that is not documented. The
//!   session shows "Another Claude session sent a message", so the text says
//!   it comes from the person, through volee.
//! - **A stopped session** is resumed in the background with
//!   `claude --bg --resume <session id> "<text>"` and no other option: any
//!   other option starts a copy. "Already running" is retried once.
//! - **Never** `claude --resume -p` on a session that is open (the two
//!   histories diverge silently) and never keys typed into a terminal.
//!
//! A write to the socket is not a delivery: volee confirms by the transcript
//! (a new user line with the text) or by the agent turning to `working`,
//! within a few seconds, and says "not delivered" otherwise. The caller then
//! offers the fallback: the text, to copy, and the terminal.

use std::io::Write;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Where the sockets of Claude Code sessions are.
pub(crate) const SOCKET_DIR: &str = "/tmp/cc-socks";

/// How long a delivery waits to see the agent take the message.
pub(crate) const CONFIRM_WITHIN: Duration = Duration::from_secs(10);

/// The session a fix is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Subject {
    /// The full Claude Code session id.
    pub(crate) session_id: String,
    /// Its process id, when the listing gives one.
    pub(crate) pid: Option<u32>,
    /// Its process is alive.
    pub(crate) alive: bool,
}

/// How a message reached an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Via {
    /// Written to the session's own socket.
    Socket,
    /// The session was stopped: resumed in the background with the text.
    Resume,
}

impl Via {
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::Socket => "socket",
            Self::Resume => "resume",
        }
    }
}

/// What delivering a fix request came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The agent has it.
    Delivered(Via, String),
    /// It could not be shown to have arrived: the caller offers the
    /// fallback.
    NotDelivered(String),
}

impl Outcome {
    pub(crate) fn delivered(&self) -> bool {
        matches!(self, Self::Delivered(..))
    }

    #[cfg(test)]
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Delivered(_, message) | Self::NotDelivered(message) => message,
        }
    }
}

/// Runs `claude --bg --resume`. A trait so that tests need no Claude Code.
pub(crate) trait Resumer: Send + Sync {
    /// Resumes `session` in the background with `text`. The error carries
    /// what the program said.
    fn resume(&self, session: &str, text: &str) -> Result<(), String>;

    /// Whether the session is alive, and its process id, read again now:
    /// the daemon's own view of the agents can be a few seconds old, and a
    /// session stopped since then must be resumed, not written to. The
    /// default trusts `subject`.
    fn liveness(&self, subject: &Subject) -> (bool, Option<u32>) {
        (subject.alive, subject.pid)
    }
}

/// Whether a process with this id exists.
fn process_exists(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .is_ok_and(|output| {
            output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
        })
}

/// What `claude agents --json --all` says about a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Listed {
    /// The output could not be read.
    Unreadable,
    /// It does not list the session.
    Missing,
    /// It lists the session, with or without a process.
    Session(Option<u32>),
}

fn listed(listing: &str, session: &str) -> Listed {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(listing) else {
        return Listed::Unreadable;
    };
    let Some(agents) = value
        .as_array()
        .or_else(|| value.get("agents").and_then(serde_json::Value::as_array))
    else {
        return Listed::Unreadable;
    };
    agents
        .iter()
        .find(|entry| entry.get("sessionId").and_then(serde_json::Value::as_str) == Some(session))
        .map_or(Listed::Missing, |entry| {
            Listed::Session(
                entry
                    .get("pid")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|pid| u32::try_from(pid).ok()),
            )
        })
}

/// The real program.
#[derive(Clone, Debug)]
pub(crate) struct ClaudeCli {
    pub(crate) program: String,
}

impl Resumer for ClaudeCli {
    fn resume(&self, session: &str, text: &str) -> Result<(), String> {
        let output = Command::new(&self.program)
            // Nothing else: another option makes `claude` start a copy.
            .args(["--bg", "--resume", session, text])
            .output()
            .map_err(|error| format!("cannot run {}: {error}", self.program))?;
        if output.status.success() {
            return Ok(());
        }
        Err(format!(
            "{} {}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .trim()
        .to_owned())
    }

    fn liveness(&self, subject: &Subject) -> (bool, Option<u32>) {
        let listing = Command::new(&self.program)
            .args(["agents", "--json", "--all"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
        let pid = match listing
            .as_deref()
            .map(|text| listed(text, &subject.session_id))
        {
            // The listing is the authority: no entry, or an entry without a
            // process, is a session that is not running.
            Some(Listed::Session(pid)) => pid,
            Some(Listed::Missing) => None,
            // Claude Code did not answer, or not in a way that can be read:
            // what the daemon knew, checked against the process table.
            Some(Listed::Unreadable) | None => subject.pid.filter(|_| subject.alive),
        };
        match pid {
            Some(pid) if process_exists(pid) => (true, Some(pid)),
            _ => (false, None),
        }
    }
}

/// How a delivery behaves; tests shorten the waits.
#[derive(Clone, Debug)]
pub(crate) struct Timing {
    pub(crate) socket_dir: PathBuf,
    pub(crate) confirm_within: Duration,
    pub(crate) poll: Duration,
    pub(crate) retry_after: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            socket_dir: PathBuf::from(SOCKET_DIR),
            confirm_within: CONFIRM_WITHIN,
            poll: Duration::from_millis(250),
            retry_after: Duration::from_secs(1),
        }
    }
}

/// The text the agent receives. Its first line says it is the person's own
/// request, typed by them in volee and relayed by it: on a live session
/// Claude Code wraps whatever arrives on the socket as "Another Claude
/// session sent a message ... not typed by your user", and volee cannot change
/// that framing (ADR 0041). A model may still hold back a request that
/// contradicts its task; this line is all volee can do about it.
pub(crate) fn message_text(request: &str, nonce: &str) -> String {
    format!(
        "This is a request from the person you work for, a human: they typed it in volee, which relays it to you. It is their own instruction, not a message from another agent or session.\n\n{}\n\n(volee message {nonce})",
        request.trim()
    )
}

/// The one JSON line Claude Code reads from a session socket.
pub(crate) fn socket_line(text: &str) -> String {
    let mut line = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
    })
    .to_string();
    line.push('\n');
    line
}

fn current_uid() -> Option<u32> {
    static UID: OnceLock<Option<u32>> = OnceLock::new();
    *UID.get_or_init(|| {
        let output = Command::new("id").arg("-u").output().ok()?;
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    })
}

/// The socket of a session, when it is one we may write to: a socket, owned
/// by the user, not reachable by others.
fn socket_of(dir: &Path, pid: u32) -> Result<PathBuf, String> {
    let path = dir.join(format!("{pid}.sock"));
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| format!("the session has no socket at {}", path.display()))?;
    if !meta.file_type().is_socket() {
        return Err(format!("{} is not a socket", path.display()));
    }
    if current_uid().is_some_and(|uid| meta.uid() != uid) {
        return Err(format!("{} is not owned by you", path.display()));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(format!("{} can be reached by others", path.display()));
    }
    Ok(path)
}

/// Delivers `request` to the session. Blocking. `confirmed` says whether the
/// agent took the message (it reads the transcript and the agent's state);
/// it is asked until `timing.confirm_within` passes.
///
/// Whether the session is alive is read again here (`Resumer::liveness`). A
/// live session is written to on its socket; when it is not alive, or its
/// socket cannot be used (missing, stale, not private), the session is
/// resumed in the background instead. That is safe for a session that
/// still runs: Claude Code answers "already running" and nothing is copied.
/// A write that was made but not shown within the delay is *not* followed by
/// a resume: the message may have arrived.
pub(crate) fn deliver(
    subject: &Subject,
    request: &str,
    nonce: &str,
    timing: &Timing,
    resumer: &dyn Resumer,
    confirmed: &mut dyn FnMut() -> bool,
) -> Outcome {
    let text = message_text(request, nonce);
    let (alive, pid) = resumer.liveness(subject);
    let mut because = None;
    if alive {
        match pid {
            Some(pid) => match by_socket(pid, &text, timing, confirmed) {
                Ok(outcome) => return outcome,
                Err(why) => because = Some(why),
            },
            None => {
                because = Some("Claude Code lists no process id for the session".to_owned());
            }
        }
    }
    by_resume(
        &subject.session_id,
        &text,
        timing,
        resumer,
        because.as_deref(),
    )
}

/// `Err` when the socket cannot be used at all: the caller resumes instead.
fn by_socket(
    pid: u32,
    text: &str,
    timing: &Timing,
    confirmed: &mut dyn FnMut() -> bool,
) -> Result<Outcome, String> {
    let path = socket_of(&timing.socket_dir, pid)?;
    let mut stream = UnixStream::connect(&path)
        .map_err(|error| format!("cannot reach the session's socket: {error}"))?;
    if let Err(error) = stream
        .write_all(socket_line(text).as_bytes())
        .and_then(|()| stream.flush())
    {
        return Err(format!("cannot write to the session's socket: {error}"));
    }
    let start = Instant::now();
    loop {
        if confirmed() {
            return Ok(Outcome::Delivered(
                Via::Socket,
                format!(
                    "The agent took the message ({}s).",
                    start.elapsed().as_secs()
                ),
            ));
        }
        if start.elapsed() >= timing.confirm_within {
            return Ok(Outcome::NotDelivered(format!(
                "written to the session's socket, but the agent did not show it within {}s",
                timing.confirm_within.as_secs()
            )));
        }
        std::thread::sleep(timing.poll);
    }
}

/// `because` is why a session that looked alive is resumed all the same.
fn by_resume(
    session: &str,
    text: &str,
    timing: &Timing,
    resumer: &dyn Resumer,
    because: Option<&str>,
) -> Outcome {
    let mut attempt = resumer.resume(session, text);
    if let Err(said) = &attempt
        && said.to_lowercase().contains("already running")
    {
        std::thread::sleep(timing.retry_after);
        attempt = resumer.resume(session, text);
    }
    match attempt {
        Ok(()) => Outcome::Delivered(
            Via::Resume,
            because.map_or_else(
                || "The session was not running: it was resumed in the background with your message.".to_owned(),
                |why| {
                    format!(
                        "The session's socket could not be used ({why}), so it was resumed in the background with your message."
                    )
                },
            ),
        ),
        Err(said) => Outcome::NotDelivered(because.map_or_else(
            || format!("`claude --bg --resume` failed: {said}"),
            |why| format!("{why}; resuming the session instead failed too: {said}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};

    use super::*;

    fn quick(dir: &Path) -> Timing {
        Timing {
            socket_dir: dir.to_path_buf(),
            confirm_within: Duration::from_millis(400),
            poll: Duration::from_millis(20),
            retry_after: Duration::from_millis(10),
        }
    }

    struct NoResume;
    impl Resumer for NoResume {
        fn resume(&self, _: &str, _: &str) -> Result<(), String> {
            panic!("a live session is never resumed");
        }
    }

    struct Script {
        answers: Mutex<Vec<Result<(), String>>>,
        calls: Mutex<Vec<(String, String)>>,
    }
    impl Resumer for Script {
        fn resume(&self, session: &str, text: &str) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push((session.into(), text.into()));
            self.answers.lock().unwrap().remove(0)
        }
    }

    fn listen(dir: &Path, pid: u32) -> Arc<Mutex<Vec<String>>> {
        let listener = UnixListener::bind(dir.join(format!("{pid}.sock"))).unwrap();
        std::fs::set_permissions(
            dir.join(format!("{pid}.sock")),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&lines);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    seen.lock().unwrap().push(line);
                }
            }
        });
        lines
    }

    #[test]
    fn the_socket_line_is_one_json_line_in_the_measured_shape() {
        let line = socket_line("fix \"it\"\nplease");
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1, "one line");
        let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(value["type"], "user");
        assert_eq!(value["message"]["role"], "user");
        assert_eq!(value["message"]["content"], "fix \"it\"\nplease");
    }

    #[test]
    fn the_message_says_it_comes_from_the_person_through_volee() {
        let text = message_text("  rename x  ", "ab12");
        let first = text.lines().next().unwrap();
        assert!(
            first.starts_with("This is a request from the person you work for, a human"),
            "{first}"
        );
        assert!(
            first.contains("relays it to you")
                && first.contains("not a message from another agent")
        );
        assert!(text.contains("rename x"));
        assert!(text.ends_with("(volee message ab12)"));
    }

    #[test]
    fn a_live_session_gets_the_line_on_its_socket_and_it_is_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let lines = listen(dir.path(), 4242);
        let subject = Subject {
            session_id: "s".into(),
            pid: Some(4242),
            alive: true,
        };
        let mut polls = 0;
        let outcome = deliver(
            &subject,
            "do it",
            "n1",
            &quick(dir.path()),
            &NoResume,
            &mut || {
                polls += 1;
                polls > 2
            },
        );
        assert!(outcome.delivered(), "{outcome:?}");
        assert!(matches!(outcome, Outcome::Delivered(Via::Socket, _)));
        std::thread::sleep(Duration::from_millis(50));
        let got = lines.lock().unwrap().clone();
        assert_eq!(got.len(), 1);
        let value: serde_json::Value = serde_json::from_str(&got[0]).unwrap();
        let content = value["message"]["content"].as_str().unwrap();
        assert!(content.contains("do it") && content.contains("(volee message n1)"));
    }

    #[test]
    fn a_write_the_agent_never_shows_is_not_a_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let _lines = listen(dir.path(), 4243);
        let subject = Subject {
            session_id: "s".into(),
            pid: Some(4243),
            alive: true,
        };
        let outcome = deliver(
            &subject,
            "x",
            "n",
            &quick(dir.path()),
            &NoResume,
            &mut || false,
        );
        assert!(!outcome.delivered());
        assert!(
            outcome.message().contains("did not show"),
            "{}",
            outcome.message()
        );
    }

    fn one_resume() -> Script {
        Script {
            answers: Mutex::new(vec![Ok(())]),
            calls: Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn a_live_session_without_a_usable_socket_is_resumed_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let subject = Subject {
            session_id: "s".into(),
            pid: Some(1),
            alive: true,
        };
        let resumer = one_resume();
        let outcome = deliver(
            &subject,
            "x",
            "n",
            &quick(dir.path()),
            &resumer,
            &mut || true,
        );
        assert!(
            matches!(outcome, Outcome::Delivered(Via::Resume, _)),
            "{outcome:?}"
        );
        assert!(
            outcome.message().contains("no socket"),
            "{}",
            outcome.message()
        );
        assert_eq!(resumer.calls.lock().unwrap().len(), 1);

        // A socket others can reach is refused too, and the session resumed.
        let _lines = listen(dir.path(), 2);
        std::fs::set_permissions(
            dir.path().join("2.sock"),
            std::fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        let loose = Subject {
            pid: Some(2),
            ..subject
        };
        let resumer = one_resume();
        let outcome = deliver(&loose, "x", "n", &quick(dir.path()), &resumer, &mut || true);
        assert!(
            outcome.message().contains("reached by others"),
            "{}",
            outcome.message()
        );
        assert!(outcome.delivered());
    }

    #[test]
    fn a_resume_that_fails_after_an_unusable_socket_says_both() {
        let dir = tempfile::tempdir().unwrap();
        let subject = Subject {
            session_id: "s".into(),
            pid: Some(1),
            alive: true,
        };
        let resumer = Script {
            answers: Mutex::new(vec![Err("no such session".into())]),
            calls: Mutex::new(Vec::new()),
        };
        let outcome = deliver(
            &subject,
            "x",
            "n",
            &quick(dir.path()),
            &resumer,
            &mut || true,
        );
        assert!(!outcome.delivered());
        assert!(
            outcome.message().contains("no socket"),
            "{}",
            outcome.message()
        );
        assert!(outcome.message().contains("no such session"));
    }

    /// A resumer whose fresh reading says the session is gone, whatever the
    /// daemon's own (stale) view was.
    struct Stopped(Script);
    impl Resumer for Stopped {
        fn resume(&self, session: &str, text: &str) -> Result<(), String> {
            self.0.resume(session, text)
        }
        fn liveness(&self, _: &Subject) -> (bool, Option<u32>) {
            (false, None)
        }
    }

    #[test]
    fn liveness_is_read_again_at_send_time_so_a_session_stopped_a_moment_ago_is_resumed() {
        let dir = tempfile::tempdir().unwrap();
        // Its old socket is still there and would accept the write.
        let lines = listen(dir.path(), 4300);
        let subject = Subject {
            session_id: "s".into(),
            pid: Some(4300),
            alive: true, // the daemon's cached view
        };
        let resumer = Stopped(one_resume());
        let outcome = deliver(
            &subject,
            "fix",
            "n",
            &quick(dir.path()),
            &resumer,
            &mut || true,
        );
        assert!(
            matches!(outcome, Outcome::Delivered(Via::Resume, _)),
            "{outcome:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            lines.lock().unwrap().is_empty(),
            "nothing was written to the stale socket"
        );
        assert_eq!(resumer.0.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_write_the_agent_never_shows_is_still_not_followed_by_a_resume() {
        let dir = tempfile::tempdir().unwrap();
        let _lines = listen(dir.path(), 4301);
        let subject = Subject {
            session_id: "s".into(),
            pid: Some(4301),
            alive: true,
        };
        // NoResume panics if a resume is attempted.
        let outcome = deliver(
            &subject,
            "x",
            "n",
            &quick(dir.path()),
            &NoResume,
            &mut || false,
        );
        assert!(!outcome.delivered());
    }

    #[test]
    fn the_listing_gives_the_pid_of_a_session_or_says_it_is_missing_or_unreadable() {
        let listing = r#"[{"sessionId":"a","pid":42},{"sessionId":"b"}]"#;
        assert_eq!(listed(listing, "a"), Listed::Session(Some(42)));
        assert_eq!(listed(listing, "b"), Listed::Session(None));
        assert_eq!(listed(listing, "c"), Listed::Missing);
        assert_eq!(
            listed(r#"{"agents":[{"sessionId":"a","pid":7}]}"#, "a"),
            Listed::Session(Some(7))
        );
        assert_eq!(listed("not json", "a"), Listed::Unreadable);
    }

    #[test]
    fn a_process_that_exists_is_told_from_one_that_does_not() {
        assert!(process_exists(std::process::id()));
        assert!(!process_exists(u32::MAX - 1));
    }

    #[test]
    fn a_stopped_session_is_resumed_once_more_after_already_running() {
        let dir = tempfile::tempdir().unwrap();
        let resumer = Script {
            answers: Mutex::new(vec![Err("session is already running".into()), Ok(())]),
            calls: Mutex::new(Vec::new()),
        };
        let subject = Subject {
            session_id: "11111111-2222".into(),
            pid: None,
            alive: false,
        };
        let outcome = deliver(
            &subject,
            "fix",
            "n",
            &quick(dir.path()),
            &resumer,
            &mut || false,
        );
        assert!(
            matches!(outcome, Outcome::Delivered(Via::Resume, _)),
            "{outcome:?}"
        );
        let calls = resumer.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "11111111-2222");
        assert!(calls[0].1.contains("fix"));
    }

    #[test]
    fn a_resume_that_fails_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let resumer = Script {
            answers: Mutex::new(vec![Err("no such session".into())]),
            calls: Mutex::new(Vec::new()),
        };
        let subject = Subject {
            session_id: "s".into(),
            pid: None,
            alive: false,
        };
        let outcome = deliver(
            &subject,
            "fix",
            "n",
            &quick(dir.path()),
            &resumer,
            &mut || false,
        );
        assert!(!outcome.delivered());
        assert!(outcome.message().contains("no such session"));
        assert_eq!(
            resumer.calls.lock().unwrap().len(),
            1,
            "only 'already running' is retried"
        );
    }
}
