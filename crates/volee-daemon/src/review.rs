//! The AI pre-review of finished work (v2, step 7).
//!
//! When an agent has finished work (commits its base does not have), the
//! daemon asks Claude Code's native `/code-review` for an **opinion** on the
//! branch, in the agent's worktree, read-only, with a small model. The
//! opinion is stored (one review per `(repo, branch, tip)`, never twice) and
//! served as findings; it never gates anything by itself.
//!
//! # What `/code-review` gives
//!
//! Measured on Claude Code 2.1.330 (see `docs/v2/pre-review.md`):
//! `claude -p "/code-review <low|medium|...> <base>...<tip>"` prints the
//! findings as **text**, not as typed data: there is no `ReportFindings`
//! event in `-p`, and `--json-schema` does not apply to a slash command. The
//! text layout varies with the effort level, the model and the language, so
//! [`parse_findings`] is tolerant and the raw text is always kept.
//!
//! # No loop, no noise
//!
//! The review session runs with every hook disabled (`disableAllHooks`), so
//! it posts nothing to the daemon, and it carries the name prefix
//! [`SESSION_PREFIX`] and the environment variable [`ENV_MARKER`], so that
//! the agents view never shows it as an agent.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, watch};
use volee_proto::methods::AgentCard;
use volee_rules::{Change, FileChange, Rules};
use volee_store::{NewFinding, NewReview, Store, StoredReview, review_status};

use crate::agents_view::Agents;

/// The name prefix of a review session. The agents view ignores sessions
/// whose name starts with it.
pub(crate) const SESSION_PREFIX: &str = "volee-review";

/// The environment variable set to `1` in a review session.
pub(crate) const ENV_MARKER: &str = "VOLEE_REVIEW";

/// Whether a session is one of the daemon's own reviews.
pub(crate) fn is_review_session(name: Option<&str>) -> bool {
    name.is_some_and(|name| name.starts_with(SESSION_PREFIX))
}

/// What a reviewer is asked.
#[derive(Clone, Debug)]
pub(crate) struct ReviewRequest {
    /// The worktree to run in.
    pub(crate) cwd: PathBuf,
    /// The branch the change goes into.
    pub(crate) base: String,
    /// The commit to review.
    pub(crate) tip: String,
    /// The `/code-review` effort level: `low` or `medium`.
    pub(crate) level: &'static str,
    /// The model name.
    pub(crate) model: String,
    /// The session name, which carries [`SESSION_PREFIX`].
    pub(crate) session_name: String,
}

/// What a reviewer said.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReviewOutput {
    /// The text of the review.
    pub(crate) text: String,
    /// What it cost, when known.
    pub(crate) cost_usd: Option<f64>,
}

/// Something that reviews a change. Claude Code in production, a fake in
/// tests.
pub(crate) trait Reviewer: Send + Sync {
    /// Runs one reviewer to the end, or fails with a reason.
    fn review(&self, request: &ReviewRequest) -> Result<ReviewOutput, String>;
}

/// Claude Code's `/code-review`, run as a child process.
pub(crate) struct ClaudeReviewer {
    command: String,
    timeout: Duration,
}

impl ClaudeReviewer {
    pub(crate) fn new(command: String, timeout: Duration) -> Self {
        Self { command, timeout }
    }

    /// The arguments of one run. Read-only: `plan` mode and the edit tools
    /// removed, never `--fix`. No hooks, so the run posts nothing to the
    /// daemon.
    pub(crate) fn args(request: &ReviewRequest) -> Vec<String> {
        vec![
            "-p".to_owned(),
            format!(
                "/code-review {} {}...{}",
                request.level, request.base, request.tip
            ),
            "--model".to_owned(),
            request.model.clone(),
            "--permission-mode".to_owned(),
            "plan".to_owned(),
            "--disallowedTools".to_owned(),
            "Edit,Write,NotebookEdit".to_owned(),
            "--name".to_owned(),
            request.session_name.clone(),
            "--settings".to_owned(),
            r#"{"disableAllHooks":true}"#.to_owned(),
            "--output-format".to_owned(),
            "json".to_owned(),
        ]
    }
}

