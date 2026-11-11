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
use crate::remote::RepoSlug;

/// The fields `gh pr list --json` is asked for.
const PR_FIELDS: &str =
    "number,url,state,isDraft,statusCheckRollup,headRefOid,headRepository,headRepositoryOwner";

/// How many pull requests named after the branch are read, most recent
/// first, to find the one that is the branch's.
const CANDIDATES: usize = 10;

/// An agent's branch, as its pull request must show it: see
/// [`Forge::pull_request`].
#[derive(Clone, Copy, Debug)]
pub struct BranchHead<'a> {
    /// The branch's name.
    pub branch: &'a str,
    /// The repository the branch is pushed to (`origin`, see
    /// [`Git::origin`](crate::Git::origin)); `None` when it is not a
    /// repository on a forge, and then no pull request is the branch's.
    pub origin: Option<&'a RepoSlug>,
    /// The commit the branch points to.
    pub tip: &'a str,
}

/// Where the pull requests of a repository come from.
///
/// Implementations reach the forge over the network: callers call them off
/// any request path, rate-limit them, and keep the last answer when one
/// fails. Tests use a fake implementation.
pub trait Forge: Send + Sync {
    /// Whether this forge client can be used, and its version. It may reach
    /// the network (to check the sign-in): call it once, in the background.
    fn status(&self) -> ForgeStatus;

    /// The most recent pull request, whatever its state, opened from
    /// `head.branch` of `head.origin` and whose head commit is `head.tip`;
    /// `None` when there is none. A pull request with the same branch name
    /// from another repository (a fork) or at another commit (an old pull
    /// request of a reused branch name) is not the branch's.
    ///
    /// `repo` is any directory of the repository; the forge finds the
    /// repository that pull requests target from its remotes.
    fn pull_request(
        &self,
        repo: &Path,
        head: &BranchHead<'_>,
    ) -> Result<Option<PullRequest>, GitError>;

    /// Merges a pull request into its base branch, with `method`, or with
    /// the first method the repository allows when none is given, and
    /// returns the method used. A draft is marked ready for review first.
    ///
    /// This is what a person does on the forge once they agreed to the
    /// work: it is called only on their word. The forge's own rules still
    /// apply (required checks and reviews, a branch that is behind): a pull
    /// request it refuses to merge is an error that says why. The head
    /// branch is never deleted.
    fn merge(
        &self,
        repo: &Path,
        pull_request: &PullRequest,
        method: Option<MergeMethod>,
    ) -> Result<MergeMethod, GitError>;
}

/// How a pull request is merged into its base branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergeMethod {
    /// A merge commit: the branch's commits stay in the history.
    Merge,
    /// One commit that holds the whole change.
    Squash,
    /// The branch's commits, replayed on the base branch.
    Rebase,
}

impl MergeMethod {
    /// The methods, in the order volee tries them when none is asked for:
    /// a merge commit keeps what the agent committed.
    pub const ALL: [Self; 3] = [Self::Merge, Self::Squash, Self::Rebase];

    /// The word for it, as configuration files and `gh` flags spell it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
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
    /// The first merge method the repository of `pull_request` allows, in
    /// the order of [`MergeMethod::ALL`].
    fn first_allowed_method(
        &self,
        repo: &Path,
        pull_request: &PullRequest,
    ) -> Result<MergeMethod, GitError> {
        // The repository the pull request belongs to, from its address: the
        // remotes of a fork's checkout name two.
        let mut view = self
            .runner
            .request(repo, self.timeout)
            .args(["repo", "view"]);
        if let Some(repository) = repository_of(&pull_request.url) {
            view = view.arg(repository);
        }
        view.args(["--json", ALLOWED_FIELDS])
            .parse(parse_allowed_method)
    }

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

    fn pull_request(
        &self,
        repo: &Path,
        head: &BranchHead<'_>,
    ) -> Result<Option<PullRequest>, GitError> {
        if head.origin.is_none() {
            // Not pushed to a repository on a forge: nothing to ask.
            return Ok(None);
        }
        // `gh pr list --head` matches the branch name only, in every fork:
        // the candidates are filtered by repository and commit below.
        self.runner
            .request(repo, self.timeout)
            .args(["pr", "list"])
            // `--head=` keeps a branch that starts with a dash from looking
            // like an option.
            .arg(format!("--head={}", head.branch))
            .args(["--state", "all", "--json", PR_FIELDS, "--limit"])
            .arg(CANDIDATES.to_string())
            .parse(|output| parse_pull_requests(output, head))
    }

    fn merge(
        &self,
        repo: &Path,
        pull_request: &PullRequest,
        method: Option<MergeMethod>,
    ) -> Result<MergeMethod, GitError> {
        let method = match method {
            Some(method) => method,
            None => self.first_allowed_method(repo, pull_request)?,
        };
        if pull_request.state == PrState::Draft {
            self.runner
                .request(repo, self.timeout)
                .args(["pr", "ready"])
                .arg(&pull_request.url)
                .run()?
                .success()?;
        }
        self.runner
            .request(repo, self.timeout)
            .args(["pr", "merge"])
            .arg(&pull_request.url)
            .arg(format!("--{}", method.name()))
            .run()?
            .success()?;
        Ok(method)
    }
}

