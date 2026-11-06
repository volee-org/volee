//! Running programs: the only place where this crate starts processes.
//!
//! Adapters describe what to run as an [`Invocation`] and hand it to a
//! [`CommandRunner`]. [`SystemRunner`] runs real processes; tests use a fake
//! that records invocations and replays fixtures, so they never need the real
//! harness.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Runs programs to completion.
///
/// Implementations must be usable from several threads at once: the daemon
/// shares one runner between its reconcilers.
pub trait CommandRunner: fmt::Debug + Send + Sync {
    /// Runs `invocation` until it exits or its timeout expires, and returns
    /// what it printed.
    ///
    /// A program that exits with a failure status is not an error here: the
    /// caller decides what the status means.
    fn run(&self, invocation: &Invocation) -> Result<Output, RunError>;
}

/// A program to run and everything it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    /// The program: a name looked up in the `PATH`, or a path.
    pub program: PathBuf,
    /// Its arguments, passed as they are (no shell).
    pub args: Vec<OsString>,
    /// The working directory; the caller's when `None`.
    pub cwd: Option<PathBuf>,
    /// Environment variables set on top of the caller's environment.
    pub env: Vec<(OsString, OsString)>,
    /// Bytes written to the program's standard input, which is closed
    /// afterwards. Without them, standard input is empty.
    pub stdin: Option<Vec<u8>>,
    /// How long the program may run before it is killed.
    pub timeout: Duration,
}

impl Invocation {
    /// An invocation of `program` with no arguments, in the caller's
    /// directory, with empty standard input.
    pub fn new(program: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            stdin: None,
            timeout,
        }
    }

    /// Adds one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Adds several arguments.
    #[must_use]
    pub fn args<I>(mut self, args: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Runs the program in `cwd`.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Sets an environment variable for the program.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Writes `bytes` to the program's standard input.
    #[must_use]
    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    /// The arguments as text, for logs and assertions. Arguments that are
    /// not valid UTF-8 are converted lossily.
    pub fn args_lossy(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }
}

/// What a finished program printed, and how it exited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    /// The exit code, or `None` when a signal ended the program.
    pub code: Option<i32>,
    /// Everything written to standard output.
    pub stdout: Vec<u8>,
    /// Everything written to standard error.
    pub stderr: Vec<u8>,
}

impl Output {
    /// A successful exit (code 0) that printed `stdout`.
    pub fn success(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            code: Some(0),
            stdout: stdout.into(),
            stderr: Vec::new(),
        }
    }

    /// A failed exit with `code` that printed `stderr`.
    pub fn failure(code: i32, stderr: impl Into<Vec<u8>>) -> Self {
        Self {
            code: Some(code),
            stdout: Vec::new(),
            stderr: stderr.into(),
        }
    }

    /// Whether the program exited with code 0.
    pub fn is_success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Why a program could not run to completion.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The program does not exist or is not on the `PATH`.
    #[error("`{}` was not found", program.display())]
    NotFound {
        /// The program.
        program: PathBuf,
    },
    /// The working directory does not exist.
    #[error("the working directory {} does not exist", cwd.display())]
    NoSuchDirectory {
        /// The directory.
        cwd: PathBuf,
    },
    /// The program ran longer than its timeout and was killed.
    #[error("`{}` did not finish within {timeout:?} and was killed", program.display())]
    TimedOut {
        /// The program.
        program: PathBuf,
        /// The timeout it exceeded.
        timeout: Duration,
    },
    /// The operating system refused to run or watch the program.
    #[error("could not run `{}`", program.display())]
    Io {
        /// The program.
        program: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

/// Runs real processes with [`std::process`].
///
/// Standard output and standard error are read on helper threads while the
/// program runs, so a chatty program never blocks on a full pipe. When the
/// program exits but a process it started in the background keeps the pipes
/// open (as a daemonizing launcher may), the runner returns what it read so
/// far after a short grace period instead of waiting for that process.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRunner;

/// How long to keep reading after the program exited while a process it
/// started still holds its output pipes.
const PIPE_GRACE: Duration = Duration::from_millis(200);

/// The longest pause between two checks of a running program.
const MAX_PAUSE: Duration = Duration::from_millis(20);

impl CommandRunner for SystemRunner {
    fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
        if let Some(cwd) = &invocation.cwd
            && !cwd.is_dir()
        {
            return Err(RunError::NoSuchDirectory { cwd: cwd.clone() });
        }
        let io_error = |source| RunError::Io {
            program: invocation.program.clone(),
            source,
        };

        let mut command = Command::new(&invocation.program);
        command
            .args(&invocation.args)
            .stdin(if invocation.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &invocation.env {
            command.env(key, value);
        }
        if let Some(cwd) = &invocation.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                RunError::NotFound {
                    program: invocation.program.clone(),
                }
            } else {
                io_error(source)
            }
        })?;

        match start_helpers(&mut child, invocation) {
            Ok(receiver) => collect(&mut child, &receiver, invocation),
            Err(source) => {
                kill(&mut child);
                Err(io_error(source))
            }
        }
    }
}

