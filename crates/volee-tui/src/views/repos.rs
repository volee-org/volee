//! The repositories view: where agents work, and what they delivered.
//!
//! Repositories come first, then their worktrees (the main checkout first)
//! with the branch each one has checked out, then the agents in them, their
//! subagents under them. Every agent shows its delivery evidence: commits
//! ahead of its base, changes, push state, pull request and checks, and the
//! mission supervisor's verdict for mission workers. A worktree whose work
//! landed (merged, merged through a pull request, or reported) and that has
//! no uncommitted changes is marked safe to tidy, by the same rule as the
//! cleanup command (ADR 0011). Nothing here removes anything.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Span;
use volee_core::delivery::{
    ChecksState, Delivery, PrState, PushState, TidyRefusal, TidyVerdict, tidy_verdict,
};
use volee_core::mission::Verdict;
use volee_core::role::Role;
use volee_core::{Agent, AgentId, Registry};

use super::hierarchy::{Zoom, ZoomChoice};
use crate::app::Model;
use crate::text;
use crate::theme::{StyleRole, Theme};
use crate::widgets::{put_spans, state_glyph, truncated};

/// A worktree, or the main checkout, and the agents in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Worktree {
    /// The checked-out branch; `None` when detached or unknown.
    pub(crate) branch: Option<String>,
    /// The branch the work started from.
    pub(crate) base: Option<String>,
    /// The worktree's directory, or the main checkout's.
    pub(crate) path: PathBuf,
    /// Whether this is the main checkout rather than a linked worktree.
    pub(crate) main: bool,
    /// The agents working here, oldest first, without subagents.
    pub(crate) agents: Vec<AgentId>,
}

/// A repository and its worktrees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Repo {
    /// The repository's display name.
    pub(crate) name: String,
    /// Its main checkout, when known.
    pub(crate) root: Option<PathBuf>,
    /// Its worktrees, the main checkout first, then by branch.
    pub(crate) worktrees: Vec<Worktree>,
    /// How many agents work in it, subagents included.
    pub(crate) agents: usize,
}

/// One line of the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    /// A repository's name.
    Repo(usize),
    /// A repository with a single worktree, both on one line.
    Single(usize),
    /// A worktree of a repository.
    Worktree(usize, usize),
    /// The heading of agents outside any repository.
    Elsewhere,
    /// An agent: its name, or (with `evidence`) its delivery evidence.
    Agent {
        /// The agent.
        agent: AgentId,
        /// A subagent, shown under its parent.
        sub: bool,
        /// The evidence line rather than the name line.
        evidence: bool,
    },
    /// Room between two repositories.
    Blank,
}

/// The repositories view's state.
#[derive(Debug, Default)]
pub(crate) struct ReposState {
    /// The detail level the person asked for.
    pub(crate) zoom: ZoomChoice,
    /// The detail level in use: two lines per agent (cards), one line with
    /// the evidence (compact), or one line with the name only (dots).
    pub(crate) density: Zoom,
    /// The repositories, sorted by name.
    pub(crate) repos: Vec<Repo>,
    /// The lines, top to bottom.
    pub(crate) lines: Vec<Line>,
    /// Agents in the order they appear, for moving with the keys.
    pub(crate) order: Vec<AgentId>,
    /// The first line shown.
    pub(crate) offset: usize,
}