/// The repository of a pull request, as `gh` names one (`HOST/OWNER/REPO`),
/// from the pull request's address (`https://HOST/OWNER/REPO/pull/7`).
fn repository_of(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let mut parts = path.split('/');
    let (host, owner, name) = (parts.next()?, parts.next()?, parts.next()?);
    let plain = |part: &str| {
        !part.is_empty()
            && !part.starts_with('-')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    };
    (parts.next() == Some("pull") && plain(host) && plain(owner) && plain(name))
        .then(|| format!("{host}/{owner}/{name}"))
}

/// The fields `gh repo view --json` is asked for, to know how the
/// repository lets pull requests be merged.
const ALLOWED_FIELDS: &str = "mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed";

/// The merge methods of a repository, as `gh repo view --json` prints them.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhMergeMethods {
    #[serde(default)]
    merge_commit_allowed: bool,
    #[serde(default)]
    squash_merge_allowed: bool,
    #[serde(default)]
    rebase_merge_allowed: bool,
}

/// The first merge method a repository allows, in the order of
/// [`MergeMethod::ALL`].
fn parse_allowed_method(output: &[u8]) -> Result<MergeMethod, ParseError> {
    let allowed: GhMergeMethods = serde_json::from_slice(output)
        .map_err(|error| ParseError::new(format!("not the JSON of a repository: {error}")))?;
    MergeMethod::ALL
        .into_iter()
        .find(|method| match method {
            MergeMethod::Merge => allowed.merge_commit_allowed,
            MergeMethod::Squash => allowed.squash_merge_allowed,
            MergeMethod::Rebase => allowed.rebase_merge_allowed,
        })
        .ok_or_else(|| ParseError::new("the repository allows no merge method"))
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
    /// The commit at the head of the pull request.
    #[serde(default)]
    head_ref_oid: Option<String>,
    /// The repository the head branch lives in; `null` once it is deleted.
    #[serde(default)]
    head_repository: Option<GhRepository>,
    /// Its owner.
    #[serde(default)]
    head_repository_owner: Option<GhOwner>,
}

impl GhPullRequest {
    /// Whether this pull request is the branch's: opened from its
    /// repository, at its tip.
    fn is_from(&self, head: &BranchHead<'_>) -> bool {
        let from_origin = match (
            head.origin,
            &self.head_repository_owner,
            &self.head_repository,
        ) {
            (Some(origin), Some(owner), Some(repository)) => {
                origin.is(&owner.login, &repository.name)
            }
            _ => false,
        };
        from_origin && self.head_ref_oid.as_deref() == Some(head.tip)
    }
}

/// A repository in `gh` output.
#[derive(Deserialize)]
struct GhRepository {
    name: String,
}

