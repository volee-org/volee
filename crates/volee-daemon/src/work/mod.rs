//! The finished-work card screen: what an agent did, in layers.
//!
//! `GET /api/agents/{id}/work` answers with [`Work`]: a one-line summary and
//! the assistant's own, the plan followed, the open points, the files with
//! the reason each was touched, the commands run, a Mermaid diagram of the
//! change and the diff per file. Git is the source of truth for what
//! changed; the transcript only says why and how. When the transcript cannot
//! be read the screen still has everything git and the hooks know.

// Style lints that cost more than they give in this parsing code.
#![allow(
    clippy::assert_is_empty,
    clippy::format_push_string,
    clippy::items_after_statements,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::case_sensitive_file_extension_comparisons
)]

pub(crate) mod diagram;
pub(crate) mod diff;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use volee_proto::methods::AgentCard;

use crate::transcript::{self, Outcome, Plan, Prompt, StepStatus, Touch, Transcript, clip};
use diff::{Category, FileDiff};

/// The summary of the work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Summary {
    /// One line.
    pub(crate) line: String,
    /// The assistant's final words, whole, when the transcript has them.
    pub(crate) text: Option<String>,
    /// `transcript` or `git`.
    pub(crate) source: &'static str,
}

/// Something the reader should know is left.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct OpenPoint {
    /// `stated` (the agent said it) or `detected` (volee noticed it).
    pub(crate) kind: &'static str,
    pub(crate) text: String,
}

/// A file of the change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WorkFile {
    #[serde(flatten)]
    pub(crate) diff: FileDiff,
    /// The tools that touched it, in order (`Edit`, `Write`...).
    pub(crate) touched_by: Vec<String>,
    /// What the assistant said just before it first touched the file.
    pub(crate) why: Option<String>,
}

/// A commit of the branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Commit {
    pub(crate) sha: String,
    pub(crate) subject: String,
}

/// What the transcript gave, and how well.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TranscriptInfo {
    pub(crate) available: bool,
    /// Why it is not, when it is not.
    pub(crate) reason: Option<String>,
    pub(crate) lines: usize,
    pub(crate) skipped: usize,
    pub(crate) prompts: Vec<Prompt>,
}

/// Whether tests ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Tests {
    pub(crate) ran: usize,
    /// Outcome of the last test run: `passed`, `failed`, or `unknown`.
    pub(crate) last: &'static str,
}

/// The diff and how it was read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DiffInfo {
    pub(crate) base: Option<String>,
    pub(crate) branch: Option<String>,
    /// Why git could not give the diff.
    pub(crate) error: Option<String>,
    pub(crate) truncated: bool,
    pub(crate) files: u32,
    pub(crate) insertions: u32,
    pub(crate) deletions: u32,
}

/// The layers of the finished-work screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Work {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    pub(crate) transcript: TranscriptInfo,
    pub(crate) summary: Summary,
    pub(crate) plan: Option<Plan>,
    pub(crate) open_points: Vec<OpenPoint>,
    pub(crate) commits: Vec<Commit>,
    pub(crate) files: Vec<WorkFile>,
    /// Files an editing tool touched that are not in the committed diff
    /// (uncommitted, or changed back).
    pub(crate) other_touched: Vec<String>,
    pub(crate) commands: Vec<transcript::Command>,
    pub(crate) tests: Tests,
    pub(crate) diff: DiffInfo,
    pub(crate) diagram: String,
}