impl Reviewer for ClaudeReviewer {
    fn review(&self, request: &ReviewRequest) -> Result<ReviewOutput, String> {
        let mut child = Command::new(&self.command)
            .args(Self::args(request))
            .current_dir(&request.cwd)
            .env(ENV_MARKER, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot run {}: {error}", self.command))?;
        let out = drain(child.stdout.take());
        let err = drain(child.stderr.take());
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "the review took more than {} s and was stopped",
                        self.timeout.as_secs()
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => return Err(format!("cannot wait for the review: {error}")),
            }
        };
        let stdout = out.join().unwrap_or_default();
        let stderr = err.join().unwrap_or_default();
        if !status.success() {
            return Err(format!(
                "{} exited with {status}: {}",
                self.command,
                stderr.trim()
            ));
        }
        parse_output(&stdout)
    }
}

fn drain<R: Read + Send + 'static>(stream: Option<R>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stream {
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes);
            text = String::from_utf8_lossy(&bytes).into_owned();
        }
        text
    })
}

/// Reads `claude -p --output-format json`: the text is `result`.
fn parse_output(stdout: &str) -> Result<ReviewOutput, String> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|error| format!("the review did not answer with JSON: {error}"))?;
    let text = value
        .get("result")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if value
        .get("is_error")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Err(format!("the review failed: {}", text.trim()));
    }
    Ok(ReviewOutput {
        text,
        cost_usd: value
            .get("total_cost_usd")
            .and_then(serde_json::Value::as_f64),
    })
}

// ---------------------------------------------------------------------------
// Findings

/// Turns the text of a review into findings. Tolerant: a finding is a line
/// that starts (within its first words) with `path:line`, whatever the list
/// marker, emoji, backticks or separator around it. Text that has no such
/// line gives no finding; the raw text is kept by the caller.
pub(crate) fn parse_findings(text: &str, cwd: &Path) -> Vec<NewFinding> {
    let lines: Vec<&str> = text.lines().collect();
    let starts: Vec<(usize, String, Option<u32>, &str)> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            locate(line).map(|(file, number, rest)| (index, file, number, rest))
        })
        .collect();
    let canonical = cwd.canonicalize().ok();
    let mut findings = Vec::new();
    for (position, (index, file, number, rest)) in starts.iter().enumerate() {
        let end = starts
            .get(position + 1)
            .map_or(lines.len(), |next| next.0)
            .max(index + 1);
        let lowered = lines[*index].to_lowercase();
        let (summary, mut scenario) = split_scenario(rest);
        let mut summary = summary.to_owned();
        if scenario.is_none() {
            scenario = lines[index + 1..end].iter().find_map(|line| {
                let trimmed = line.trim();
                scenario_after(trimmed).map(str::to_owned)
            });
        }
        summary = clean_summary(&summary);
        if summary.is_empty() {
            continue;
        }
        findings.push(NewFinding {
            file: relative(file, cwd, canonical.as_deref()),
            line: *number,
            severity: severity_of(&lowered).to_owned(),
            summary,
            failure_scenario: scenario.filter(|text| !text.is_empty()),
        });
    }
    findings
}

/// The file, the line and the rest of a line that opens a finding.
fn locate(line: &str) -> Option<(String, Option<u32>, &str)> {
    let mut offset = 0;
    for (index, word) in line.split(' ').enumerate() {
        let end = offset + word.len();
        if index > 6 {
            break;
        }
        if let Some((file, number)) = path_and_line(word) {
            return Some((file, number, &line[end..]));
        }
        offset = end + 1;
    }
    None
}

fn path_and_line(word: &str) -> Option<(String, Option<u32>)> {
    let trimmed = word.trim_matches(|c: char| "`()[]*,:;".contains(c));
    let (file, tail) = trimmed.rsplit_once(':')?;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || file.is_empty() || file.contains(char::is_whitespace) {
        return None;
    }
    let name = file.rsplit('/').next().unwrap_or(file);
    let (_, extension) = name.rsplit_once('.')?;
    if !extension.chars().next().is_some_and(char::is_alphabetic) {
        return None;
    }
    Some((file.to_owned(), digits.parse().ok()))
}

