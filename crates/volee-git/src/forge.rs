//! Pull requests, from the forge that hosts a repository: the [`Forge`]
//! trait, and [`GhCli`], which asks the GitHub CLI.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use volee_core::delivery::{ChecksState, PrState, PullRequest};

use crate::command::Runner;
use crate::error::GitError;
use crate::parse::ParseError;

/// The fields `gh pr list --json` is asked for.
const PR_FIELDS: &str = "number,url,state,isDraft,statusCheckRollup";

/// Where the pull requests of a repository come from.
///
/// Implementations reach the forge over the network: callers call them off
/// any request path, rate-limit them, and keep the last answer when one
/// fails. Tests use a fake implementation.
pub trait Forge: Send + Sync {
    /// Whether this forge client can be used, and its version. It may reach
    /// the network (to check the sign-in): call it once, in the background.
    fn status(&self) -> ForgeStatus;

    /// The most recent pull request opened from `branch` of the repository
    /// that contains `repo`, whatever its state; `None` when there is none.
    fn pull_request(&self, repo: &Path, branch: &str) -> Result<Option<PullRequest>, GitError>;
}

/// What is known about a forge client: see [`Forge::status`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeStatus {
    /// The program, such as `gh`.
    pub program: String,
    /// Whether volee can use it: installed and signed in.
    pub available: bool,
    /// Its version, when known.
    pub version: Option<String>,
    /// Why it is unavailable, or a note.
    pub detail: Option<String>,
}

/// Asks the GitHub CLI (`gh`) for pull requests.
///
/// Every command runs in the repository's directory, so `gh` finds the
/// GitHub repository from its remotes, with standard input closed, prompts
/// and update checks disabled, a timeout, and at most two at the same time.
/// `gh` uses the person's own sign-in (`gh auth login`); volee never sees a
/// token.
///
/// It talks to GitHub over the network: tests use a fake [`Forge`] instead.
#[derive(Debug)]
pub struct GhCli {
    runner: Runner,
    timeout: Duration,
}

impl GhCli {
    /// How long a command may run before it is stopped, unless set with
    /// [`GhCli::timeout`].
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

    /// Runs `gh` from the `PATH`.
    pub fn new() -> Self {
        Self::with_program("gh")
    }

    /// Runs the given `gh` executable.
    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        let limit = NonZeroUsize::new(2).expect("2 is not zero");
        let mut runner = Runner::in_directory(program.into(), limit);
        for (key, value) in [
            ("GH_PROMPT_DISABLED", "1"),
            ("GH_NO_UPDATE_NOTIFIER", "1"),
            ("NO_COLOR", "1"),
        ] {
            runner.set_env(key.into(), value.into());
        }
        Self {
            runner,
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Sets an environment variable for every `gh` command. It wins over the
    /// variables `GhCli` sets itself.
    #[must_use]
    pub fn env(
        mut self,
        key: impl Into<std::ffi::OsString>,
        value: impl Into<std::ffi::OsString>,
    ) -> Self {
        self.runner.set_env(key.into(), value.into());
        self
    }

    /// Sets how long a command may run before it is stopped.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Default for GhCli {
    fn default() -> Self {
        Self::new()
    }
}

impl Forge for GhCli {
    fn status(&self) -> ForgeStatus {
        let program = self.runner.program_name();
        let status = |available, version, detail| ForgeStatus {
            program: program.clone(),
            available,
            version,
            detail,
        };
        // Any directory does: these commands are about `gh` itself.
        let anywhere = Path::new("/");
        let version = match self
            .runner
            .request(anywhere, self.timeout)
            .arg("--version")
            .run()
        {
            Ok(completed) if completed.code == Some(0) => {
                gh_version(&String::from_utf8_lossy(&completed.stdout))
            }
            Ok(completed) => return status(false, None, Some(completed.failure().to_string())),
            Err(GitError::Spawn { .. }) => {
                return status(false, None, Some(format!("{program} is not installed")));
            }
            Err(error) => return status(false, None, Some(error.to_string())),
        };
        match self
            .runner
            .request(anywhere, self.timeout)
            .args(["auth", "status"])
            .run()
        {
            Ok(completed) if completed.code == Some(0) => status(true, version, None),
            Ok(_) => status(
                false,
                version,
                Some(format!("not signed in: run `{program} auth login`")),
            ),
            Err(error) => status(false, version, Some(error.to_string())),
        }
    }