/// Builds the screen of `agent`. `projects` is Claude Code's projects
/// directory (`~/.claude/projects`), where the transcript lives.
pub(crate) fn build(agent: &AgentCard, projects: Option<&Path>) -> Work {
    let repo = agent.repo.as_ref();
    let cwd: PathBuf = repo
        .and_then(|repo| repo.worktree.clone())
        .unwrap_or_else(|| agent.cwd.clone());
    let base = repo.and_then(|repo| repo.base.clone());
    let branch = repo.and_then(|repo| repo.branch.clone());
    let head = branch.clone().unwrap_or_else(|| "HEAD".to_owned());

    let commits = base
        .as_deref()
        .map(|base| commits(&cwd, base, &head))
        .unwrap_or_default();
    let since = commits.first().and_then(|(_, _, at)| *at);

    let (files_diff, diff_error, truncated) = match base.as_deref() {
        None => (
            Vec::new(),
            Some("the base branch is not known".to_owned()),
            false,
        ),
        Some(base) => match diff::run(&cwd, base, &head) {
            Ok(done) => (done.files, None, done.truncated),
            Err(why) => (Vec::new(), Some(why), false),
        },
    };

    let transcript = projects
        .and_then(|projects| transcript::locate(projects, &cwd, &agent.session_id))
        .ok_or(transcript::Unavailable::NotFound)
        .and_then(|path| transcript::read(&path, since));
    let (read, info) = match transcript {
        Ok(read) => {
            let info = TranscriptInfo {
                available: true,
                reason: None,
                lines: read.lines,
                skipped: read.skipped,
                prompts: read.prompts.clone(),
            };
            (read, info)
        }
        Err(why) => (
            Transcript::default(),
            TranscriptInfo {
                available: false,
                reason: Some(why.to_string()),
                lines: 0,
                skipped: 0,
                prompts: Vec::new(),
            },
        ),
    };

    let roots = [Some(cwd.as_path()), repo.map(|repo| repo.root.as_path())];
    let files = files_of(files_diff, &read.touches, &roots);
    let other_touched = untracked_touches(&files, &read.touches, &roots);
    let summary = summary_of(&read, &commits_view(&commits));
    let tests = tests_of(&read);
    let open_points = open_points(
        &read,
        &files,
        info.available,
        agent.git.as_ref().is_some_and(|git| git.dirty),
        &tests,
    );
    let totals = files.iter().fold((0, 0), |(i, d), f| {
        (i + f.diff.insertions, d + f.diff.deletions)
    });
    let diagram = diagram::mermaid(&files.iter().map(|f| f.diff.clone()).collect::<Vec<_>>());
    Work {
        id: agent.session_id.clone(),
        name: agent.name.clone(),
        transcript: info,
        summary,
        plan: read.plan,
        open_points,
        commits: commits_view(&commits),
        diff: DiffInfo {
            base,
            branch,
            error: diff_error,
            truncated,
            files: u32::try_from(files.len()).unwrap_or(u32::MAX),
            insertions: totals.0,
            deletions: totals.1,
        },
        files,
        other_touched,
        commands: read.commands,
        tests,
        diagram,
    }
}

fn commits_view(commits: &[(String, String, Option<i64>)]) -> Vec<Commit> {
    commits
        .iter()
        .map(|(sha, subject, _)| Commit {
            sha: sha.chars().take(7).collect(),
            subject: subject.clone(),
        })
        .collect()
}

/// The branch's commits, oldest first, with their time in milliseconds.
fn commits(cwd: &Path, base: &str, head: &str) -> Vec<(String, String, Option<i64>)> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "log",
            "--reverse",
            "--format=%H%x1f%ct%x1f%s",
            "--end-of-options",
        ])
        .arg(format!("{base}..{head}"))
        .output();
    let Ok(output) = output.map_err(|error| tracing::debug!(%error, "git log failed")) else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\u{1f}');
            let sha = parts.next()?.to_owned();
            let at = parts
                .next()?
                .parse::<i64>()
                .ok()
                .map(|seconds| seconds * 1000);
            let subject = parts.next()?.to_owned();
            Some((sha, subject, at))
        })
        .collect()
}

/// A path as the tool call gave it, relative to the checkout when it lies in
/// one.
fn relative(path: &str, roots: &[Option<&Path>]) -> String {
    for root in roots.iter().flatten() {
        if let Ok(rest) = Path::new(path).strip_prefix(root) {
            return rest.to_string_lossy().into_owned();
        }
    }
    path.to_owned()
}

/// Whether a touched path is this diff path: the same relative path, or an
/// absolute path that ends with it (the transcript was written elsewhere).
fn same_file(touched: &str, diff_path: &str, roots: &[Option<&Path>]) -> bool {
    relative(touched, roots) == diff_path
        || (touched.starts_with('/') && touched.ends_with(&format!("/{diff_path}")))
}

fn files_of(files: Vec<FileDiff>, touches: &[Touch], roots: &[Option<&Path>]) -> Vec<WorkFile> {
    files
        .into_iter()
        .map(|diff| {
            let own: Vec<&Touch> = touches
                .iter()
                .filter(|touch| same_file(&touch.path, &diff.path, roots))
                .collect();
            WorkFile {
                touched_by: own.iter().map(|touch| touch.tool.clone()).collect(),
                why: own.iter().find_map(|touch| touch.context.clone()),
                diff,
            }
        })
        .collect()
}