fn scenario_after(text: &str) -> Option<&str> {
    let lowered = text.to_lowercase();
    let at = lowered.find("failure scenario")?;
    let after = &text[at + "failure scenario".len()..];
    Some(after.trim_start_matches([':', ' ', '*']).trim())
}

fn split_scenario(rest: &str) -> (&str, Option<String>) {
    let lowered = rest.to_lowercase();
    match lowered.find("failure scenario") {
        Some(at) => (
            rest[..at].trim_end(),
            scenario_after(&rest[at..]).map(str::to_owned),
        ),
        None => (rest, None),
    }
}

fn clean_summary(text: &str) -> String {
    const SEPARATORS: &[char] = &[' ', ':', '\u{2014}', '\u{2013}', '-', '*'];
    let mut text = text.trim_start_matches(SEPARATORS);
    for _ in 0..3 {
        let lowered = text.to_lowercase();
        let Some(word) = ["important", "pre-existing", "pre_existing", "nit"]
            .into_iter()
            .find(|word| lowered.starts_with(word))
        else {
            break;
        };
        text = text[word.len()..].trim_start_matches(SEPARATORS);
    }
    text.trim().to_owned()
}

fn severity_of(lowered_line: &str) -> &'static str {
    let words: Vec<&str> = lowered_line
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .collect();
    if lowered_line.contains("pre-existing")
        || lowered_line.contains("preexisting")
        || words.contains(&"pre_existing")
    {
        "pre_existing"
    } else if words.contains(&"nit") && !words.contains(&"important") {
        "nit"
    } else {
        "important"
    }
}

fn relative(file: &str, cwd: &Path, canonical: Option<&Path>) -> String {
    let path = Path::new(file);
    let stripped = path
        .strip_prefix(cwd)
        .ok()
        .or_else(|| canonical.and_then(|canonical| path.strip_prefix(canonical).ok()));
    stripped
        .map_or(file, |stripped| stripped.to_str().unwrap_or(file))
        .trim_start_matches("./")
        .to_owned()
}

/// Merges the findings of several reviewers: the same file and line once.
pub(crate) fn merge_findings(groups: Vec<Vec<NewFinding>>) -> Vec<NewFinding> {
    let mut seen: HashSet<(String, Option<u32>)> = HashSet::new();
    groups
        .into_iter()
        .flatten()
        .filter(|finding| seen.insert((finding.file.clone(), finding.line)))
        .collect()
}

// ---------------------------------------------------------------------------
// Planning

/// How a change is reviewed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The model name.
    pub(crate) model: String,
    /// How many reviewers; 0 skips the review.
    pub(crate) reviewers: u32,
}

/// The reviewer's `/code-review` levels, in the order they run. One reviewer
/// is a `low` pass (few, confident findings); the second, for a risky
/// change, is a `medium` pass, which reads the diff from more angles.
const LEVELS: [&str; 2] = ["low", "medium"];

/// The facts of a change for the rules, read from git.
pub(crate) fn change_of(target: &Target, tip: &str) -> Change {
    let numstat = git(
        &target.cwd,
        &["diff", "--numstat", &format!("{}...{tip}", target.base)],
    )
    .unwrap_or_default();
    let files: Vec<FileChange> = numstat
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let added = parts.next()?.parse().unwrap_or(0);
            let removed = parts.next()?.parse().unwrap_or(0);
            Some(FileChange {
                path: parts.next()?.to_owned(),
                added,
                removed,
            })
        })
        .collect();
    let commits = git(
        &target.cwd,
        &["rev-list", "--count", &format!("{}..{tip}", target.base)],
    )
    .and_then(|count| count.trim().parse().ok())
    .unwrap_or(0);
    let lower = |file: &FileChange| file.path.to_lowercase();
    Change {
        repo: target.repo_name.clone(),
        branch: target.branch.clone(),
        base: target.base.clone(),
        tests_touched: files
            .iter()
            .any(|file| lower(file).contains("test") || lower(file).contains("spec")),
        migrations: files.iter().any(|file| lower(file).contains("migration")),
        files,
        commits,
        agent: "claude".to_owned(),
        developer: std::env::var("USER").unwrap_or_default(),
    }
}