    fn pull_request(&self, repo: &Path, branch: &str) -> Result<Option<PullRequest>, GitError> {
        self.runner
            .request(repo, self.timeout)
            .args(["pr", "list"])
            // `--head=` keeps a branch that starts with a dash from looking
            // like an option.
            .arg(format!("--head={branch}"))
            .args(["--state", "all", "--json", PR_FIELDS, "--limit", "1"])
            .parse(parse_pull_requests)
    }
}

/// A pull request as `gh pr list --json` prints it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u64,
    url: String,
    state: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    status_check_rollup: Option<Vec<GhCheck>>,
}

/// One check of a pull request: a check run (GitHub Actions and apps) or a
/// commit status (older integrations).
#[derive(Deserialize)]
struct GhCheck {
    #[serde(rename = "__typename", default)]
    kind: String,
    /// A check run's progress: `QUEUED`, `IN_PROGRESS`, `COMPLETED`...
    #[serde(default)]
    status: Option<String>,
    /// A completed check run's result: `SUCCESS`, `FAILURE`, `SKIPPED`...
    #[serde(default)]
    conclusion: Option<String>,
    /// A commit status's state: `SUCCESS`, `PENDING`, `FAILURE`...
    #[serde(default)]
    state: Option<String>,
}

/// How one check went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Passed,
    Pending,
    Failed,
}

/// Parses `gh pr list --json` with [`PR_FIELDS`]: the first pull request of
/// the list, if any.
fn parse_pull_requests(output: &[u8]) -> Result<Option<PullRequest>, ParseError> {
    let list: Vec<GhPullRequest> = serde_json::from_slice(output)
        .map_err(|error| ParseError::new(format!("not a list of pull requests: {error}")))?;
    let Some(found) = list.into_iter().next() else {
        return Ok(None);
    };
    Ok(Some(PullRequest {
        number: found.number,
        state: pr_state(&found.state, found.is_draft)?,
        checks: checks_state(found.status_check_rollup.as_deref().unwrap_or_default()),
        url: found.url,
    }))
}

fn pr_state(state: &str, is_draft: bool) -> Result<PrState, ParseError> {
    match state {
        "OPEN" if is_draft => Ok(PrState::Draft),
        "OPEN" => Ok(PrState::Open),
        "MERGED" => Ok(PrState::Merged),
        "CLOSED" => Ok(PrState::Closed),
        other => Err(ParseError::new(format!(
            "`{other}` is not a pull request state"
        ))),
    }
}

/// The combined state of a pull request's checks: failing as soon as one
/// failed, pending while one runs, passing when all passed (or were
/// skipped), none without checks.
fn checks_state(checks: &[GhCheck]) -> ChecksState {
    let outcomes: Vec<Outcome> = checks.iter().map(outcome).collect();
    if outcomes.is_empty() {
        ChecksState::None
    } else if outcomes.contains(&Outcome::Failed) {
        ChecksState::Failing
    } else if outcomes.contains(&Outcome::Pending) {
        ChecksState::Pending
    } else {
        ChecksState::Passing
    }
}

fn outcome(check: &GhCheck) -> Outcome {
    if check.kind == "StatusContext" {
        return match check.state.as_deref() {
            Some("SUCCESS") => Outcome::Passed,
            Some("FAILURE" | "ERROR") => Outcome::Failed,
            _ => Outcome::Pending,
        };
    }
    if check.status.as_deref() != Some("COMPLETED") {
        return Outcome::Pending;
    }
    match check.conclusion.as_deref() {
        Some("SUCCESS" | "NEUTRAL" | "SKIPPED") => Outcome::Passed,
        // A completed run without a conclusion is still settling.
        None => Outcome::Pending,
        Some(_) => Outcome::Failed,
    }
}