fn untracked_touches(
    files: &[WorkFile],
    touches: &[Touch],
    roots: &[Option<&Path>],
) -> Vec<String> {
    let mut other: Vec<String> = Vec::new();
    for touch in touches {
        let known = files
            .iter()
            .any(|file| same_file(&touch.path, &file.diff.path, roots));
        let path = relative(&touch.path, roots);
        if !known && !other.contains(&path) {
            other.push(path);
        }
    }
    other
}

fn summary_of(read: &Transcript, commits: &[Commit]) -> Summary {
    if let Some(text) = &read.summary {
        return Summary {
            line: first_line(text),
            text: Some(text.clone()),
            source: "transcript",
        };
    }
    let line = match commits {
        [] => "No commit and no final message: nothing to summarise.".to_owned(),
        [one] => clip(&one.subject, 160),
        [first, rest @ ..] => clip(
            &format!(
                "{} commits: {}, and {} more",
                commits.len(),
                first.subject,
                rest.len() - 1
            )
            .replace(", and 0 more", ""),
            160,
        ),
    };
    Summary {
        line,
        text: None,
        source: "git",
    }
}

/// The first sentence of a text, without markdown decoration.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(|line| line.trim().trim_start_matches(['#', '-', '*', ' ']).trim())
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .replace("**", "")
        .replace('`', "");
    let sentence = line
        .split_once(". ")
        .map_or(line.as_str(), |(first, _)| first)
        .to_owned();
    clip(&sentence, 160)
}

fn tests_of(read: &Transcript) -> Tests {
    let runs: Vec<&transcript::Command> = read.commands.iter().filter(|c| c.is_test).collect();
    let last = match runs.last().map(|run| run.outcome) {
        Some(Outcome::Ok) => "passed",
        Some(Outcome::Failed) => "failed",
        _ => "unknown",
    };
    Tests {
        ran: runs.len(),
        last,
    }
}

fn open_points(
    read: &Transcript,
    files: &[WorkFile],
    transcript_ok: bool,
    dirty: bool,
    tests: &Tests,
) -> Vec<OpenPoint> {
    let mut points = Vec::new();
    if let Some(text) = &read.summary {
        for line in stated_points(text) {
            points.push(OpenPoint {
                kind: "stated",
                text: line,
            });
        }
    }
    if let Some(plan) = &read.plan
        && plan.source == transcript::PlanSource::Tasks
    {
        for step in &plan.steps {
            if matches!(step.status, StepStatus::Pending | StepStatus::InProgress) {
                points.push(OpenPoint {
                    kind: "detected",
                    text: format!("Plan step not done: {}", step.text),
                });
            }
        }
    }
    if tests.last == "failed" {
        points.push(OpenPoint {
            kind: "detected",
            text: "The last test run failed.".to_owned(),
        });
    }
    let changes_code = files
        .iter()
        .any(|file| file.diff.category == Category::Source);
    if transcript_ok && changes_code && tests.ran == 0 {
        points.push(OpenPoint {
            kind: "detected",
            text: "Source changed and no test run is recorded in the transcript.".to_owned(),
        });
    }
    if dirty {
        points.push(OpenPoint {
            kind: "detected",
            text: "The worktree has uncommitted changes that are not in this diff.".to_owned(),
        });
    }
    points
}