/// Asks the Lua rules how to review `change`: the model and the number of
/// reviewers (more when the risk is high). Rules that fail give the safest
/// plan, as `volee-rules` decides.
pub(crate) fn plan_for(change: &Change, repo_root: &Path, config_dir: &Path) -> Plan {
    let rules = Rules::load(Some(repo_root), Some(config_dir));
    let evaluated = rules.evaluate(change);
    Plan {
        model: evaluated.review.model,
        reviewers: u32::from(evaluated.review.reviewers),
    }
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()
    })
}

// ---------------------------------------------------------------------------
// The service

/// A branch to review, from an agent's card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// The repository's display name.
    pub(crate) repo_name: String,
    /// Its main checkout: what reviews are keyed by.
    pub(crate) repo_root: PathBuf,
    /// The worktree the review runs in.
    pub(crate) cwd: PathBuf,
    /// The branch.
    pub(crate) branch: String,
    /// The branch it goes into.
    pub(crate) base: String,
}

impl Target {
    /// The target of an agent's card, when it has a branch and a base.
    pub(crate) fn of(card: &AgentCard) -> Option<Self> {
        let repo = card.repo.as_ref()?;
        Some(Self {
            repo_name: repo.name.clone(),
            repo_root: repo.root.clone(),
            cwd: card.cwd.clone(),
            branch: repo.branch.clone()?,
            base: repo.base.clone()?,
        })
    }

    /// The commit the branch points at, read from git.
    pub(crate) fn tip(&self) -> Option<String> {
        git(
            &self.cwd,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", self.branch),
            ],
        )
    }

    fn repo_key(&self) -> String {
        self.repo_root.to_string_lossy().into_owned()
    }
}

/// What asking for a review did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Requested {
    /// A review was queued now.
    Queued,
    /// The tip already has a review (any status): nothing was queued.
    Exists,
    /// The rules say not to review this change (0 reviewers).
    Skipped,
    /// The review is switched off.
    Disabled,
    /// The branch has no readable tip.
    NoTip,
}

/// The review service: queue, cache and runner.
pub(crate) struct Reviews {
    settings: volee_config::Review,
    config_dir: PathBuf,
    store: Arc<Mutex<Store>>,
    reviewer: Arc<dyn Reviewer>,
    slots: Arc<Semaphore>,
}

impl std::fmt::Debug for Reviews {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reviews").finish_non_exhaustive()
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

impl Reviews {
    /// The service over the database at `database`. Reviews a stopped daemon
    /// left unfinished are failed: nothing is resumed.
    pub(crate) fn open(
        settings: volee_config::Review,
        config_dir: PathBuf,
        database: &Path,
        reviewer: Arc<dyn Reviewer>,
    ) -> Result<Self, volee_store::StoreError> {
        let store = Store::open(database)?;
        match store.interrupt_reviews(now()) {
            Ok(0) => {}
            Ok(count) => tracing::info!(count, "failed the reviews a stop interrupted"),
            Err(error) => tracing::warn!(%error, "cannot fail the interrupted reviews"),
        }
        Ok(Self {
            slots: Arc::new(Semaphore::new(settings.concurrency.max(1))),
            settings,
            config_dir,
            store: Arc::new(Mutex::new(store)),
            reviewer,
        })
    }

    /// Whether finished work is reviewed as soon as it appears.
    pub(crate) fn automatic(&self) -> bool {
        self.settings.enabled && self.settings.auto
    }

