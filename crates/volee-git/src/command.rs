//! Running git: one process per command, with standard input closed, a
//! timeout, and a limit on how many run at the same time (ADR 0010).

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
const FIXED_ENV: [(&str, &str); 3] = [
    // git's messages in English, so the few that are matched stay stable.
    ("LC_ALL", "C"),
    // Never ask for a user name or password on the terminal.
    ("GIT_TERMINAL_PROMPT", "0"),
    // Never let Git Credential Manager open a window or a browser.
    ("GCM_INTERACTIVE", "never"),
];

/// The longest pause between two checks of a running command. Short enough
/// to add little latency to fast commands, long enough to cost nothing.
const MAX_POLL_PAUSE: Duration = Duration::from_millis(20);

/// Starts git processes for [`GitCli`](crate::GitCli).
#[derive(Debug)]
pub(crate) struct Runner {
    program: PathBuf,
    envs: Vec<(OsString, OsString)>,
    limiter: Limiter,
}

impl Runner {
    pub(crate) fn new(program: PathBuf, max_concurrent: NonZeroUsize) -> Self {
        Self {
            program,
            envs: Vec::new(),
            limiter: Limiter::new(max_concurrent),
        }
    }

    pub(crate) fn set_env(&mut self, key: OsString, value: OsString) {
        self.envs.retain(|(existing, _)| *existing != key);
        self.envs.push((key, value));
    }

    pub(crate) fn set_max_concurrent(&mut self, limit: NonZeroUsize) {
        self.limiter = Limiter::new(limit);
    }

    /// A command to run in `dir`, given `timeout` to finish.
    pub(crate) fn request<'a>(&'a self, dir: &'a Path, timeout: Duration) -> Request<'a> {
        Request {
            runner: self,
            dir,
            args: Vec::new(),
            timeout,
        }
    }

    /// The process for `git -C <dir> --no-optional-locks <args>`, ready to
    /// spawn.
    fn command(&self, dir: &Path, args: &[OsString]) -> Command {
        let mut command = Command::new(&self.program);
        command
            .arg("-C")
            .arg(dir)
            .arg("--no-optional-locks")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in REPOSITORY_ENV {
            command.env_remove(key);
        }
        command.envs(FIXED_ENV);
        command.envs(self.envs.iter().map(|(key, value)| (key, value)));
        command
    }

    fn execute(
        &self,
        dir: &Path,
        args: &[OsString],
        timeout: Duration,
    ) -> Result<Completed, GitError> {
        let mut command = self.command(dir, args);
        let rendered = render(&command);
        let _permit = self.limiter.acquire();
        let started = Instant::now();
        let mut child = command.spawn().map_err(|source| GitError::Spawn {
            program: self.program.clone(),
            source,
        })?;
        let outcome = supervise(&mut child, Deadline::after(started, timeout));
        let elapsed = started.elapsed();
        match outcome {
            Ok((status, stdout, stderr)) => {
                tracing::debug!(
                    command = %rendered,
                    code = ?status.code(),
                    ?elapsed,
                    "git finished"
                );
                Ok(Completed {
                    command: rendered,
                    code: status.code(),
                    stdout,
                    stderr,
                })
            }
            Err(failure) => {
                // Never leave a git process behind, whatever went wrong.
                stop(&mut child);
                tracing::debug!(command = %rendered, ?elapsed, "git stopped");
                Err(match failure {
                    Failure::TimedOut => GitError::TimedOut {
                        command: rendered,
                        timeout,
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
    timeout: Duration,
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

    /// Gives the command another time limit.
    #[must_use]
    pub(crate) fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Runs the command and returns how it ended, whatever its exit code.
    pub(crate) fn run(self) -> Result<Completed, GitError> {
        self.runner.execute(self.dir, &self.args, self.timeout)
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
}

impl Completed {
    /// The command itself when it exited with 0, the failure otherwise.
    pub(crate) fn success(self) -> Result<Self, GitError> {
        if self.code == Some(0) {
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

/// Which pipe a reader thread drained.
#[derive(Clone, Copy)]
enum Pipe {
    Stdout,
    Stderr,
}

type PipeResult = (Pipe, io::Result<Vec<u8>>);

/// Waits for `child` until `deadline` and collects its output.
///
/// Both pipes are drained by threads while the child runs, so a command
/// that prints more than a pipe holds (a large diff) cannot block.
fn supervise(
    child: &mut Child,
    deadline: Deadline,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), Failure> {
    let (sender, receiver) = mpsc::channel();
    drain(child.stdout.take(), Pipe::Stdout, sender.clone())?;
    drain(child.stderr.take(), Pipe::Stderr, sender)?;
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
    Ok((
        status,
        stdout.unwrap_or_default(),
        stderr.unwrap_or_default(),
    ))
}

/// Reads `pipe` to its end on a thread and sends the bytes.
fn drain(
    pipe: Option<impl Read + Send + 'static>,
    which: Pipe,
    sender: Sender<PipeResult>,
) -> Result<(), Failure> {
    let Some(mut pipe) = pipe else {
        // Not piped: there is nothing to read.
        let _ = sender.send((which, Ok(Vec::new())));
        return Ok(());
    };
    thread::Builder::new()
        .name("volee-git-pipe".to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.read_to_end(&mut bytes).map(|_| bytes);
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

/// Kills `child` and reaps it. Errors are ignored: the process may already
/// have exited, and there is nothing better to do.
fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
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
        let mut runner = Runner::new(PathBuf::from("git"), NonZeroUsize::MIN);
        runner.set_env("GIT_CONFIG_GLOBAL".into(), "/dev/null".into());
        runner
    }

    #[test]
    fn commands_target_the_directory_without_optional_locks() {
        let command = runner().command(Path::new("/work/volee"), &["status".into()]);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["-C", "/work/volee", "--no-optional-locks", "status"]);
    }

    #[test]
    fn commands_never_prompt_and_ignore_inherited_repositories() {
        let command = runner().command(Path::new("/work"), &[]);
        let envs: Vec<_> = command.get_envs().collect();
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
        let command = runner.command(Path::new("/work"), &[]);
        let values: Vec<_> = command
            .get_envs()
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