impl ReposState {
    /// Rebuilds the lines from the registry, keeping only the agents that
    /// pass `matches`, for an area `height` lines tall, and scrolls to keep
    /// `selected` in view.
    ///
    /// The automatic level shows the evidence in full when it fits, and on
    /// one line per agent otherwise, scrolling: names alone hide what this
    /// view is for, so only the person asks for them.
    pub(crate) fn relayout(
        &mut self,
        registry: &Registry,
        matches: &dyn Fn(&Agent) -> bool,
        height: u16,
        selected: Option<AgentId>,
    ) {
        let (repos, elsewhere, subagents) = group(registry, matches);
        let densities: &[Zoom] = match &self.zoom {
            ZoomChoice::Auto => &[Zoom::Cards, Zoom::Compact],
            ZoomChoice::Fixed(zoom) => std::slice::from_ref(zoom),
        };
        // Room between repositories when it fits, then less detail; the
        // last try stays even when it scrolls.
        let tries = densities
            .iter()
            .flat_map(|density| [(*density, true), (*density, false)]);
        // Agents in a repository have git evidence; coordinators have
        // their mission's progress; others outside repositories have none.
        let has_evidence = |agent: AgentId| {
            registry.agent(agent).is_some_and(|agent| {
                agent.workspace.repo.is_some() || agent.role == Role::Coordinator
            })
        };
        for (density, blanks) in tries {
            self.density = density;
            let shape = Shape {
                density,
                blanks,
                has_evidence: &has_evidence,
            };
            self.lines = lines(&repos, &elsewhere, &subagents, &shape);
            if self.lines.len() <= usize::from(height) {
                break;
            }
        }
        self.order = self
            .lines
            .iter()
            .filter_map(|line| match line {
                Line::Agent {
                    agent,
                    evidence: false,
                    ..
                } => Some(*agent),
                _ => None,
            })
            .collect();
        self.repos = repos;
        self.offset = self.fit_offset(selected, usize::from(height));
    }

    /// The lines of an agent: where its first line is and how many it has.
    pub(crate) fn rows_of(&self, agent: AgentId) -> Option<(usize, usize)> {
        let first = self.lines.iter().position(
            |line| matches!(line, Line::Agent { agent: a, evidence: false, .. } if *a == agent),
        )?;
        let count = 1 + usize::from(matches!(
            self.lines.get(first + 1),
            Some(Line::Agent { agent: a, evidence: true, .. }) if *a == agent
        ));
        Some((first, count))
    }

    fn fit_offset(&self, selected: Option<AgentId>, visible: usize) -> usize {
        let mut offset = self.offset.min(self.lines.len().saturating_sub(visible));
        if let Some((first, count)) = selected.and_then(|agent| self.rows_of(agent)) {
            if first < offset {
                offset = first.saturating_sub(1);
            } else if visible > 0 && first + count > offset + visible {
                offset = first + count - visible;
            }
        }
        offset
    }

    /// The agent `step` agents away from `from` in the view's order.
    pub(crate) fn step(&self, from: Option<AgentId>, forward: bool) -> Option<AgentId> {
        let index = from.and_then(|agent| self.order.iter().position(|a| *a == agent));
        match (index, forward) {
            (None, _) => self.order.first().copied(),
            (Some(index), true) => self.order.get(index + 1).copied(),
            (Some(index), false) => index
                .checked_sub(1)
                .and_then(|i| self.order.get(i).copied()),
        }
    }

    /// The first agent of the repository before or after the one of `from`.
    pub(crate) fn jump_repo(&self, from: Option<AgentId>, forward: bool) -> Option<AgentId> {
        // The first agent line of each section, in order.
        let mut starts: Vec<(usize, AgentId)> = Vec::new();
        let mut section = None;
        let mut current = None;
        for line in &self.lines {
            match line {
                Line::Repo(repo) | Line::Single(repo) => section = Some(*repo),
                Line::Elsewhere => section = Some(usize::MAX),
                Line::Agent {
                    agent,
                    evidence: false,
                    ..
                } => {
                    let Some(section) = section else { continue };
                    if starts.last().is_none_or(|(last, _)| *last != section) {
                        starts.push((section, *agent));
                    }
                    if Some(*agent) == from {
                        current = Some(starts.len() - 1);
                    }
                }
                _ => {}
            }
        }
        let target = match (current, forward) {
            (None, _) => 0,
            (Some(index), true) => index + 1,
            (Some(index), false) => index.checked_sub(1)?,
        };
        starts.get(target).map(|(_, agent)| *agent)
    }

    /// How many worktrees are safe to tidy.
    pub(crate) fn tidy_count(&self, registry: &Registry) -> usize {
        self.order
            .iter()
            .filter_map(|agent| registry.agent(*agent))
            .filter(|agent| tidy_of(agent) == Some(TidyVerdict::Allowed))
            .count()
    }
}