/// A repository owner in `gh` output.
#[derive(Deserialize)]
struct GhOwner {
    login: String,
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

/// Parses `gh pr list --json` with [`PR_FIELDS`], most recent first: the
/// first pull request that is the branch's (see [`Forge::pull_request`]),
/// if any.
fn parse_pull_requests(
    output: &[u8],
    head: &BranchHead<'_>,
) -> Result<Option<PullRequest>, ParseError> {
    let list: Vec<GhPullRequest> = serde_json::from_slice(output)
        .map_err(|error| ParseError::new(format!("not a list of pull requests: {error}")))?;
    let Some(found) = list.into_iter().find(|candidate| candidate.is_from(head)) else {
        return Ok(None);
    };
    Ok(Some(PullRequest {
        number: found.number,
        state: pr_state(&found.state, found.is_draft)?,
        checks: checks_state(found.status_check_rollup.as_deref().unwrap_or_default()),
        url: found.url,
        head: found.head_ref_oid,
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

    #[test]
    fn a_pull_request_address_names_its_repository() {
        assert_eq!(
            repository_of("https://github.com/volee-org/volee/pull/7").as_deref(),
            Some("github.com/volee-org/volee")
        );
        assert_eq!(
            repository_of("https://git.example.com:8443/team/tool.rs/pull/12").as_deref(),
            Some("git.example.com:8443/team/tool.rs")
        );
        // Anything else is left to `gh`, which reads the checkout's remotes.
        for other in [
            "",
            "github.com/volee-org/volee/pull/7",
            "https://github.com/volee-org/volee",
            "https://github.com/volee-org/volee/issues/7",
            "https://github.com/--web/volee/pull/7",
            "https://github.com/volee org/volee/pull/7",
            "https://github.com//volee/pull/7",
        ] {
            assert_eq!(repository_of(other), None, "{other}");
        }
    }

    fn check_run(status: &str, conclusion: Option<&str>) -> String {
        let conclusion = conclusion.map_or_else(|| "null".to_owned(), |c| format!("\"{c}\""));
        format!(
            r#"{{"__typename":"CheckRun","name":"CI","status":"{status}","conclusion":{conclusion}}}"#
        )
    }

    /// The tip of the branch in these tests, `feat/x` of `volee-org/volee`.
    const TIP: &str = "6adc11fa9619748617663ded520ce631e9292269";

    /// Parses `gh` output for that branch.
    fn parse(output: &[u8]) -> Result<Option<PullRequest>, ParseError> {
        let origin = RepoSlug {
            owner: "volee-org".into(),
            name: "volee".into(),
        };
        let head = BranchHead {
            branch: "feat/x",
            origin: Some(&origin),
            tip: TIP,
        };
        parse_pull_requests(output, &head)
    }

    /// The fields that say where a pull request comes from.
    fn head_fields(owner: &str, oid: &str) -> String {
        format!(
            r#""headRefOid":"{oid}","headRepository":{{"id":"R_1","name":"volee"}},"headRepositoryOwner":{{"id":"O_1","login":"{owner}"}}"#
        )
    }

    /// One pull request, opened from `owner/volee` at `oid`.
    fn candidate(number: u64, state: &str, owner: &str, oid: &str) -> String {
        format!(
            r#"{{"number":{number},"url":"https://github.com/volee-org/volee/pull/{number}","state":"{state}","isDraft":false,"statusCheckRollup":[],{}}}"#,
            head_fields(owner, oid)
        )
    }

    /// The branch's own pull request, alone in the list.
    fn list(state: &str, draft: bool, checks: &[String]) -> Vec<u8> {
        format!(
            r#"[{{"number":12,"url":"https://github.com/volee-org/volee/pull/12","state":"{state}","isDraft":{draft},"statusCheckRollup":[{}],{}}}]"#,
            checks.join(","),
            head_fields("volee-org", TIP)
        )
        .into_bytes()
    }

    #[test]
    fn a_pull_request_is_read_from_gh() {
        let output = list("OPEN", false, &[check_run("COMPLETED", Some("SUCCESS"))]);
        assert_eq!(
            parse(&output).unwrap(),
            Some(PullRequest {
                number: 12,
                url: "https://github.com/volee-org/volee/pull/12".into(),
                state: PrState::Open,
                checks: ChecksState::Passing,
                head: Some(TIP.into()),
            })
        );
    }

    #[test]
    fn only_a_pull_request_from_origin_at_the_tip_is_the_branch_s() {
        let old = "0dd5ed0dd5ed0dd5ed0dd5ed0dd5ed0dd5ed0dd5";
        let found = |candidates: &[String]| {
            parse(format!("[{}]", candidates.join(",")).as_bytes())
                .unwrap()
                .map(|pr| pr.number)
        };
        // A reused branch name: an old pull request, merged long ago.
        assert_eq!(found(&[candidate(3, "MERGED", "volee-org", old)]), None);
        // A fork with the same branch name, even at the same commit.
        assert_eq!(found(&[candidate(9, "OPEN", "someone", TIP)]), None);
        // The branch's own pull request, behind a newer one from a fork and
        // ahead of the old one; owners compare without case.
        assert_eq!(
            found(&[
                candidate(11, "OPEN", "someone", old),
                candidate(10, "OPEN", "Volee-Org", TIP),
                candidate(3, "MERGED", "volee-org", old),
            ]),
            Some(10)
        );
    }

    #[test]
    fn a_pull_request_whose_head_repository_is_gone_is_not_the_branch_s() {
        let output = format!(
            r#"[{{"number":5,"url":"u","state":"CLOSED","isDraft":false,"statusCheckRollup":[],"headRefOid":"{TIP}","headRepository":null,"headRepositoryOwner":null}}]"#
        );
        assert_eq!(parse(output.as_bytes()).unwrap(), None);
    }

    #[test]
    fn states_include_drafts() {
        let state = |state, draft| parse(&list(state, draft, &[])).unwrap().unwrap().state;
        assert_eq!(state("OPEN", true), PrState::Draft);
        assert_eq!(state("OPEN", false), PrState::Open);
        assert_eq!(state("MERGED", false), PrState::Merged);
        assert_eq!(state("CLOSED", false), PrState::Closed);
        assert!(parse(&list("REOPENED", false, &[])).is_err());
    }

    #[test]
    fn checks_combine_into_one_state() {
        let checks = |runs: &[String]| parse(&list("OPEN", false, runs)).unwrap().unwrap().checks;
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
        assert_eq!(parse(b"[]").unwrap(), None);
        let without_checks = format!(
            r#"[{{"number":1,"url":"u","state":"OPEN","isDraft":false,"statusCheckRollup":null,{}}}]"#,
            head_fields("volee-org", TIP)
        );
        assert_eq!(
            parse(without_checks.as_bytes()).unwrap().unwrap().checks,
            ChecksState::None
        );
        // Output of a `gh` too old to print heads: no pull request is proven
        // to be the branch's.
        assert_eq!(
            parse(br#"[{"number":1,"url":"u","state":"MERGED","isDraft":false}]"#).unwrap(),
            None
        );
        assert!(parse(b"no pull requests").is_err());
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
