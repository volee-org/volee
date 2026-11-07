//! Running git: one process per command, with standard input closed, a
//! timeout, and a limit on how many run at the same time (ADR 0010).
//!
//! On Unix, each git runs in a process group of its own. A command that
//! overruns its timeout is stopped as a whole, the processes git started
//! (ssh, filters, hooks) included: first with SIGTERM, so git can remove its
//! lock files and half-made worktrees, then with SIGKILL.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::GitError;
use crate::parse::{self, ParseError};

/// Environment variables that tie git to one repository. A daemon serves
/// many repositories, so they are removed and `-C <dir>` alone decides which
/// repository a command reads. This is the part of `git rev-parse
/// --local-env-vars` that locates a repository; `GIT_CONFIG_COUNT` and
/// `GIT_CONFIG_PARAMETERS` are kept, since they carry configuration the user
/// may rely on (such as `safe.directory` in a container).
const REPOSITORY_ENV: [&str; 11] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_DIR",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

/// Environment variables set for every command.
const FIXED_ENV: [(&str, &str); 4] = [
    // git's messages in English, so the few that are matched stay stable.
    ("LC_ALL", "C"),
    // Never ask for a user name or password on the terminal.
    ("GIT_TERMINAL_PROMPT", "0"),
    // Never let Git Credential Manager open a window or a browser.
    ("GCM_INTERACTIVE", "never"),
    // Never let ssh open a password or passphrase dialog.
    ("SSH_ASKPASS_REQUIRE", "never"),
];

/// The longest pause between two checks of a running command. Short enough
/// to add little latency to fast commands, long enough to cost nothing.
const MAX_POLL_PAUSE: Duration = Duration::from_millis(20);

/// How long a command that is being stopped gets to clean up after SIGTERM
/// before SIGKILL.
const TERMINATION_GRACE: Duration = Duration::from_secs(2);

/// How a program learns which directory a command is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    /// git: `git -C <dir> --no-optional-locks <args>`.
    Git,
    /// Any other program, such as `gh`: run with `<dir>` as its current
    /// directory.
    InDirectory,
}

/// Starts the processes of a git tool: git for [`GitCli`](crate::GitCli),
/// the GitHub CLI for [`GhCli`](crate::GhCli).
#[derive(Debug)]
pub(crate) struct Runner {
    program: PathBuf,
    style: Style,
    envs: Vec<(OsString, OsString)>,
    limiter: Limiter,
}

impl Runner {
    /// Runs git.
    pub(crate) fn git(program: PathBuf, max_concurrent: NonZeroUsize) -> Self {
        Self::with_style(program, Style::Git, max_concurrent)
    }

    /// Runs another program in the directory each command is about.
    pub(crate) fn in_directory(program: PathBuf, max_concurrent: NonZeroUsize) -> Self {
        Self::with_style(program, Style::InDirectory, max_concurrent)
    }

    fn with_style(program: PathBuf, style: Style, max_concurrent: NonZeroUsize) -> Self {
        Self {
            program,
            style,
            envs: Vec::new(),
            limiter: Limiter::new(max_concurrent),
        }
    }