/// Groups the agents that pass `matches` by repository and worktree.
/// Returns the repositories, the agents outside any repository, and the
/// subagents shown under each parent.
fn group(
    registry: &Registry,
    matches: &dyn Fn(&Agent) -> bool,
) -> (Vec<Repo>, Vec<AgentId>, BTreeMap<AgentId, Vec<AgentId>>) {
    let shown: BTreeSet<AgentId> = registry
        .agents()
        .filter(|agent| matches(agent))
        .map(|agent| agent.id)
        .collect();
    let mut repos: BTreeMap<(String, Option<PathBuf>), Repo> = BTreeMap::new();
    let mut elsewhere = Vec::new();
    let mut subagents: BTreeMap<AgentId, Vec<AgentId>> = BTreeMap::new();
    for agent in registry.agents().filter(|agent| shown.contains(&agent.id)) {
        // A subagent shares its parent's session and folder: it goes under
        // its parent, when the parent is shown.
        if agent.role == Role::Subagent
            && let Some(parent) = agent.parent.filter(|parent| shown.contains(parent))
        {
            subagents.entry(parent).or_default().push(agent.id);
            continue;
        }
        let workspace = &agent.workspace;
        let Some(name) = workspace.repo.clone() else {
            elsewhere.push(agent.id);
            continue;
        };
        let root = workspace.repo_root.clone();
        let repo = repos
            .entry((name.to_lowercase(), root.clone()))
            .or_insert_with(|| Repo {
                name,
                root: root.clone(),
                worktrees: Vec::new(),
                agents: 0,
            });
        let path = workspace
            .worktree
            .clone()
            .or(root)
            .unwrap_or_else(|| workspace.cwd.clone());
        let main = workspace.worktree.is_none();
        if let Some(worktree) = repo.worktrees.iter_mut().find(|w| w.path == path) {
            worktree.agents.push(agent.id);
        } else {
            repo.worktrees.push(Worktree {
                branch: workspace.branch.clone(),
                base: workspace.base.clone(),
                path,
                main,
                agents: vec![agent.id],
            });
        }
    }
    let mut repos: Vec<Repo> = repos.into_values().collect();
    for repo in &mut repos {
        repo.worktrees
            .sort_by(|a, b| (!a.main, &a.branch, &a.path).cmp(&(!b.main, &b.branch, &b.path)));
        repo.agents = repo
            .worktrees
            .iter()
            .flat_map(|worktree| &worktree.agents)
            .map(|agent| 1 + subagents.get(agent).map_or(0, Vec::len))
            .sum();
    }
    (repos, elsewhere, subagents)
}

/// How the lines are laid out.
struct Shape<'a> {
    /// The detail level.
    density: Zoom,
    /// A blank line between repositories.
    blanks: bool,
    /// Whether an agent has something to show on an evidence line.
    has_evidence: &'a dyn Fn(AgentId) -> bool,
}

/// The lines of the view.
fn lines(
    repos: &[Repo],
    elsewhere: &[AgentId],
    subagents: &BTreeMap<AgentId, Vec<AgentId>>,
    shape: &Shape<'_>,
) -> Vec<Line> {
    let (density, blanks) = (shape.density, shape.blanks);
    let mut lines = Vec::new();
    let agent_lines = |lines: &mut Vec<Line>, agent: AgentId| {
        lines.push(Line::Agent {
            agent,
            sub: false,
            evidence: false,
        });
        if density == Zoom::Cards && (shape.has_evidence)(agent) {
            lines.push(Line::Agent {
                agent,
                sub: false,
                evidence: true,
            });
        }
        for sub in subagents.get(&agent).into_iter().flatten() {
            lines.push(Line::Agent {
                agent: *sub,
                sub: true,
                evidence: false,
            });
        }
    };
    for (index, repo) in repos.iter().enumerate() {
        if index > 0 && blanks {
            lines.push(Line::Blank);
        }
        if let [worktree] = repo.worktrees.as_slice() {
            lines.push(Line::Single(index));
            for agent in &worktree.agents {
                agent_lines(&mut lines, *agent);
            }
            continue;
        }
        lines.push(Line::Repo(index));
        for (place, worktree) in repo.worktrees.iter().enumerate() {
            lines.push(Line::Worktree(index, place));
            for agent in &worktree.agents {
                agent_lines(&mut lines, *agent);
            }
        }
    }
    if !elsewhere.is_empty() {
        if !repos.is_empty() && blanks {
            lines.push(Line::Blank);
        }
        lines.push(Line::Elsewhere);
        for agent in elsewhere {
            agent_lines(&mut lines, *agent);
        }
    }
    lines
}

