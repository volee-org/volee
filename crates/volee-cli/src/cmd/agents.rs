//! `volee agents`: the Claude Code sessions the daemon sees, with their
//! branch, what git says of it, and whether they finished work.

use std::fmt::Write as _;

use anyhow::Result;
use volee_core::state::AgentState;
use volee_proto::methods::{AgentCard, AgentsView, ViewAgents, ViewParams};

use crate::connect::connect_or_start;
use crate::text;

/// Prints the agents as a table, or as JSON with `--json`. Sessions whose
/// process ended are left out unless `all`.
pub(crate) async fn run(json: bool, all: bool) -> Result<()> {
    let client = connect_or_start().await?;
    let mut view = client
        .call::<ViewAgents>(ViewParams {
            max_age_ms: Some(1_000),
        })
        .await?;
    if !all {
        view.agents.retain(|card| card.alive);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&view)?);
    } else {
        print!("{}", render(&view));
    }
    Ok(())
}

/// The table, with a note when the last listing failed.
fn render(view: &AgentsView) -> String {
    let mut out = String::new();
    if let Some(error) = &view.error {
        let _ = writeln!(
            out,
            "warning: {} (showing the last reading)\n",
            text::printable(error)
        );
    }
    if view.agents.is_empty() {
        out.push_str(if view.updated_at.is_none() && view.error.is_some() {
            "No agents known: Claude Code could not be listed.\n"
        } else {
            "No agents are running.\n"
        });
        return out;
    }
    let rows: Vec<[String; 5]> = view.agents.iter().map(row).collect();
    let header = ["NAME", "STATE", "WHERE", "GIT", "FINISHED WORK"];
    let widths: [usize; 5] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .chain([header[column].chars().count()])
            .max()
            .unwrap_or(0)
    });
    let line = |cells: [&str; 5]| {
        let padded: Vec<String> = cells
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        format!("{}\n", padded.join("  ").trim_end())
    };
    out.push_str(&line(header));
    for row in &rows {
        out.push_str(&line(std::array::from_fn(|i| row[i].as_str())));
    }
    out
}

fn row(card: &AgentCard) -> [String; 5] {
    [
        name(card),
        state(card),
        place(card),
        git(card),
        finished(card),
    ]
}

fn name(card: &AgentCard) -> String {
    card.name.as_deref().map_or_else(
        || card.session_id.chars().take(8).collect(),
        |name| text::one_line(name).into_owned(),
    )
}

fn state(card: &AgentCard) -> String {
    let label = match &card.state {
        AgentState::Launching => "launching",
        AgentState::Working => "working",
        AgentState::NeedsInput { .. } => "needs input",
        AgentState::Idle => "idle",
        AgentState::Done => "done",
        AgentState::Failed => "failed",
        AgentState::Stopped => "stopped",
        AgentState::Lost => "lost",
    };
    match &card.waiting_for {
        Some(what) => format!("{label}: {}", text::one_line(what)),
        None => label.to_owned(),
    }
}

/// `repo › branch`, with `(worktree)` for a linked worktree.
fn place(card: &AgentCard) -> String {
    let Some(repo) = &card.repo else {
        return card
            .git_error
            .as_ref()
            .map_or_else(|| "-".to_owned(), |_| "(git unreadable)".to_owned());
    };
    let branch = repo.branch.as_deref().unwrap_or("(detached)");
    let tree = if repo.worktree.is_some() {
        " (worktree)"
    } else {
        ""
    };
    // Where the branch came from; a `?` when that is only a guess.
    let base = match (repo.base.as_deref(), repo.base_source.as_deref()) {
        (Some(base), Some("default" | "nearest")) => format!(" from {}?", text::one_line(base)),
        (Some(base), _) => format!(" from {}", text::one_line(base)),
        (None, _) => String::new(),
    };
    format!(
        "{} \u{203a} {branch}{tree}{base}",
        text::one_line(&repo.name)
    )
}