    /// The program's file name, for messages: `git` or `gh`.
    pub(crate) fn program_name(&self) -> String {
        self.program.file_name().map_or_else(
            || self.program.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// Sets a variable for every command, over the ones set by default.
    pub(crate) fn set_env(&mut self, key: OsString, value: OsString) {
        self.envs.retain(|(existing, _)| *existing != key);
        self.envs.push((key, value));
    }

    pub(crate) fn set_max_concurrent(&mut self, limit: NonZeroUsize) {
        self.limiter = Limiter::new(limit);
    }

    /// Whether the person chose a value for `key`, in the environment of
    /// this process or with [`Runner::set_env`].
    pub(crate) fn person_sets(&self, key: &str) -> bool {
        self.envs.iter().any(|(existing, _)| existing == key) || std::env::var_os(key).is_some()
    }

    /// A command to run in `dir`, given `timeout` to finish.
    pub(crate) fn request<'a>(&'a self, dir: &'a Path, timeout: Duration) -> Request<'a> {
        Request {
            runner: self,
            dir,
            args: Vec::new(),
            envs: Vec::new(),
            timeout,
            max_stdout: None,
        }
    }

    /// The process for `git -C <dir> --no-optional-locks <args>` (or
    /// `<program> <args>` run in `<dir>`), ready to spawn. Variables set with
    /// [`Runner::set_env`] win over `envs`, which win over the fixed ones.
    fn command(&self, dir: &Path, args: &[OsString], envs: &[(OsString, OsString)]) -> Command {
        let mut command = Command::new(&self.program);
        match self.style {
            Style::Git => {
                command.arg("-C").arg(dir).arg("--no-optional-locks");
            }
            Style::InDirectory => {
                command.current_dir(dir);
            }
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in REPOSITORY_ENV {
            command.env_remove(key);
        }
        command.envs(FIXED_ENV);
        command.envs(envs.iter().map(|(key, value)| (key, value)));
        command.envs(self.envs.iter().map(|(key, value)| (key, value)));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // A group of its own: a timeout stops git and everything it
            // started, and nothing it starts can read from the person's
            // terminal (a background group is stopped if it tries).
            command.process_group(0);
        }
        command
    }

    fn execute(&self, request: &Request<'_>) -> Result<Completed, GitError> {
        let mut command = self.command(request.dir, &request.args, &request.envs);
        let rendered = render(&command);
        let _permit = self.limiter.acquire();
        let started = Instant::now();
        let mut child = command.spawn().map_err(|source| GitError::Spawn {
            program: self.program.clone(),
            source,
        })?;
        let deadline = Deadline::after(started, request.timeout);
        let outcome = supervise(&mut child, deadline, request.max_stdout);
        let elapsed = started.elapsed();
        match outcome {
            Ok(finished) => {
                tracing::debug!(
                    command = %rendered,
                    code = ?finished.status.code(),
                    truncated = finished.truncated,
                    ?elapsed,
                    "git finished"
                );
                Ok(Completed {
                    command: rendered,
                    code: finished.status.code(),
                    stdout: finished.stdout,
                    stderr: finished.stderr,
                    truncated: finished.truncated,
                })
            }
            Err(failure) => {
                // Never leave a git process behind, whatever went wrong.
                stop(&mut child);
                tracing::debug!(command = %rendered, ?elapsed, "git stopped");
                Err(match failure {
                    Failure::TimedOut => GitError::TimedOut {
                        command: rendered,
                        timeout: request.timeout,
                    },
                    Failure::Io(action, source) => GitError::Io {
                        action,
                        path: self.program.clone(),
                        source,
                    },
                })
            }
        }
    }
}

/// A git command being prepared: see [`Runner::request`].
#[derive(Debug)]
pub(crate) struct Request<'a> {
    runner: &'a Runner,
    dir: &'a Path,
    args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
    timeout: Duration,
    max_stdout: Option<usize>,
}

impl Request<'_> {
    /// Adds one argument.
    #[must_use]
    pub(crate) fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    /// Adds several arguments.
    #[must_use]
    pub(crate) fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        self
    }

    /// Sets environment variables for this command only. Variables the
    /// person set with [`GitCli::env`](crate::GitCli::env) still win.
    #[must_use]
    pub(crate) fn envs<I, K, V>(mut self, envs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        self.envs.extend(
            envs.into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        self
    }

    /// Gives the command another time limit.
    #[must_use]
    pub(crate) fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Stops reading the standard output once it holds more than `limit`
    /// bytes. git then stops on a broken pipe, and the result is marked
    /// [truncated](Completed::truncated) and counts as a success.
    #[must_use]
    pub(crate) fn max_stdout(mut self, limit: usize) -> Self {
        self.max_stdout = Some(limit);
        self
    }

    /// Runs the command and returns how it ended, whatever its exit code.
    pub(crate) fn run(self) -> Result<Completed, GitError> {
        self.runner.execute(&self)
    }

    /// Runs the command and returns its standard output; any exit code other
    /// than 0 is an error.
    pub(crate) fn stdout(self) -> Result<Vec<u8>, GitError> {
        self.run()?.success().map(|completed| completed.stdout)
    }

    /// Runs the command and parses its standard output; any exit code other
    /// than 0 is an error.
    pub(crate) fn parse<T>(
        self,
        parser: impl FnOnce(&[u8]) -> Result<T, ParseError>,
    ) -> Result<T, GitError> {
        self.run()?.success()?.parse(parser)
    }
}

/// A git command that ran to its end.
#[derive(Debug)]
pub(crate) struct Completed {
    /// The command line, for messages.
    pub(crate) command: String,
    /// The exit code; `None` when git was stopped by a signal.
    pub(crate) code: Option<i32>,
    /// Standard output.
    pub(crate) stdout: Vec<u8>,
    /// Standard error.
    pub(crate) stderr: Vec<u8>,
    /// The standard output went past the limit set with
    /// [`Request::max_stdout`]: it holds one byte more than the limit, and
    /// git was stopped before it finished.
    pub(crate) truncated: bool,
}