/// The cleanup rule's verdict on an agent's worktree; `None` when there is
/// nothing to tidy: the main checkout, a subagent, no repository.
pub(crate) fn tidy_of(agent: &Agent) -> Option<TidyVerdict> {
    (agent.role != Role::Subagent && agent.workspace.worktree.is_some())
        .then(|| tidy_verdict(&agent.state, &agent.delivery))
}

/// The name of a detail level in the title.
pub(crate) const fn density_label(density: Zoom) -> &'static str {
    match density {
        Zoom::Cards => "full",
        Zoom::Compact => "compact",
        Zoom::Dots => "names",
    }
}

/// The view's title after its name: the detail level, the repositories and
/// what is safe to tidy.
pub(crate) fn title(model: &Model) -> Vec<Span<'static>> {
    let theme = &model.theme;
    let state = &model.repos;
    let dim = theme.style(StyleRole::TextDim);
    let mut density = density_label(state.density).to_owned();
    if state.zoom != ZoomChoice::Auto {
        density.push_str(" (fixed)");
    }
    // On a narrow panel the count of repositories makes room for the rest.
    let narrow = model.layout.view.is_some_and(|view| view.width < 70);
    let count = state.repos.len();
    let mut spans = vec![Span::styled(
        if narrow {
            format!(" · {density}")
        } else {
            format!(
                " · {density} · {count} {}",
                if count == 1 {
                    "repository"
                } else {
                    "repositories"
                }
            )
        },
        dim,
    )];
    let tidy = state.tidy_count(&model.registry);
    if tidy > 0 {
        spans.push(Span::styled(" · ", dim));
        spans.push(Span::styled(
            format!("{tidy} safe to tidy"),
            theme.style(StyleRole::StateDone),
        ));
    }
    spans
}

/// Draws the view in `area`, the inside of the view panel.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    let area = area.intersection(buf.area);
    if area.is_empty() {
        return;
    }
    let theme = &model.theme;
    let state = &model.repos;
    if state.lines.is_empty() {
        let message = if model.registry.agents().next().is_none() {
            "No agents yet: their repositories and worktrees will show here."
        } else {
            "No agent matches the filter."
        };
        put_spans(
            buf,
            area.x + 1,
            area.y,
            &[Span::styled(message, theme.style(StyleRole::Placeholder))],
            area.width.saturating_sub(1),
        );
        return;
    }
    for (row, line) in (0u16..).zip(state.lines.iter().skip(state.offset)) {
        if row >= area.height {
            break;
        }
        let rect = Rect::new(area.x, area.y + row, area.width, 1);
        draw_line(model, *line, rect, buf);
    }
    // Arrows on the panel's border tell that the list goes on.
    let arrow = theme.style(StyleRole::TextDim);
    let column = area.right().saturating_sub(2);
    if state.offset > 0
        && let Some(row) = area.y.checked_sub(1)
    {
        buf.set_string(column, row, "▲", arrow);
    }
    if state.offset + usize::from(area.height) < state.lines.len()
        && area.bottom() < buf.area.bottom()
    {
        buf.set_string(column, area.bottom(), "▼", arrow);
    }
}