/// Starts the threads that feed the program's input and read its output,
/// and returns the receiving end of its output.
fn start_helpers(child: &mut Child, invocation: &Invocation) -> io::Result<Receiver<Chunk>> {
    if let (Some(mut pipe), Some(bytes)) = (child.stdin.take(), invocation.stdin.clone()) {
        spawn_helper(&invocation.program, "stdin", move || {
            // A program may exit without reading all of its input; the
            // broken pipe that follows is not an error.
            if let Err(error) = pipe.write_all(&bytes)
                && error.kind() != io::ErrorKind::BrokenPipe
            {
                tracing::debug!(%error, "could not write a program's standard input");
            }
        })?;
    }
    let (sender, receiver) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        pump(&invocation.program, stdout, Stream::Stdout, sender.clone())?;
    }
    if let Some(stderr) = child.stderr.take() {
        pump(&invocation.program, stderr, Stream::Stderr, sender)?;
    }
    Ok(receiver)
}

/// Which output stream a chunk came from.
#[derive(Clone, Copy, Debug)]
enum Stream {
    Stdout,
    Stderr,
}

type Chunk = (Stream, Vec<u8>);

/// Starts a named helper thread. It is detached: it ends on its own when its
/// pipe closes.
fn spawn_helper(
    program: &Path,
    role: &str,
    work: impl FnOnce() + Send + 'static,
) -> io::Result<()> {
    let name = format!("volee-run-{role}-{}", file_name(program));
    thread::Builder::new().name(name).spawn(work).map(drop)
}

fn file_name(program: &Path) -> String {
    program
        .file_name()
        .unwrap_or_else(|| OsStr::new("program"))
        .to_string_lossy()
        .into_owned()
}

/// Forwards everything read from `source` to `sink`, chunk by chunk, until
/// the pipe closes or nobody listens any more.
fn pump(
    program: &Path,
    mut source: impl Read + Send + 'static,
    stream: Stream,
    sink: Sender<Chunk>,
) -> io::Result<()> {
    let role = match stream {
        Stream::Stdout => "stdout",
        Stream::Stderr => "stderr",
    };
    spawn_helper(program, role, move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match source.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    if sink.send((stream, buffer[..read].to_vec())).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    tracing::debug!(%error, "stopped reading a program's output");
                    break;
                }
            }
        }
    })
}