/// What a final message admits is left: the lines under a heading such as
/// "Not done" when it has one, else the lines that carry a cue ("did not",
/// "n'ai pas"...). A heuristic, labelled as the agent's own words.
fn stated_points(text: &str) -> Vec<String> {
    const SECTIONS: [&str; 14] = [
        "not done",
        "not implemented",
        "left undone",
        "left to do",
        "remaining",
        "open point",
        "caveat",
        "limitation",
        "todo",
        "non fait",
        "pas fait",
        "reste à",
        "restant",
        "à faire",
    ];
    const CUES: [&str; 14] = [
        "didn't",
        "did not",
        "couldn't",
        "could not",
        "unable to",
        "skipped",
        "not implemented",
        "n'ai pas",
        "pas pu",
        "pas fait",
        "reste à",
        "restent",
        "à noter",
        "pas demandé",
    ];
    let clean = |line: &str| {
        line.trim()
            .trim_start_matches(['-', '*', ' '])
            .replace("**", "")
            .trim()
            .to_owned()
    };
    // A heading is a markdown heading, a line that is only bold, or a line
    // that ends with a colon.
    let is_heading = |line: &str| {
        let line = line.trim();
        line.starts_with('#')
            || (line.starts_with("**") && line.ends_with("**") && line.len() > 4)
            || (line.ends_with(':') && !line.starts_with(['-', '*']))
    };
    let mut under: Vec<String> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if is_heading(line) {
            let lower = clean(line.trim_start_matches('#')).to_lowercase();
            inside = SECTIONS.iter().any(|section| lower.contains(section));
        } else if inside {
            let item = clean(line);
            if !item.is_empty() {
                under.push(clip(&item, 240));
            }
        }
    }
    if !under.is_empty() {
        return under;
    }
    text.lines()
        .map(clean)
        .filter(|line| !line.is_empty() && !is_heading(line))
        .filter(|line| {
            let lower = line.to_lowercase();
            CUES.iter().any(|cue| lower.contains(cue))
        })
        .map(|line| clip(&line, 240))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::read_str;
    use volee_core::delivery::{DiffStat, PushState};
    use volee_core::state::AgentState;
    use volee_proto::methods::{GitFacts, HookFacts, RepoFacts};

    pub(super) fn run_git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=t@t.invalid", "-c", "user.name=t"])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub(super) fn card(cwd: &Path, session: &str, dirty: bool) -> AgentCard {
        AgentCard {
            session_id: session.to_owned(),
            name: Some("trial".to_owned()),
            background: false,
            alive: true,
            pid: None,
            cwd: cwd.to_path_buf(),
            started_at: None,
            state: AgentState::Idle,
            waiting_for: None,
            repo: Some(RepoFacts {
                name: "demo".to_owned(),
                root: cwd.to_path_buf(),
                worktree: None,
                branch: Some("feat/mul".to_owned()),
                base: Some("main".to_owned()),
                base_source: None,
            }),
            git: Some(GitFacts {
                ahead: 1,
                unique: 1,
                behind: 0,
                dirty,
                merged: false,
                diff: DiffStat {
                    files: 0,
                    insertions: 0,
                    deletions: 0,
                },
                push: PushState::NoUpstream,
                merge_source: None,
                merge_unknown: None,
                pull_request: None,
            }),
            git_error: None,
            hooks: HookFacts::default(),
            finished_work: true,
        }
    }

    /// A repository with `main` and a `feat/mul` branch of one commit.
    pub(super) fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        run_git(path, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(path.join("src")).unwrap();
        std::fs::create_dir_all(path.join("tests")).unwrap();
        std::fs::write(path.join("src/math.ts"), "export const add = 1;\n").unwrap();
        std::fs::write(path.join("README.md"), "# demo\n").unwrap();
        std::fs::write(
            path.join("tests/math.test.ts"),
            "import { add } from \"../src/math\";\n",
        )
        .unwrap();
        std::fs::write(path.join("package-lock.json"), "{}\n").unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-q", "-m", "chore: init"]);
        run_git(path, &["switch", "-q", "-c", "feat/mul"]);
        std::fs::write(
            path.join("src/math.ts"),
            "export const add = 1;\nexport const mul = 2;\n",
        )
        .unwrap();
        std::fs::write(path.join("README.md"), "# demo\n\nmul added\n").unwrap();
        std::fs::write(
            path.join("tests/math.test.ts"),
            "import { add, mul } from \"../src/math.ts\";\n",
        )
        .unwrap();
        std::fs::write(path.join("package-lock.json"), "{\n \"a\": 1\n}\n").unwrap();
        run_git(path, &["add", "-A"]);
        run_git(path, &["commit", "-q", "-m", "feat: add mul"]);
        dir
    }

    #[test]
    fn git_alone_gives_the_screen_when_there_is_no_transcript() {
        let dir = repo();
        let work = build(&card(dir.path(), "no-such-session", false), None);
        assert!(!work.transcript.available);
        assert!(
            work.transcript
                .reason
                .as_deref()
                .unwrap()
                .contains("no transcript")
        );
        assert_eq!(work.summary.source, "git");
        assert_eq!(work.summary.line, "feat: add mul");
        assert!(work.plan.is_none());
        assert_eq!(work.commits.len(), 1);
        assert_eq!(work.diff.files, 4);
        let order: Vec<&str> = work.files.iter().map(|f| f.diff.path.as_str()).collect();
        assert_eq!(
            order,
            [
                "src/math.ts",
                "tests/math.test.ts",
                "README.md",
                "package-lock.json"
            ]
        );
        assert!(work.files.iter().all(|f| f.why.is_none()));
        assert!(work.files[3].diff.collapsed);
        assert!(work.diagram.contains("flowchart LR"));
        assert!(
            work.diagram.contains("-->"),
            "the added import is an edge: {}",
            work.diagram
        );
        assert!(work.diff.error.is_none());
    }

    #[test]
    fn the_transcript_adds_why_plan_and_summary() {
        let dir = repo();
        let projects = tempfile::tempdir().unwrap();
        let project = projects.path().join(transcript::encode_cwd(dir.path()));
        std::fs::create_dir_all(&project).unwrap();
        let cwd = dir.path().to_string_lossy();
        let lines = [
            serde_json::json!({"type":"user","timestamp":"2020-01-01T00:00:00Z","origin":{"kind":"human"},
                "message":{"content":"1) add mul 2) test it 3) document it"}}),
            serde_json::json!({"type":"assistant","timestamp":"2020-01-01T00:00:01Z","message":{"content":[
                {"type":"text","text":"I add mul to the math module first."},
                {"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":format!("{cwd}/src/math.ts"),"old_string":"a","new_string":"b"}}]}}),
            serde_json::json!({"type":"assistant","timestamp":"2020-01-01T00:00:02Z","message":{"content":[
                {"type":"tool_use","id":"e2","name":"Write","input":{"file_path":format!("{cwd}/scratch.txt"),"content":"x"}}]}}),
            serde_json::json!({"type":"assistant","timestamp":"2020-01-01T00:00:03Z","message":{"content":[
                {"type":"text","text":"Done: mul works. I did not run the linter."}]}}),
        ];
        let text = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(project.join("sess-1.jsonl"), text).unwrap();

        let work = build(&card(dir.path(), "sess-1", true), Some(projects.path()));
        assert!(work.transcript.available);
        assert_eq!(work.summary.source, "transcript");
        assert_eq!(work.summary.line, "Done: mul works");
        assert_eq!(work.plan.as_ref().unwrap().steps.len(), 3);
        let math = work
            .files
            .iter()
            .find(|f| f.diff.path == "src/math.ts")
            .unwrap();
        assert_eq!(math.touched_by, ["Edit"]);
        assert_eq!(
            math.why.as_deref(),
            Some("I add mul to the math module first.")
        );
        assert_eq!(work.other_touched, ["scratch.txt"]);
        let points: Vec<&str> = work.open_points.iter().map(|p| p.text.as_str()).collect();
        assert!(points.iter().any(|p| p.contains("did not run the linter")));
        assert!(points.iter().any(|p| p.contains("no test run is recorded")));
        assert!(points.iter().any(|p| p.contains("uncommitted")));
    }

    #[test]
    fn the_real_recorded_transcript_matches_its_own_story() {
        let recorded = include_str!("../../tests/fixtures/haiku-two-tasks.jsonl");
        let read = read_str(recorded, Some(0));
        let tests = tests_of(&read);
        assert!(tests.ran >= 1);
        let summary = summary_of(&read, &[]);
        assert_eq!(summary.source, "transcript");
        assert!(!summary.line.is_empty());
    }

    #[test]
    fn a_missing_base_is_said_not_fatal() {
        let dir = repo();
        let mut agent = card(dir.path(), "x", false);
        agent.repo.as_mut().unwrap().base = None;
        let work = build(&agent, None);
        assert!(work.diff.error.as_deref().unwrap().contains("base"));
        assert!(work.files.is_empty());
        assert!(work.diagram.contains("no file changed"));
    }

    #[test]
    fn stated_points_prefer_the_section_that_lists_what_was_not_done() {
        let text = "Done.\n\n**Choices**\n- I did not add a guard.\n\n**Not done**\n- No push.\n- Lock file untouched.\n\n**Next**\n- Merge it.";
        assert_eq!(stated_points(text), ["No push.", "Lock file untouched."]);
        let french = "Fait.\n\n**Non faits**\n- Pas de push\n\n**Choix**\n- x";
        assert_eq!(stated_points(french), ["Pas de push"]);
    }

    #[test]
    fn stated_points_fall_back_to_cue_lines() {
        let text = "All good.\nI did not run the linter.\nI skipped the docs.\nThe end.";
        assert_eq!(
            stated_points(text),
            ["I did not run the linter.", "I skipped the docs."]
        );
        assert!(stated_points("Everything is done.").is_empty());
    }

    #[test]
    fn first_line_is_a_plain_sentence() {
        assert_eq!(
            first_line("## **Done** with `mul`. More text."),
            "Done with mul"
        );
        assert_eq!(first_line("\n\n  - one thing"), "one thing");
        assert!(first_line(&"x".repeat(500)).len() <= 163);
    }
}