/// Draws one line: its text at the left, its badges at the right when they
/// leave room for the text.
fn draw_line(model: &Model, line: Line, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    if let Line::Agent { agent, .. } = line
        && model.selected == Some(agent)
    {
        buf.set_style(area, theme.style(StyleRole::Selection));
        buf.set_string(area.x, area.y, "▌", theme.style(StyleRole::BorderFocused));
    }
    let Some((left, right)) = line_spans(model, line, area.width) else {
        return;
    };
    let right_width = width_of(&right);
    let room = area.width.saturating_sub(2);
    let (left_room, show_right) = if right_width > 0 && right_width + 4 <= room / 2 + room / 4 {
        (room.saturating_sub(right_width + 2), true)
    } else {
        (room, false)
    };
    put_spans(
        buf,
        area.x + 1,
        area.y,
        &truncated(&left, left_room),
        left_room,
    );
    if show_right {
        put_spans(
            buf,
            area.right().saturating_sub(right_width + 1),
            area.y,
            &right,
            right_width,
        );
    }
}

/// The text of a line, and the badges at its right.
type Halves = (Vec<Span<'static>>, Vec<Span<'static>>);

/// What a line `width` cells wide says, if anything.
fn line_spans(model: &Model, line: Line, width: u16) -> Option<Halves> {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let state = &model.repos;
    Some(match line {
        Line::Blank => return None,
        Line::Repo(index) => {
            let repo = &state.repos[index];
            let mut left = vec![Span::styled(
                repo.name.clone(),
                theme.style(StyleRole::Title),
            )];
            if let Some(root) = &repo.root {
                left.push(Span::styled(format!("  {}", root.display()), dim));
            }
            let right = vec![Span::styled(
                format!(
                    "{} · {}",
                    plural(repo.worktrees.len(), "worktree"),
                    plural(repo.agents, "agent")
                ),
                dim,
            )];
            (left, right)
        }
        Line::Single(index) => {
            let repo = &state.repos[index];
            let left = vec![
                Span::styled(repo.name.clone(), theme.style(StyleRole::Title)),
                Span::styled(" › ", dim),
            ];
            let right = vec![Span::styled(plural(repo.agents, "agent"), dim)];
            let room = width.saturating_sub(width_of(&right) + 4);
            (worktree_spans(&repo.worktrees[0], theme, left, room), right)
        }
        Line::Worktree(repo, index) => {
            let worktree = &state.repos[repo].worktrees[index];
            let room = width.saturating_sub(2);
            (
                worktree_spans(worktree, theme, vec![Span::raw("  ")], room),
                Vec::new(),
            )
        }
        Line::Elsewhere => (
            vec![
                Span::styled("Outside repositories", theme.style(StyleRole::Title)),
                Span::styled("  coordinators and agents in plain folders", dim),
            ],
            Vec::new(),
        ),
        Line::Agent {
            agent,
            sub,
            evidence,
        } => {
            let record = model.registry.agent(agent)?;
            if evidence {
                let mut left = vec![Span::raw("        ")];
                if record.workspace.repo.is_some() {
                    left.extend(evidence_spans(&record.delivery, theme));
                } else {
                    left.extend(mission_spans(model, record));
                }
                (left, tidy_spans(record, theme, true))
            } else {
                agent_line(model, record, sub)
            }
        }
    })
}

/// A coordinator's mission, as delivery: `running · 1 of 3 workers
/// delivered · 2 need you`, on the supervisor's verdicts.
fn mission_spans(model: &Model, agent: &Agent) -> Vec<Span<'static>> {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let Some(mission) = agent.mission.and_then(|id| model.registry.mission(id)) else {
        return vec![Span::styled("no mission", dim)];
    };
    let workers: Vec<&Agent> = model
        .registry
        .mission_agents(mission.id)
        .into_iter()
        .filter(|worker| worker.role == Role::Worker)
        .collect();
    let verdict = |worker: &&Agent, wanted: Verdict| {
        mission
            .assessments
            .get(&worker.id)
            .is_some_and(|assessment| assessment.verdict == wanted)
    };
    let delivered = workers
        .iter()
        .filter(|worker| verdict(worker, Verdict::Delivered))
        .count();
    let waiting = workers
        .iter()
        .filter(|worker| verdict(worker, Verdict::NeedsHuman))
        .count();
    let mut spans = vec![
        Span::styled(mission.status.label(), theme.style(StyleRole::Text)),
        Span::styled(
            format!(
                " · {delivered} of {} delivered",
                plural(workers.len(), "worker")
            ),
            dim,
        ),
    ];
    if waiting > 0 {
        spans.push(Span::styled(" · ", dim));
        spans.push(Span::styled(
            format!("{waiting} need{} you", if waiting == 1 { "s" } else { "" }),
            theme.style(StyleRole::StateNeedsInput),
        ));
    }
    spans
}