/// Waits for the program to exit, gathering its output, and kills it when it
/// runs past its timeout.
fn collect(
    child: &mut Child,
    receiver: &Receiver<Chunk>,
    invocation: &Invocation,
) -> Result<Output, RunError> {
    let started = Instant::now();
    let mut output = Output::default();
    let mut exited: Option<Instant> = None;
    let mut pipes_open = true;
    let mut pause = Duration::from_millis(1);
    loop {
        if exited.is_none() {
            let status = child.try_wait().map_err(|source| RunError::Io {
                program: invocation.program.clone(),
                source,
            })?;
            if let Some(status) = status {
                output.code = status.code();
                exited = Some(Instant::now());
            }
        }
        match exited {
            Some(at) if !pipes_open || at.elapsed() >= PIPE_GRACE => {
                if pipes_open {
                    tracing::debug!(
                        program = %invocation.program.display(),
                        "the program exited but a process it started keeps its output open"
                    );
                }
                break;
            }
            None if started.elapsed() >= invocation.timeout => {
                kill(child);
                return Err(RunError::TimedOut {
                    program: invocation.program.clone(),
                    timeout: invocation.timeout,
                });
            }
            _ => {}
        }
        if pipes_open {
            match receiver.recv_timeout(pause) {
                Ok(chunk) => {
                    append(&mut output, chunk);
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => pipes_open = false,
            }
        } else {
            thread::sleep(pause);
        }
        pause = (pause * 2).min(MAX_PAUSE);
    }
    while let Ok(chunk) = receiver.try_recv() {
        append(&mut output, chunk);
    }
    Ok(output)
}

fn append(output: &mut Output, (stream, bytes): Chunk) {
    match stream {
        Stream::Stdout => output.stdout.extend_from_slice(&bytes),
        Stream::Stderr => output.stderr.extend_from_slice(&bytes),
    }
}

/// Kills and reaps a program that ran too long.
fn kill(child: &mut Child) {
    // The program may exit on its own between the timeout check and here;
    // failing to kill it then is expected.
    if let Err(error) = child.kill() {
        tracing::debug!(%error, "could not kill a program that timed out");
    }
    if let Err(error) = child.wait() {
        tracing::debug!(%error, "could not reap a program that timed out");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Invocation {
        Invocation::new("sh", Duration::from_secs(10)).args(["-c", script])
    }

    #[test]
    fn captures_both_streams_and_the_exit_code() {
        let output = SystemRunner
            .run(&sh("printf out; printf err >&2; exit 3"))
            .unwrap();
        assert_eq!(output.code, Some(3));
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
        assert!(!output.is_success());
    }

    #[test]
    fn feeds_standard_input() {
        let output = SystemRunner.run(&sh("cat").stdin("hello")).unwrap();
        assert!(output.is_success());
        assert_eq!(output.stdout, b"hello");
    }

    #[test]
    fn standard_input_is_empty_by_default() {
        let output = SystemRunner.run(&sh("wc -c < /dev/stdin")).unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0");
    }

    #[test]
    fn runs_in_the_working_directory() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let output = SystemRunner.run(&sh("pwd -P").cwd(&dir)).unwrap();
        let printed = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        assert_eq!(printed, dir);
    }

    #[test]
    fn sets_environment_variables() {
        let output = SystemRunner
            .run(&sh("printf %s \"$VOLEE_RUNNER_TEST\"").env("VOLEE_RUNNER_TEST", "set"))
            .unwrap();
        assert_eq!(output.stdout, b"set");
    }

    #[test]
    fn reads_large_outputs_without_blocking() {
        let output = SystemRunner
            .run(&sh(
                "head -c 1048576 /dev/zero; head -c 65536 /dev/zero >&2",
            ))
            .unwrap();
        assert!(output.is_success());
        assert_eq!(output.stdout.len(), 1_048_576);
        assert_eq!(output.stderr.len(), 65_536);
    }

    #[test]
    fn reports_a_missing_program() {
        let invocation = Invocation::new("volee-no-such-program", Duration::from_secs(5));
        let error = SystemRunner.run(&invocation).unwrap_err();
        assert!(matches!(error, RunError::NotFound { .. }), "{error:?}");
    }

    #[test]
    fn reports_a_missing_directory() {
        let error = SystemRunner
            .run(&sh("true").cwd("/volee/no/such/directory"))
            .unwrap_err();
        assert!(
            matches!(error, RunError::NoSuchDirectory { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn kills_a_program_that_runs_too_long() {
        let mut invocation = sh("sleep 30");
        invocation.timeout = Duration::from_millis(200);
        let started = Instant::now();
        let error = SystemRunner.run(&invocation).unwrap_err();
        assert!(matches!(error, RunError::TimedOut { .. }), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn returns_when_a_background_process_keeps_the_pipes_open() {
        let started = Instant::now();
        let output = SystemRunner.run(&sh("sleep 3 & printf started")).unwrap();
        assert!(output.is_success());
        assert_eq!(output.stdout, b"started");
        assert!(
            started.elapsed() < Duration::from_millis(2_500),
            "waited {:?} for a background process",
            started.elapsed()
        );
    }

    #[test]
    fn builders_fill_every_field() {
        let invocation = Invocation::new("claude", Duration::from_secs(1))
            .arg("agents")
            .args(["--json", "--all"])
            .cwd("/work")
            .env("KEY", "value")
            .stdin("input");
        assert_eq!(invocation.args_lossy(), ["agents", "--json", "--all"]);
        assert_eq!(invocation.cwd.as_deref(), Some(Path::new("/work")));
        assert_eq!(invocation.env, [("KEY".into(), "value".into())]);
        assert_eq!(invocation.stdin.as_deref(), Some(&b"input"[..]));
    }
}