impl Completed {
    /// The command itself when it exited with 0 or was cut short on purpose,
    /// the failure otherwise.
    pub(crate) fn success(self) -> Result<Self, GitError> {
        if self.code == Some(0) || self.truncated {
            Ok(self)
        } else {
            Err(self.failure())
        }
    }

    /// The command as a [`GitError::Failed`].
    pub(crate) fn failure(self) -> GitError {
        GitError::Failed {
            command: self.command,
            code: self.code,
            stderr: String::from_utf8_lossy(&self.stderr).trim().to_owned(),
        }
    }

    /// Parses the standard output.
    pub(crate) fn parse<T>(
        &self,
        parser: impl FnOnce(&[u8]) -> Result<T, ParseError>,
    ) -> Result<T, GitError> {
        parser(&self.stdout).map_err(|error| GitError::UnexpectedOutput {
            command: self.command.clone(),
            detail: error.to_string(),
        })
    }

    /// The standard output as one trimmed line of UTF-8 text.
    pub(crate) fn text(&self) -> Result<String, GitError> {
        self.parse(|stdout| {
            std::str::from_utf8(stdout)
                .map(|text| text.trim().to_owned())
                .map_err(|_| ParseError::new("the output is not UTF-8"))
        })
    }

    /// The standard output as an object name, such as a commit.
    pub(crate) fn object_id(&self) -> Result<String, GitError> {
        self.parse(|stdout| parse::parse_object_id(&String::from_utf8_lossy(stdout)))
    }
}

/// When a running command must have finished.
#[derive(Clone, Copy, Debug)]
struct Deadline(Option<Instant>);

impl Deadline {
    /// `timeout` after `start`. A timeout too long to represent, such as
    /// [`Duration::MAX`], never expires.
    fn after(start: Instant, timeout: Duration) -> Self {
        Self(start.checked_add(timeout))
    }

    /// The time left; zero once the deadline has passed.
    fn remaining(self) -> Duration {
        self.0.map_or(Duration::MAX, |at| {
            at.saturating_duration_since(Instant::now())
        })
    }
}

/// Why a running command was abandoned.
enum Failure {
    /// It did not finish in time.
    TimedOut,
    /// Supervising it failed.
    Io(&'static str, io::Error),
}

/// How a supervised command ended.
struct Finished {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
}

/// Which pipe a reader thread drained.
#[derive(Clone, Copy)]
enum Pipe {
    Stdout,
    Stderr,
}

type PipeResult = (Pipe, io::Result<Vec<u8>>);

/// Waits for `child` until `deadline` and collects its output, at most one
/// byte past `max_stdout` on the standard output.
///
/// Both pipes are drained by threads while the child runs, so a command
/// that prints more than a pipe holds (a large diff) cannot block.
fn supervise(
    child: &mut Child,
    deadline: Deadline,
    max_stdout: Option<usize>,
) -> Result<Finished, Failure> {
    let (sender, receiver) = mpsc::channel();
    let stdout_limit =
        max_stdout.map(|limit| u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1));
    drain(
        child.stdout.take(),
        Pipe::Stdout,
        stdout_limit,
        sender.clone(),
    )?;
    drain(child.stderr.take(), Pipe::Stderr, None, sender)?;
    let status = wait_until(child, deadline)?;
    let (mut stdout, mut stderr) = (None, None);
    while stdout.is_none() || stderr.is_none() {
        // A process that git started and that outlived it may still hold
        // a pipe open: the deadline bounds that wait too.
        let (pipe, bytes) =
            receiver
                .recv_timeout(deadline.remaining())
                .map_err(|error| match error {
                    RecvTimeoutError::Timeout => Failure::TimedOut,
                    RecvTimeoutError::Disconnected => Failure::Io(
                        "read the output of",
                        io::Error::other("an output reader stopped early"),
                    ),
                })?;
        let bytes = bytes.map_err(|error| Failure::Io("read the output of", error))?;
        match pipe {
            Pipe::Stdout => stdout = Some(bytes),
            Pipe::Stderr => stderr = Some(bytes),
        }
    }
    let stdout = stdout.unwrap_or_default();
    let truncated = max_stdout.is_some_and(|limit| stdout.len() > limit);
    Ok(Finished {
        status,
        stdout,
        stderr: stderr.unwrap_or_default(),
        truncated,
    })
}