/// `spans` followed by a worktree's branch, its base and where it is,
/// within `room` cells: the end of the path, which names the worktree, is
/// kept.
fn worktree_spans(
    worktree: &Worktree,
    theme: &Theme,
    mut spans: Vec<Span<'static>>,
    room: u16,
) -> Vec<Span<'static>> {
    let dim = theme.style(StyleRole::TextDim);
    spans.push(Span::styled(
        worktree
            .branch
            .clone()
            .unwrap_or_else(|| "detached".to_owned()),
        theme.style(StyleRole::Text).add_modifier(Modifier::BOLD),
    ));
    if let Some(base) = &worktree.base {
        spans.push(Span::styled(format!(" from {base}"), dim));
    }
    if worktree.main {
        spans.push(Span::styled("  main checkout", dim));
    } else {
        let left = usize::from(room.saturating_sub(width_of(&spans) + 2));
        let path = worktree.path.display().to_string();
        spans.push(Span::styled(
            format!("  {}", text::truncate_start(&path, left)),
            dim,
        ));
    }
    spans
}

/// The width of spans, in cells.
fn width_of(spans: &[Span<'_>]) -> u16 {
    u16::try_from(
        spans
            .iter()
            .map(|span| text::width(&span.content))
            .sum::<usize>(),
    )
    .unwrap_or(u16::MAX)
}

/// An agent's name line, and the badges at its right: the supervisor's
/// verdict and, without an evidence line, the evidence and tidy state.
fn agent_line(model: &Model, agent: &Agent, sub: bool) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let density = model.repos.density;
    let letter = model
        .letters
        .of(agent.id)
        .map_or_else(|| " ".to_owned(), |letter| letter.to_string());
    let indent = if sub { "      " } else { "    " };
    let mut name_style = theme.role(agent.role);
    if model.selected == Some(agent.id) {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    let mut left = vec![
        Span::raw(indent),
        Span::styled(letter, theme.style(StyleRole::Letter)),
        Span::raw(" "),
        Span::styled(state_glyph(&agent.state), theme.state(&agent.state)),
        Span::raw(" "),
        Span::styled(agent.name.clone(), name_style),
    ];
    // On one line, the evidence takes the place of the role and mission.
    let one_line = density == Zoom::Compact && agent.workspace.repo.is_some() && !sub;
    if one_line {
        left.push(Span::raw("  "));
        left.extend(evidence_spans(&agent.delivery, theme));
    } else {
        left.push(Span::styled(format!(" · {}", agent.role), dim));
        if let Some(mission) = agent.mission.and_then(|id| model.registry.mission(id)) {
            left.push(Span::styled(format!(" · ◆ {}", mission.title), dim));
        }
    }
    if sub {
        return (left, Vec::new());
    }
    let mut right = verdict_spans(model, agent, density == Zoom::Cards).unwrap_or_default();
    if density != Zoom::Cards
        && let Some(TidyVerdict::Allowed) = tidy_of(agent)
    {
        if !right.is_empty() {
            right.push(Span::raw("  "));
        }
        right.extend(tidy_spans(agent, theme, false));
    }
    (left, right)
}

/// The mission supervisor's last verdict on a worker, `✓ delivered`, after
/// the word `verdict` when `word`.
fn verdict_spans(model: &Model, agent: &Agent, word: bool) -> Option<Vec<Span<'static>>> {
    let theme = &model.theme;
    let mission = agent.mission.and_then(|id| model.registry.mission(id))?;
    let assessment = mission.assessments.get(&agent.id)?;
    let (glyph, role) = match assessment.verdict {
        Verdict::Delivered => ("✓", StyleRole::StateDone),
        Verdict::NotDelivered => ("✗", StyleRole::ChecksFailing),
        Verdict::NeedsHuman => ("!", StyleRole::StateNeedsInput),
        Verdict::Failed => ("✗", StyleRole::StateFailed),
        Verdict::Lost => ("∅", StyleRole::StateLost),
    };
    let mut spans = Vec::new();
    if word {
        spans.push(Span::styled("verdict ", theme.style(StyleRole::TextDim)));
    }
    spans.push(Span::styled(
        format!("{glyph} {}", assessment.verdict.label()),
        theme.style(role),
    ));
    Some(spans)
}