    /// Whether reviews are on at all.
    pub(crate) fn enabled(&self) -> bool {
        self.settings.enabled
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The review of a branch at its current tip, if one was asked for.
    pub(crate) fn current(&self, target: &Target) -> Option<StoredReview> {
        let tip = target.tip()?;
        self.lock()
            .review(&target.repo_key(), &target.branch, &tip)
            .map_err(|error| tracing::warn!(%error, "cannot read the review"))
            .ok()
            .flatten()
    }

    /// Queues a review of the branch at its current tip, unless the tip has
    /// one already. `retry` also redoes a failed one. Blocking (git, the
    /// rules): call it from a blocking thread. The review itself runs on the
    /// Tokio runtime `handle`.
    pub(crate) fn request(
        self: &Arc<Self>,
        handle: &tokio::runtime::Handle,
        target: &Target,
        retry: bool,
    ) -> Requested {
        if !self.settings.enabled {
            return Requested::Disabled;
        }
        let Some(tip) = target.tip() else {
            return Requested::NoTip;
        };
        let plan = plan_for(
            &change_of(target, &tip),
            &target.repo_root,
            &self.config_dir,
        );
        if plan.reviewers == 0 {
            return Requested::Skipped;
        }
        let id = {
            let store = self.lock();
            let new = NewReview {
                repo: &target.repo_key(),
                branch: &target.branch,
                tip: &tip,
                base: &target.base,
                reviewers: plan.reviewers.min(2),
                model: &plan.model,
                at: now(),
            };
            match store.begin_review(&new) {
                Ok(Some(id)) => id,
                Ok(None) => {
                    let existing = store.review(new.repo, &target.branch, &tip).ok().flatten();
                    match existing {
                        Some(review)
                            if retry
                                && review.status == review_status::FAILED
                                && store.retry_review(review.id, now()).unwrap_or(false) =>
                        {
                            review.id
                        }
                        _ => return Requested::Exists,
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "cannot record the review");
                    return Requested::Exists;
                }
            }
        };
        let service = Arc::clone(self);
        let target = target.clone();
        handle.spawn(async move {
            service.run(id, target, tip, plan).await;
        });
        Requested::Queued
    }

    async fn run(self: Arc<Self>, id: i64, target: Target, tip: String, plan: Plan) {
        let Ok(_slot) = Arc::clone(&self.slots).acquire_owned().await else {
            return;
        };
        let service = Arc::clone(&self);
        let outcome = tokio::task::spawn_blocking(move || {
            service.lock().start_review(id, now()).ok();
            service.run_reviewers(&target, &tip, &plan)
        })
        .await;
        let store = Arc::clone(&self.store);
        let mut store = store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match outcome {
            Ok(Ok((raw, cost, findings))) => {
                if let Err(error) = store.finish_review(id, now(), &raw, cost, &findings) {
                    tracing::warn!(%error, "cannot store the review");
                }
            }
            Ok(Err(reason)) => {
                tracing::info!(%reason, "a review failed");
                store.fail_review(id, now(), &reason).ok();
            }
            Err(error) => {
                store
                    .fail_review(id, now(), &format!("the review task failed: {error}"))
                    .ok();
            }
        }
    }

    /// Runs the reviewers one after the other (frugal: never two models at
    /// once) and merges what they found.
    fn run_reviewers(
        &self,
        target: &Target,
        tip: &str,
        plan: &Plan,
    ) -> Result<(String, Option<f64>, Vec<NewFinding>), String> {
        let mut raw = Vec::new();
        let mut cost: Option<f64> = None;
        let mut groups = Vec::new();
        for level in LEVELS.iter().take(plan.reviewers.clamp(1, 2) as usize) {
            let request = ReviewRequest {
                cwd: target.cwd.clone(),
                base: target.base.clone(),
                tip: tip.to_owned(),
                level,
                model: plan.model.clone(),
                session_name: format!("{SESSION_PREFIX}-{}-{level}", &tip[..tip.len().min(8)]),
            };
            let output = self.reviewer.review(&request)?;
            groups.push(parse_findings(&output.text, &target.cwd));
            raw.push(format!("## {level}\n{}", output.text.trim()));
            cost = match (cost, output.cost_usd) {
                (Some(a), Some(b)) => Some(a + b),
                (a, b) => a.or(b),
            };
        }
        Ok((raw.join("\n\n"), cost, merge_findings(groups)))
    }

    /// Reviews the finished work of every card that has none for its tip.
    pub(crate) fn review_finished(
        self: &Arc<Self>,
        handle: &tokio::runtime::Handle,
        cards: &[AgentCard],
    ) {
        for card in cards.iter().filter(|card| card.finished_work) {
            if is_review_session(card.name.as_deref()) {
                continue;
            }
            if let Some(target) = Target::of(card) {
                self.request(handle, &target, false);
            }
        }
    }
}

/// Reviews finished work as it appears, until `stop`.
pub(crate) async fn watch_finished(
    reviews: Arc<Reviews>,
    agents: Arc<Agents>,
    mut stop: watch::Receiver<bool>,
) {
    if !reviews.automatic() {
        return;
    }
    let mut revisions = agents.subscribe();
    let handle = tokio::runtime::Handle::current();
    loop {
        if let Some(view) = agents.peek() {
            let reviews = Arc::clone(&reviews);
            let handle = handle.clone();
            let cards = view.agents;
            let _ =
                tokio::task::spawn_blocking(move || reviews.review_finished(&handle, &cards)).await;
        }
        tokio::select! {
            moved = revisions.changed() => if moved.is_err() { return },
            _ = stop.changed() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // What `/code-review low` printed on a two-bug branch (Claude Code
    // 2.1.330, haiku), and what `medium` printed. Real outputs, trimmed.
    const LOW: &str = "util.py:10 \u{2014} important \u{2014} `last(xs)` lit `xs[len(xs)]`, hors bornes. Fix : `return xs[-1]`.\n\nutil.py:14 \u{2014} important \u{2014} `percent(a, b)` divise sans garde : `percent(1, 0)` l\u{e8}ve `ZeroDivisionError`.\n\nBoth functions are new in this diff.";
    const MEDIUM: &str = "Le diff a \u{e9}t\u{e9} lu.\n\n2 findings (2 \u{1f534} Important):\n\n1. \u{1f534} Important `/work/trial/util.py:8`: last(xs) crashes on any list. Fix: return xs[-1].\n\n   Failure scenario: last([1, 2, 3]) \u{2192} IndexError.\n\n2. \u{1f534} Important `/work/trial/util.py:12`: percent(a, b) ne g\u{e8}re pas b == 0. Fix: lever ValueError.\n\n   Failure scenario: percent(5, 0) \u{2192} ZeroDivisionError.\n\nAucun autre d\u{e9}faut.";

    #[test]
    fn the_low_layout_gives_file_line_severity_and_summary() {
        let findings = parse_findings(LOW, Path::new("/work/trial"));
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].file, "util.py");
        assert_eq!(findings[0].line, Some(10));
        assert_eq!(findings[0].severity, "important");
        assert!(findings[0].summary.starts_with("`last(xs)` lit"));
        assert_eq!(findings[1].line, Some(14));
        assert!(findings[1].summary.contains("ZeroDivisionError"));
    }

    #[test]
    fn the_numbered_layout_gives_absolute_paths_made_relative_and_the_scenario() {
        let findings = parse_findings(MEDIUM, Path::new("/work/trial"));
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].file, "util.py");
        assert_eq!(findings[0].line, Some(8));
        assert_eq!(findings[0].severity, "important");
        assert!(findings[0].summary.starts_with("last(xs) crashes"));
        assert!(
            findings[0]
                .failure_scenario
                .as_deref()
                .unwrap()
                .starts_with("last([1, 2, 3])")
        );
        assert_eq!(findings[1].line, Some(12));
    }