/// Reads `pipe` on a thread, to its end or up to `limit` bytes, and sends
/// the bytes. Stopping early closes the pipe, which stops the writer.
fn drain(
    pipe: Option<impl Read + Send + 'static>,
    which: Pipe,
    limit: Option<u64>,
    sender: Sender<PipeResult>,
) -> Result<(), Failure> {
    let Some(pipe) = pipe else {
        // Not piped: there is nothing to read.
        let _ = sender.send((which, Ok(Vec::new())));
        return Ok(());
    };
    thread::Builder::new()
        .name("volee-git-pipe".to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe
                .take(limit.unwrap_or(u64::MAX))
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            // The receiver is gone only when the command was abandoned.
            let _ = sender.send((which, result));
        })
        .map(drop)
        .map_err(|error| Failure::Io("start a reader thread for", error))
}

/// Waits for `child` to exit, polling with growing pauses, until `deadline`.
fn wait_until(child: &mut Child, deadline: Deadline) -> Result<ExitStatus, Failure> {
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| Failure::Io("wait for", error))?
        {
            return Ok(status);
        }
        let remaining = deadline.remaining();
        if remaining.is_zero() {
            return Err(Failure::TimedOut);
        }
        thread::sleep(pause.min(remaining));
        pause = pause.saturating_mul(2).min(MAX_POLL_PAUSE);
    }
}

/// Stops `child` and everything it started, then reaps it.
///
/// SIGTERM goes to its process group first, so git can remove its lock
/// files and half-made worktrees. Whatever still runs after
/// [`TERMINATION_GRACE`] gets SIGKILL. The child is reaped only after both
/// signals: until then, its process group cannot be reused, so the signals
/// can only reach git and the processes it started. Errors are ignored: the
/// processes may already be gone, and there is nothing better to do.
#[cfg(unix)]
fn stop(child: &mut Child) {
    use rustix::process::{Pid, Signal, kill_process_group};

    let group = Pid::from_child(child);
    let _ = kill_process_group(group, Signal::TERM);
    let deadline = Deadline::after(Instant::now(), TERMINATION_GRACE);
    let mut pause = Duration::from_millis(1);
    while !has_exited(child) && !deadline.remaining().is_zero() {
        thread::sleep(pause.min(deadline.remaining()));
        pause = pause.saturating_mul(2).min(MAX_POLL_PAUSE);
    }
    let _ = kill_process_group(group, Signal::KILL);
    let _ = child.wait();
}

/// Stops `child` and reaps it. Errors are ignored: the process may already
/// have exited, and there is nothing better to do.
#[cfg(not(unix))]
fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Whether `child` has exited, without reaping it. An error counts as
/// exited, so the caller stops waiting.
#[cfg(unix)]
fn has_exited(child: &Child) -> bool {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};

    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    !matches!(
        waitid(WaitId::Pid(Pid::from_child(child)), options),
        Ok(None)
    )
}

/// The command line of `command`, quoted so a POSIX shell would run it as
/// is. The environment is left out.
fn render(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `arg` as a POSIX shell word: unchanged when it is made of safe
/// characters only, single-quoted otherwise.
fn quote(arg: &OsStr) -> Cow<'_, str> {
    let text = arg.to_string_lossy();
    let safe = |byte: u8| byte.is_ascii_alphanumeric() || b"-_./=:@,+%".contains(&byte);
    if !text.is_empty() && text.bytes().all(safe) {
        text
    } else {
        Cow::Owned(format!("'{}'", text.replace('\'', r"'\''")))
    }
}

/// Lets at most a fixed number of git processes run at the same time.
#[derive(Debug)]
struct Limiter {
    max: usize,
    running: Mutex<usize>,
    freed: Condvar,
}