/// Whether a worktree is safe to tidy, or why it is kept. With `reasons`,
/// every reason is spelled out.
fn tidy_spans(agent: &Agent, theme: &Theme, reasons: bool) -> Vec<Span<'static>> {
    match tidy_of(agent) {
        None => Vec::new(),
        Some(TidyVerdict::Allowed) => vec![Span::styled(
            "✓ safe to tidy",
            theme.style(StyleRole::StateDone),
        )],
        Some(TidyVerdict::Refused { reasons: why }) => {
            let dim = theme.style(StyleRole::TextDim);
            if !reasons {
                return vec![Span::styled("keep", dim)];
            }
            let words: Vec<&str> = why
                .iter()
                .map(|reason| match reason {
                    TidyRefusal::Busy => "busy",
                    TidyRefusal::UncommittedChanges => "uncommitted",
                    TidyRefusal::NotDelivered => "not merged",
                })
                .collect();
            vec![Span::styled(format!("keep: {}", words.join(", ")), dim)]
        }
    }
}

/// The delivery evidence, in full: `3 commits · +163 −43 · pushed · PR #42
/// open ✓`.
pub(crate) fn evidence_spans(delivery: &Delivery, theme: &Theme) -> Vec<Span<'static>> {
    let dim = theme.style(StyleRole::TextDim);
    let plain = theme.style(StyleRole::Text);
    let mut parts: Vec<Vec<Span<'static>>> = Vec::new();
    if delivery.commits > 0 {
        parts.push(vec![Span::styled(
            plural(usize::try_from(delivery.commits).unwrap_or(0), "commit"),
            plain,
        )]);
    }
    if delivery.diff.files > 0 {
        parts.push(vec![
            Span::styled(
                format!("+{}", delivery.diff.insertions),
                theme.style(StyleRole::DiffAdded),
            ),
            Span::raw(" "),
            Span::styled(
                format!("\u{2212}{}", delivery.diff.deletions),
                theme.style(StyleRole::DiffRemoved),
            ),
        ]);
    }
    if delivery.commits > 0 || delivery.pull_request.is_some() {
        parts.push(vec![match delivery.push {
            PushState::NoUpstream => Span::styled("not pushed", dim),
            PushState::UpToDate => Span::styled("pushed", plain),
            PushState::Ahead { commits } => Span::styled(
                format!("{commits} to push"),
                theme.style(StyleRole::ChecksPending),
            ),
        }]);
    }
    if let Some(pr) = &delivery.pull_request {
        let state = match pr.state {
            PrState::Draft => "draft",
            PrState::Open => "open",
            PrState::Merged => "merged",
            PrState::Closed => "closed",
        };
        let mut part = vec![Span::styled(
            format!("PR #{} {state}", pr.number),
            theme.style(StyleRole::BadgePr),
        )];
        let checks = match pr.checks {
            ChecksState::Passing => Some(("✓ checks", StyleRole::ChecksPassing)),
            ChecksState::Failing => Some(("✗ checks", StyleRole::ChecksFailing)),
            ChecksState::Pending => Some(("… checks", StyleRole::ChecksPending)),
            ChecksState::None => None,
        };
        if let Some((label, role)) = checks {
            part.push(Span::styled(format!(" {label}"), theme.style(role)));
        }
        parts.push(part);
    }
    let pr_merged = delivery
        .pull_request
        .as_ref()
        .is_some_and(|pr| pr.state == PrState::Merged);
    if delivery.merged_into_base && !pr_merged {
        parts.push(vec![Span::styled(
            "merged",
            theme.style(StyleRole::StateDone),
        )]);
    }
    if delivery.report_delivered {
        parts.push(vec![Span::styled(
            "report sent",
            theme.style(StyleRole::StateDone),
        )]);
    }
    if delivery.dirty {
        parts.push(vec![Span::styled(
            "uncommitted changes",
            theme.style(StyleRole::ChecksPending),
        )]);
    }
    if parts.is_empty() {
        return vec![Span::styled("nothing delivered yet", dim)];
    }
    let mut spans = Vec::new();
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", dim));
        }
        spans.extend(part);
    }
    spans
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use volee_core::time::Timestamp;

    use super::*;
    use crate::clock::ManualClock;
    use crate::demo::{
        DemoSource, EXPLORE_HANDLERS, INFRA, INVOICE_SCHEMA, INVOICING, TRACE_GATEWAY, agent_id,
    };

    fn start() -> Timestamp {
        Timestamp::from_millis(1_793_986_617_000)
    }

    fn registry() -> Registry {
        let demo = DemoSource::new(Arc::new(ManualClock::new(start())));
        Registry::from_events(&demo.events_until(start())).unwrap()
    }

    fn id(member: usize) -> AgentId {
        agent_id(start(), member)
    }

    #[test]
    fn agents_group_by_repository_and_worktree() {
        let registry = registry();
        let mut state = ReposState::default();
        state.relayout(&registry, &|_| true, 200, None);
        let names: Vec<&str> = state.repos.iter().map(|repo| repo.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "repositories are sorted by name");
        let gateway = state
            .repos
            .iter()
            .find(|repo| repo.name == "gateway")
            .unwrap();
        assert_eq!(gateway.worktrees.len(), 1);
        assert_eq!(gateway.worktrees[0].agents, [id(TRACE_GATEWAY)]);
        // Subagents sit under their parent, coordinators outside repositories.
        let position = |agent| state.order.iter().position(|a| *a == agent).unwrap();
        assert_eq!(
            position(id(EXPLORE_HANDLERS)),
            position(id(TRACE_GATEWAY)) + 1
        );
        assert!(state.lines.contains(&Line::Elsewhere));
        assert!(position(id(INVOICING)) > position(id(INVOICE_SCHEMA)));
        assert_eq!(state.density, Zoom::Cards);
        assert_eq!(state.rows_of(id(INVOICE_SCHEMA)).map(|(_, n)| n), Some(2));
    }

    #[test]
    fn the_detail_level_shrinks_to_fit_and_the_selection_stays_in_view() {
        let registry = registry();
        let mut state = ReposState::default();
        state.relayout(&registry, &|_| true, 22, Some(id(INVOICING)));
        assert_ne!(state.density, Zoom::Cards);
        let (first, count) = state.rows_of(id(INVOICING)).unwrap();
        assert!(first >= state.offset && first + count <= state.offset + 22);
        state.zoom = ZoomChoice::Fixed(Zoom::Cards);
        state.relayout(&registry, &|_| true, 22, Some(id(INVOICING)));
        assert_eq!(state.density, Zoom::Cards);
    }

    #[test]
    fn keys_walk_agents_and_repositories() {
        let registry = registry();
        let mut state = ReposState::default();
        state.relayout(&registry, &|_| true, 200, None);
        let first = state.order[0];
        assert_eq!(state.step(None, true), Some(first));
        assert_eq!(state.step(Some(first), false), None);
        assert_eq!(state.step(Some(first), true), Some(state.order[1]));
        let next_repo = state.jump_repo(Some(first), true).unwrap();
        assert_ne!(next_repo, first);
        assert_eq!(state.jump_repo(Some(next_repo), false), Some(first));
    }

    #[test]
    fn only_delivered_clean_worktrees_are_safe_to_tidy() {
        let registry = registry();
        let infra = registry.agent(id(INFRA)).unwrap();
        assert_eq!(tidy_of(infra), Some(TidyVerdict::Allowed));
        let schema = registry.agent(id(INVOICE_SCHEMA)).unwrap();
        assert!(matches!(tidy_of(schema), Some(TidyVerdict::Refused { .. })));
        let coordinator = registry.agent(id(INVOICING)).unwrap();
        assert_eq!(tidy_of(coordinator), None);
    }
}