/// `ahead`, `behind` and uncommitted changes against the base branch.
fn git(card: &AgentCard) -> String {
    let Some(facts) = &card.git else {
        return String::new();
    };
    let mut parts = vec![format!("\u{2191}{} \u{2193}{}", facts.ahead, facts.behind)];
    if facts.merged {
        parts.push("merged".to_owned());
    }
    if facts.dirty {
        parts.push("dirty".to_owned());
    }
    parts.join(" ")
}

/// For finished work, what it changes: commits and the diff summary.
fn finished(card: &AgentCard) -> String {
    let (true, Some(facts)) = (card.finished_work, &card.git) else {
        return String::new();
    };
    let commits = if facts.unique == 1 {
        "commit"
    } else {
        "commits"
    };
    let files = if facts.diff.files == 1 {
        "file"
    } else {
        "files"
    };
    format!(
        "yes: {} {commits}, {} {files} +{} \u{2212}{}",
        facts.unique, facts.diff.files, facts.diff.insertions, facts.diff.deletions
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use volee_core::delivery::{DiffStat, PushState};
    use volee_core::finished::Activity;
    use volee_core::state::InputRequest;
    use volee_proto::methods::{GitFacts, HookFacts, RepoFacts};

    use super::*;

    fn card(name: &str, state: AgentState) -> AgentCard {
        AgentCard {
            session_id: "0123456789abcdef".into(),
            name: Some(name.into()),
            background: false,
            alive: true,
            pid: Some(1),
            cwd: PathBuf::from("/work/alpha"),
            started_at: None,
            waiting_for: None,
            state,
            repo: None,
            git: None,
            git_error: None,
            hooks: HookFacts {
                activity: Activity::default(),
                ..HookFacts::default()
            },
            finished_work: false,
        }
    }

    fn with_work(mut card: AgentCard) -> AgentCard {
        card.repo = Some(RepoFacts {
            name: "alpha".into(),
            root: PathBuf::from("/work/alpha"),
            worktree: Some(PathBuf::from("/work/alpha/.claude/worktrees/feat")),
            branch: Some("feat/invoices".into()),
            base: Some("main".into()),
            base_source: Some("reflog".into()),
        });
        card.git = Some(GitFacts {
            ahead: 2,
            unique: 2,
            behind: 1,
            dirty: false,
            merged: false,
            diff: DiffStat {
                files: 3,
                insertions: 10,
                deletions: 2,
            },
            push: PushState::default(),
        });
        card.finished_work = true;
        card
    }

    #[test]
    fn finished_work_shows_its_commits_and_diff_summary() {
        let view = AgentsView {
            updated_at: None,
            error: None,
            agents: vec![with_work(card("billing", AgentState::Idle))],
        };
        let table = render(&view);
        assert!(table.starts_with("NAME"), "{table}");
        assert!(
            table.contains("alpha \u{203a} feat/invoices (worktree) from main"),
            "{table}"
        );
        assert!(table.contains("\u{2191}2 \u{2193}1"), "{table}");
        assert!(
            table.contains("yes: 2 commits, 3 files +10 \u{2212}2"),
            "{table}"
        );
    }

    #[test]
    fn a_waiting_agent_says_what_it_waits_for() {
        let mut waiting = card(
            "taxes",
            AgentState::NeedsInput {
                request: InputRequest::Question,
            },
        );
        waiting.waiting_for = Some("an answer to a question".into());
        let table = render(&AgentsView {
            updated_at: None,
            error: None,
            agents: vec![waiting],
        });
        assert!(
            table.contains("needs input: an answer to a question"),
            "{table}"
        );
    }

    #[test]
    fn a_failed_listing_is_said_and_the_last_reading_still_shown() {
        let table = render(&AgentsView {
            updated_at: None,
            error: Some("cannot list sessions: boom".into()),
            agents: vec![card("a", AgentState::Idle)],
        });
        assert!(
            table.starts_with("warning: cannot list sessions: boom"),
            "{table}"
        );
        assert!(table.contains("NAME"), "{table}");
        let nothing = render(&AgentsView {
            updated_at: None,
            error: Some("boom".into()),
            agents: vec![],
        });
        assert!(nothing.contains("could not be listed"), "{nothing}");
        let quiet = render(&AgentsView::default());
        assert_eq!(quiet, "No agents are running.\n");
    }
}