/// The version in `gh --version`: `2.103.3` in `gh version 2.103.3
/// (2026-10-26)`.
fn gh_version(output: &str) -> Option<String> {
    output
        .lines()
        .next()?
        .split_whitespace()
        .nth(2)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_run(status: &str, conclusion: Option<&str>) -> String {
        let conclusion = conclusion.map_or_else(|| "null".to_owned(), |c| format!("\"{c}\""));
        format!(
            r#"{{"__typename":"CheckRun","name":"CI","status":"{status}","conclusion":{conclusion}}}"#
        )
    }

    fn list(state: &str, draft: bool, checks: &[String]) -> Vec<u8> {
        format!(
            r#"[{{"number":12,"url":"https://github.com/volee-org/volee/pull/12","state":"{state}","isDraft":{draft},"statusCheckRollup":[{}]}}]"#,
            checks.join(",")
        )
        .into_bytes()
    }

    #[test]
    fn a_pull_request_is_read_from_gh() {
        let output = list("OPEN", false, &[check_run("COMPLETED", Some("SUCCESS"))]);
        assert_eq!(
            parse_pull_requests(&output).unwrap(),
            Some(PullRequest {
                number: 12,
                url: "https://github.com/volee-org/volee/pull/12".into(),
                state: PrState::Open,
                checks: ChecksState::Passing,
            })
        );
    }

    #[test]
    fn states_include_drafts() {
        let state = |state, draft| {
            parse_pull_requests(&list(state, draft, &[]))
                .unwrap()
                .unwrap()
                .state
        };
        assert_eq!(state("OPEN", true), PrState::Draft);
        assert_eq!(state("OPEN", false), PrState::Open);
        assert_eq!(state("MERGED", false), PrState::Merged);
        assert_eq!(state("CLOSED", false), PrState::Closed);
        assert!(parse_pull_requests(&list("REOPENED", false, &[])).is_err());
    }

    #[test]
    fn checks_combine_into_one_state() {
        let checks = |runs: &[String]| {
            parse_pull_requests(&list("OPEN", false, runs))
                .unwrap()
                .unwrap()
                .checks
        };
        let passed = check_run("COMPLETED", Some("SUCCESS"));
        let skipped = check_run("COMPLETED", Some("SKIPPED"));
        let running = check_run("IN_PROGRESS", None);
        let failed = check_run("COMPLETED", Some("FAILURE"));
        let status = |state: &str| format!(r#"{{"__typename":"StatusContext","state":"{state}"}}"#);
        assert_eq!(checks(&[]), ChecksState::None);
        assert_eq!(checks(&[passed.clone(), skipped]), ChecksState::Passing);
        assert_eq!(checks(&[passed.clone(), running]), ChecksState::Pending);
        assert_eq!(checks(&[passed.clone(), failed]), ChecksState::Failing);
        assert_eq!(checks(&[status("PENDING")]), ChecksState::Pending);
        assert_eq!(checks(&[status("ERROR"), passed]), ChecksState::Failing);
        assert_eq!(checks(&[status("SUCCESS")]), ChecksState::Passing);
    }

    #[test]
    fn no_pull_request_is_none_and_garbage_is_an_error() {
        assert_eq!(parse_pull_requests(b"[]").unwrap(), None);
        assert_eq!(
            parse_pull_requests(
                br#"[{"number":1,"url":"u","state":"OPEN","isDraft":false,"statusCheckRollup":null}]"#
            )
            .unwrap()
            .unwrap()
            .checks,
            ChecksState::None
        );
        assert!(parse_pull_requests(b"no pull requests").is_err());
    }

    #[test]
    fn the_version_is_read_from_the_first_line() {
        assert_eq!(
            gh_version(
                "gh version 2.103.3 (2026-10-26)\nhttps://github.com/cli/cli/releases/tag/v2.103.3\n"
            )
            .as_deref(),
            Some("2.103.3")
        );
        assert_eq!(gh_version(""), None);
    }
}