impl Limiter {
    fn new(max: NonZeroUsize) -> Self {
        Self {
            max: max.get(),
            running: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    /// Waits for a free slot and takes it until the permit is dropped.
    fn acquire(&self) -> Permit<'_> {
        // The lock guards a counter that is never left inconsistent, so a
        // poisoned lock is still safe to use.
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        while *running >= self.max {
            running = self
                .freed
                .wait(running)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *running += 1;
        Permit { limiter: self }
    }
}

/// A slot taken from a [`Limiter`], given back on drop.
struct Permit<'a> {
    limiter: &'a Limiter,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut running = self
            .limiter
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *running -= 1;
        self.limiter.freed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn runner() -> Runner {
        let mut runner = Runner::git(PathBuf::from("git"), NonZeroUsize::MIN);
        runner.set_env("GIT_CONFIG_GLOBAL".into(), "/dev/null".into());
        runner
    }

    fn envs_of(command: &Command) -> Vec<(&OsStr, Option<&OsStr>)> {
        command.get_envs().collect()
    }

    #[test]
    fn other_programs_run_in_the_directory() {
        let runner = Runner::in_directory(PathBuf::from("/usr/bin/gh"), NonZeroUsize::MIN);
        let command = runner.command(Path::new("/work/volee"), &["pr".into()], &[]);
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["pr"]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/work/volee")));
        assert_eq!(runner.program_name(), "gh");
    }

    #[test]
    fn commands_target_the_directory_without_optional_locks() {
        let command = runner().command(Path::new("/work/volee"), &["status".into()], &[]);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["-C", "/work/volee", "--no-optional-locks", "status"]);
    }

    #[test]
    fn commands_never_prompt_and_ignore_inherited_repositories() {
        let command = runner().command(Path::new("/work"), &[], &[]);
        let envs = envs_of(&command);
        for key in REPOSITORY_ENV {
            assert!(
                envs.contains(&(OsStr::new(key), None)),
                "{key} is not removed"
            );
        }
        for (key, value) in FIXED_ENV {
            assert!(envs.contains(&(OsStr::new(key), Some(OsStr::new(value)))));
        }
        assert!(envs.contains(&(
            OsStr::new("GIT_CONFIG_GLOBAL"),
            Some(OsStr::new("/dev/null"))
        )));
    }

    #[test]
    fn the_person_s_variables_win_over_the_command_s() {
        let mut runner = runner();
        runner.set_env("GIT_SSH_COMMAND".into(), "my-ssh".into());
        let command = runner.command(
            Path::new("/work"),
            &[],
            &[("GIT_SSH_COMMAND".into(), "ssh -o BatchMode=yes".into())],
        );
        let last = envs_of(&command)
            .into_iter()
            .rfind(|(key, _)| *key == "GIT_SSH_COMMAND");
        assert_eq!(
            last,
            Some((OsStr::new("GIT_SSH_COMMAND"), Some(OsStr::new("my-ssh"))))
        );
        assert!(runner.person_sets("GIT_SSH_COMMAND"));
    }

    #[test]
    fn deadlines_expire_and_unbounded_timeouts_do_not() {
        let now = Instant::now();
        assert_eq!(
            Deadline::after(now, Duration::ZERO).remaining(),
            Duration::ZERO
        );
        assert!(Deadline::after(now, Duration::from_secs(60)).remaining() > Duration::ZERO);
        assert_eq!(
            Deadline::after(now, Duration::MAX).remaining(),
            Duration::MAX
        );
    }

    #[test]
    fn a_later_env_replaces_an_earlier_one() {
        let mut runner = runner();
        runner.set_env("GIT_CONFIG_GLOBAL".into(), "/etc/other".into());
        let command = runner.command(Path::new("/work"), &[], &[]);
        let values: Vec<_> = envs_of(&command)
            .into_iter()
            .filter(|(key, _)| *key == "GIT_CONFIG_GLOBAL")
            .collect();
        assert_eq!(
            values,
            [(
                OsStr::new("GIT_CONFIG_GLOBAL"),
                Some(OsStr::new("/etc/other"))
            )]
        );
    }

    #[test]
    fn rendered_commands_can_be_pasted_into_a_shell() {
        let command = runner().command(
            Path::new("/work/my repo"),
            &[
                "rev-parse".into(),
                "main^{commit}".into(),
                "it's".into(),
                String::new().into(),
            ],
            &[],
        );
        assert_eq!(
            render(&command),
            "git -C '/work/my repo' --no-optional-locks rev-parse 'main^{commit}' 'it'\\''s' ''"
        );
    }

    #[test]
    fn the_limiter_caps_concurrency() {
        let limiter = Arc::new(Limiter::new(NonZeroUsize::new(2).unwrap()));
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (limiter, running, peak) = (limiter.clone(), running.clone(), peak.clone());
                thread::spawn(move || {
                    let _permit = limiter.acquire();
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(5));
                    running.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let peak = peak.load(Ordering::SeqCst);
        assert!((1..=2).contains(&peak), "peak was {peak}");
        assert_eq!(*limiter.running.lock().unwrap(), 0);
    }
}