    #[test]
    fn nits_and_pre_existing_findings_are_told_apart() {
        let text = "src/a.rs:3 - nit - rename x\nsrc/b.rs:9 - pre-existing - old unwrap\n";
        let findings = parse_findings(text, Path::new("/w"));
        assert_eq!(findings[0].severity, "nit");
        assert_eq!(findings[0].summary, "rename x");
        assert_eq!(findings[1].severity, "pre_existing");
        assert_eq!(findings[1].summary, "old unwrap");
    }

    #[test]
    fn prose_and_a_clean_review_give_no_finding() {
        let text = "No issues found. Listening on localhost:8080 was fine, see v1.2:3.";
        assert_eq!(parse_findings(text, Path::new("/w")).len(), 0);
        assert_eq!(parse_findings("", Path::new("/w")).len(), 0);
    }

    #[test]
    fn two_reviewers_report_a_line_once() {
        let one = parse_findings(LOW, Path::new("/work/trial"));
        let mut two = parse_findings(MEDIUM, Path::new("/work/trial"));
        two[0].line = Some(10);
        let merged = merge_findings(vec![one, two]);
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn the_command_is_read_only_hookless_and_named() {
        let request = ReviewRequest {
            cwd: PathBuf::from("/w"),
            base: "main".into(),
            tip: "abc".into(),
            level: "low",
            model: "haiku".into(),
            session_name: "volee-review-abc-low".into(),
        };
        let args = ClaudeReviewer::args(&request);
        assert_eq!(args[1], "/code-review low main...abc");
        assert!(args.windows(2).any(|w| w == ["--model", "haiku"]));
        assert!(args.windows(2).any(|w| w == ["--permission-mode", "plan"]));
        assert!(
            args.windows(2)
                .any(|w| w == ["--disallowedTools", "Edit,Write,NotebookEdit"])
        );
        assert!(args.iter().any(|a| a.contains("disableAllHooks")));
        assert!(!args.iter().any(|a| a == "--fix"));
        assert!(is_review_session(Some(&request.session_name)));
        assert!(!is_review_session(Some("my work")));
        assert!(!is_review_session(None));
    }

    #[test]
    fn the_json_output_gives_the_text_and_the_cost() {
        let ok = parse_output(r#"{"is_error":false,"result":"a.py:1 - x","total_cost_usd":0.02}"#)
            .unwrap();
        assert_eq!(ok.text, "a.py:1 - x");
        assert_eq!(ok.cost_usd, Some(0.02));
        assert!(parse_output(r#"{"is_error":true,"result":"boom"}"#).is_err());
        assert!(parse_output("not json").is_err());
    }

    // ---- the service, over a real temporary repository and a fake reviewer

    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Fake {
        calls: AtomicUsize,
        running: AtomicUsize,
        most: AtomicUsize,
        fail: bool,
        levels: Mutex<Vec<String>>,
    }

    impl Reviewer for Fake {
        fn review(&self, request: &ReviewRequest) -> Result<ReviewOutput, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(150));
            self.running.fetch_sub(1, Ordering::SeqCst);
            self.levels.lock().unwrap().push(format!(
                "{} {} {}",
                request.level, request.model, request.session_name
            ));
            if self.fail {
                return Err("the fake failed".to_owned());
            }
            let line = if request.level == "low" { 8 } else { 9 };
            Ok(ReviewOutput {
                text: format!(
                    "util.py:{line} - important - bug found by {}",
                    request.level
                ),
                cost_usd: Some(0.01),
            })
        }
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A repository with `main` and a branch that adds a line.
    fn repo(branch: &str) -> (tempfile::TempDir, Target) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run_git(root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("util.py"), "a = 1\n").unwrap();
        run_git(root, &["add", "-A"]);
        run_git(root, &["commit", "-qm", "base"]);
        run_git(root, &["switch", "-qc", branch]);
        std::fs::write(root.join("util.py"), "a = 1\nb = 2\n").unwrap();
        run_git(root, &["commit", "-qam", "work"]);
        let target = Target {
            repo_name: "trial".to_owned(),
            repo_root: root.to_owned(),
            cwd: root.to_owned(),
            branch: branch.to_owned(),
            base: "main".to_owned(),
        };
        (dir, target)
    }

    fn service(
        fake: &Arc<Fake>,
        concurrency: usize,
        enabled: bool,
    ) -> (tempfile::TempDir, Arc<Reviews>) {
        let dir = tempfile::tempdir().unwrap();
        let settings = volee_config::Review {
            enabled,
            concurrency,
            ..volee_config::Review::default()
        };
        let reviews = Reviews::open(
            settings,
            dir.path().join("config"),
            &dir.path().join("volee.db"),
            fake.clone(),
        )
        .unwrap();
        (dir, Arc::new(reviews))
    }

    async fn ask(reviews: &Arc<Reviews>, target: &Target, retry: bool) -> Requested {
        let handle = tokio::runtime::Handle::current();
        let (reviews, target) = (Arc::clone(reviews), target.clone());
        tokio::task::spawn_blocking(move || reviews.request(&handle, &target, retry))
            .await
            .unwrap()
    }

    async fn settled(reviews: &Arc<Reviews>, target: &Target) -> StoredReview {
        for _ in 0..200 {
            if let Some(review) = reviews.current(target)
                && (review.status == review_status::DONE || review.status == review_status::FAILED)
            {
                return review;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the review did not settle");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_tip_is_reviewed_once_and_its_findings_are_stored() {
        let fake = Arc::new(Fake::default());
        let (_db, reviews) = service(&fake, 1, true);
        let (_repo, target) = repo("feat");
        assert_eq!(ask(&reviews, &target, false).await, Requested::Queued);
        let done = settled(&reviews, &target).await;
        assert_eq!(done.status, review_status::DONE);
        assert_eq!(done.findings.len(), 1);
        assert_eq!(done.findings[0].file, "util.py");
        assert_eq!(done.findings[0].line, Some(8));
        assert_eq!(done.model, "haiku");
        assert_eq!(done.cost_usd, Some(0.01));
        // The same tip, asked again (even with retry): never reviewed twice.
        assert_eq!(ask(&reviews, &target, true).await, Requested::Exists);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        let levels = fake.levels.lock().unwrap().clone();
        assert!(
            levels[0].starts_with("low haiku volee-review-"),
            "{levels:?}"
        );
        let name = levels[0].split(' ').nth(2).unwrap();
        assert!(is_review_session(Some(name)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_review_is_not_redone_unless_asked() {
        let fake = Arc::new(Fake {
            fail: true,
            ..Fake::default()
        });
        let (_db, reviews) = service(&fake, 1, true);
        let (_repo, target) = repo("feat");
        assert_eq!(ask(&reviews, &target, false).await, Requested::Queued);
        let failed = settled(&reviews, &target).await;
        assert_eq!(failed.status, review_status::FAILED);
        assert!(failed.error.unwrap().contains("the fake failed"));
        assert_eq!(ask(&reviews, &target, false).await, Requested::Exists);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        assert_eq!(ask(&reviews, &target, true).await, Requested::Queued);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_switched_off_service_reviews_nothing() {
        let fake = Arc::new(Fake::default());
        let (_db, reviews) = service(&fake, 1, false);
        let (_repo, target) = repo("feat");
        assert_eq!(ask(&reviews, &target, true).await, Requested::Disabled);
        assert!(!reviews.enabled() && !reviews.automatic());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn at_most_the_configured_number_of_reviews_run_at_once() {
        let fake = Arc::new(Fake::default());
        let (_db, reviews) = service(&fake, 1, true);
        let (_repo_a, a) = repo("feat-a");
        let (_repo_b, b) = repo("feat-b");
        ask(&reviews, &a, false).await;
        ask(&reviews, &b, false).await;
        settled(&reviews, &a).await;
        settled(&reviews, &b).await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
        assert_eq!(fake.most.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_rules_choose_the_reviewers_and_can_skip_the_review() {
        let fake = Arc::new(Fake::default());
        let (_db, reviews) = service(&fake, 1, true);
        let (repo_dir, target) = repo("feat");
        std::fs::create_dir_all(repo_dir.path().join(".volee")).unwrap();
        std::fs::write(
            repo_dir.path().join(".volee/rules.lua"),
            "return { sensitive_paths = { \"*.py\" } }\n",
        )
        .unwrap();
        ask(&reviews, &target, false).await;
        let done = settled(&reviews, &target).await;
        // A sensitive path is high risk: two reviewers, `low` then `medium`.
        assert_eq!(done.reviewers, 2);
        let levels = fake.levels.lock().unwrap().clone();
        assert_eq!(levels.len(), 2, "{levels:?}");
        assert!(levels[0].starts_with("low ") && levels[1].starts_with("medium "));
        assert_eq!(done.findings.len(), 2, "the passes found different lines");

        let (repo_dir, target) = repo("feat-skip");
        std::fs::create_dir_all(repo_dir.path().join(".volee")).unwrap();
        std::fs::write(
            repo_dir.path().join(".volee/rules.lua"),
            "return { review = { reviewers = 0 } }\n",
        )
        .unwrap();
        assert_eq!(ask(&reviews, &target, false).await, Requested::Skipped);
    }
}
