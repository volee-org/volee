//! Cards: one agent in the hierarchy, at three levels of detail.
//!
//! ```text
//! ╭─ invoice-schema ───── worker ─╮   ╭─ invoice-schema ─────╮   ! invoice-schema
//! │ ! needs you · 2 min  ▂▅▃▁▁▃▂▁ │   │ ! needs you · 2 min  │
//! │ billing › feat/invoices       │   ╰──────────────────────╯
//! │ Bash: bun run db:migrate      │
//! │ 2 commits · not pushed        │
//! ╰───────────────────────────────╯
//! ```
//!
//! Subagents hang under their parent's card as one-line satellites.

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use volee_core::delivery::{ChecksState, Delivery, PrState, PushState};
use volee_core::mission::Verdict;
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::StateCategory;
use volee_core::time::Elapsed;
use volee_core::{Agent, AgentState};

use super::canvas::Canvas;
use super::{state_glyph, truncated};
use crate::activity::{self, BUCKETS};
use crate::app::{Model, Motion};
use crate::text;
use crate::theme::StyleRole;
use crate::views::hierarchy::{SceneNode, Zoom};

/// The width of a full card.
pub(crate) const FULL_WIDTH: u16 = 32;

/// The width of a compact card.
pub(crate) const COMPACT_WIDTH: u16 = 24;

/// The longest name shown on a dot or a satellite.
pub(crate) const NAME_MAX: usize = 18;

/// What cards need from the model.
pub(crate) struct CardContext<'a> {
    model: &'a Model,
    spinner: char,
}

impl<'a> CardContext<'a> {
    /// The context of the current frame.
    pub(crate) fn new(model: &'a Model) -> Self {
        Self {
            model,
            spinner: model.spinner(),
        }
    }

    fn style(&self, role: StyleRole) -> Style {
        self.model.theme.style(role)
    }
}

/// How a card looks.
struct Look {
    border: Style,
    selected: bool,
    dimmed: bool,
}

impl Look {
    fn of(agent: &Agent, context: &CardContext<'_>) -> Self {
        let theme = &context.model.theme;
        let selected = context.model.selected == Some(agent.id);
        let dimmed = !context.model.matches(agent);
        let border = if dimmed {
            theme.style(StyleRole::TextDim)
        } else if selected {
            theme.style(StyleRole::CardSelected)
        } else {
            match agent.state.category() {
                StateCategory::NeedsInput | StateCategory::Failed => theme.state(&agent.state),
                StateCategory::Stopped | StateCategory::Lost => theme.style(StyleRole::TextDim),
                _ => theme.role(agent.role),
            }
        };
        Self {
            border,
            selected,
            dimmed,
        }
    }

    /// The border style of a node's own card.
    fn of_border(node: &SceneNode, context: &CardContext<'_>) -> Style {
        context
            .model
            .registry
            .agent(node.agent)
            .map_or_else(Style::default, |agent| Self::of(agent, context).border)
    }

    /// `style`, or the dim style when the card is filtered out.
    fn or_dim(&self, style: Style, context: &CardContext<'_>) -> Style {
        if self.dimmed {
            context.style(StyleRole::TextDim)
        } else {
            style
        }
    }
}

/// Draws a node: its card and its satellites.
pub(crate) fn draw(
    canvas: &mut Canvas<'_>,
    zoom: Zoom,
    node: &SceneNode,
    context: &CardContext<'_>,
) {
    let Some(agent) = context.model.registry.agent(node.agent) else {
        return;
    };
    let look = Look::of(agent, context);
    let (x, y) = (i32::from(node.place.x), i32::from(node.place.y));
    match zoom {
        Zoom::Cards => full(canvas, x, y, node.place.width, agent, &look, context),
        Zoom::Compact => compact(canvas, x, y, node.place.width, agent, &look, context),
        Zoom::Dots => dot(canvas, x, y, node.place.width, agent, &look, context),
    }
    satellites(canvas, zoom, node, context);
}

/// A rounded box, or a heavy one when selected, with the agent's name at
/// the left of its top border and `right` (role, spinner) at the right.
fn frame(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    (width, height): (u16, u16),
    look: &Look,
    (name, right): (Span<'_>, &[Span<'_>]),
    context: &CardContext<'_>,
) {
    let (w, h) = (i32::from(width), i32::from(height));
    let [
        top_left,
        top_right,
        bottom_left,
        bottom_right,
        horizontal,
        vertical,
    ] = if look.selected {
        ["┏", "┓", "┗", "┛", "━", "┃"]
    } else {
        ["╭", "╮", "╰", "╯", "─", "│"]
    };
    if look.selected {
        let fill = Style {
            bg: context.style(StyleRole::CardSelected).bg,
            ..Style::default()
        };
        canvas.paint(x, y, width, height, fill);
    }
    let border = look.border;
    for dx in 1..w - 1 {
        canvas.put(x + dx, y, horizontal, border);
        canvas.put(x + dx, y + h - 1, horizontal, border);
    }
    for dy in 1..h - 1 {
        canvas.put(x, y + dy, vertical, border);
        canvas.put(x + w - 1, y + dy, vertical, border);
    }
    canvas.put(x, y, top_left, border);
    canvas.put(x + w - 1, y, top_right, border);
    canvas.put(x, y + h - 1, bottom_left, border);
    canvas.put(x + w - 1, y + h - 1, bottom_right, border);

    // ╭─ name ─────── role ⠦ ─╮
    let right_width: usize = right.iter().map(|span| text::width(&span.content)).sum();
    let right_width = u16::try_from(right_width).unwrap_or(0);
    let right_room = if right_width > 0 { right_width + 2 } else { 0 };
    // At least one line cell between the name and the right part.
    let room = width.saturating_sub(7 + right_room);
    let name = Span::styled(
        text::truncate(&name.content, usize::from(room)).into_owned(),
        name.style,
    );
    canvas.put(x + 2, y, " ", border);
    let used = canvas.spans(x + 3, y, std::slice::from_ref(&name), room);
    canvas.put(x + 3 + i32::from(used), y, " ", border);
    if right_width > 0 {
        let at = x + w - 2 - i32::from(right_room);
        canvas.put(at, y, " ", border);
        canvas.spans(at + 1, y, right, right_width);
        canvas.put(at + i32::from(right_room) - 1, y, " ", border);
    }
}

/// Whether a card `width` wide shows the role of an agent named `name`. An
/// independent session, often started outside volee and named in several
/// words by Claude Code, gives the room of its long role to its name when
/// both do not fit: the name's color tells the role too, and no edge joins
/// it from above. Other roles keep theirs: their parent's edge meets the
/// middle of their top border, where a longer name would hide the junction.
fn shows_role(name: &str, role: Role, spinner: bool, width: u16) -> bool {
    if role != Role::Independent {
        return true;
    }
    // The top border as `frame` lays it out: the corners, the blanks around
    // the name and one line cell (7), then the right part and its blanks.
    let right = text::width(role.label()) + if spinner { 2 } else { 0 };
    text::width(name) + 7 + right + 2 <= usize::from(width)
}

/// Draws an agent alone as a compact card (its name and its state) with
/// its top left at (`x`, `y`): the network view's most detailed level.
pub(crate) fn draw_compact(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    agent: &Agent,
    context: &CardContext<'_>,
) {
    let look = Look::of(agent, context);
    compact(canvas, x, y, COMPACT_WIDTH, agent, &look, context);
}

/// The spinner of a working agent, as an extra to its state; none when the
/// person asked for reduced motion.
fn spinner(agent: &Agent, context: &CardContext<'_>) -> Option<Span<'static>> {
    let moving = matches!(agent.state, AgentState::Working | AgentState::Launching);
    (moving && context.model.motion == Motion::Full).then(|| {
        Span::styled(
            context.spinner.to_string(),
            context.model.theme.state(&agent.state),
        )
    })
}

fn full(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    width: u16,
    agent: &Agent,
    look: &Look,
    context: &CardContext<'_>,
) {
    let theme = &context.model.theme;
    let name_style = look.or_dim(theme.role(agent.role).add_modifier(Modifier::BOLD), context);
    let name = Span::styled(agent.name.as_str(), name_style);
    let spinner = spinner(agent, context);
    let mut right = Vec::new();
    if shows_role(&agent.name, agent.role, spinner.is_some(), width) {
        right.push(Span::styled(
            agent.role.label(),
            context.style(StyleRole::TextDim),
        ));
    }
    if let Some(spinner) = spinner {
        if !right.is_empty() {
            right.push(Span::raw(" "));
        }
        right.push(spinner);
    }
    frame(canvas, (x, y), (width, 6), look, (name, &right), context);

    let inner = width.saturating_sub(4);
    let left = x + 2;
    let state = state_spans(agent, context);
    let used = canvas.spans(left, y + 1, &dim_all(&state, look, context), inner);
    // The sparkline sits at the right of the state line when there is room.
    let counts = context.model.activity.counts(agent.id, context.model.now);
    let spark_width = u16::try_from(BUCKETS).unwrap_or(0);
    if used + 2 + spark_width <= inner && counts.iter().any(|count| *count > 0) {
        let spark_x = left + i32::from(inner - spark_width);
        for (dx, bar) in (0..).zip(activity::bars(&counts)) {
            let (symbol, style) = match bar {
                Some(bar) => (
                    bar,
                    look.or_dim(context.style(StyleRole::Sparkline), context),
                ),
                None => ('▁', context.style(StyleRole::Edge)),
            };
            let mut buffer = [0; 4];
            canvas.put(spark_x + dx, y + 1, symbol.encode_utf8(&mut buffer), style);
        }
    }
    let lines = [
        place_spans(agent, context),
        doing_spans(agent, context),
        fourth_line(agent, context),
    ];
    for (dy, line) in (2..).zip(&lines) {
        let line = truncated(&dim_all(line, look, context), inner);
        canvas.spans(left, y + dy, &line, inner);
    }
}

fn compact(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    width: u16,
    agent: &Agent,
    look: &Look,
    context: &CardContext<'_>,
) {
    let name_style = look.or_dim(
        context
            .model
            .theme
            .role(agent.role)
            .add_modifier(Modifier::BOLD),
        context,
    );
    let name = Span::styled(agent.name.as_str(), name_style);
    let right: Vec<Span<'_>> = spinner(agent, context).into_iter().collect();
    frame(canvas, (x, y), (width, 3), look, (name, &right), context);
    let state = state_spans(agent, context);
    canvas.spans(
        x + 2,
        y + 1,
        &dim_all(&state, look, context),
        width.saturating_sub(4),
    );
}

fn dot(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    width: u16,
    agent: &Agent,
    look: &Look,
    context: &CardContext<'_>,
) {
    let theme = &context.model.theme;
    let name = text::truncate(&agent.name, NAME_MAX);
    let mut spans = vec![
        Span::styled(state_glyph(&agent.state), theme.state(&agent.state)),
        Span::raw(" "),
        Span::styled(name, theme.role(agent.role)),
    ];
    if look.selected {
        let selection = context.style(StyleRole::Selection);
        for span in &mut spans {
            span.style = span.style.patch(selection);
        }
    }
    canvas.spans(x, y, &dim_all(&spans, look, context), width);
}

/// Subagents under the card: `├─ ▸ explore-handlers  Grep "p95"`.
fn satellites(canvas: &mut Canvas<'_>, zoom: Zoom, node: &SceneNode, context: &CardContext<'_>) {
    if node.satellites.is_empty() {
        return;
    }
    let theme = &context.model.theme;
    let edge = context.style(StyleRole::Edge);
    let place = node.place;
    // Under a card's border, or under a dot's glyph.
    let x = i32::from(place.x) + if zoom == Zoom::Dots { 0 } else { 2 };
    let bottom = i32::from(place.y + node.card_height) - 1;
    if zoom != Zoom::Dots {
        let joint = match canvas.symbol(x, bottom) {
            Some("━") => "┯",
            _ => "┬",
        };
        let style = Look::of_border(node, context);
        canvas.put(x, bottom, joint, style);
    }
    let count = node.satellites.len();
    for (index, id) in node.satellites.iter().enumerate() {
        let Some(agent) = context.model.registry.agent(*id) else {
            continue;
        };
        let row = bottom + 1 + i32::try_from(index).unwrap_or(0);
        let branch = if index + 1 == count {
            "└─"
        } else {
            "├─"
        };
        canvas.text(x, row, branch, edge, 2);
        let look = Look::of(agent, context);
        let room = place
            .width
            .saturating_sub(u16::try_from(x - i32::from(place.x)).unwrap_or(0) + 3);
        let mut spans = vec![
            Span::styled(state_glyph(&agent.state), theme.state(&agent.state)),
            Span::raw(" "),
            Span::styled(
                text::truncate(&agent.name, NAME_MAX).into_owned(),
                theme.role(agent.role),
            ),
        ];
        // What the subagent does, when there is room for a few words of it.
        let used = 2 + text::width(&agent.name).min(NAME_MAX);
        if zoom == Zoom::Cards && usize::from(room) >= used + 14 {
            let doing = agent
                .last_activity
                .as_ref()
                .map(|activity| activity.summary.clone())
                .or_else(|| agent.detail.clone())
                .unwrap_or_default();
            spans.push(Span::styled(
                format!("  {doing}"),
                context.style(StyleRole::TextDim),
            ));
        }
        if look.selected {
            let selection = context.style(StyleRole::Selection);
            for span in &mut spans {
                span.style = span.style.patch(selection);
            }
        }
        let line = truncated(&dim_all(&spans, &look, context), room);
        canvas.spans(x + 3, row, &line, room);
    }
}

/// The spans dimmed when the card is filtered out.
fn dim_all<'a>(spans: &[Span<'a>], look: &Look, context: &CardContext<'_>) -> Vec<Span<'a>> {
    if !look.dimmed {
        return spans.to_vec();
    }
    let dim = context.style(StyleRole::TextDim);
    spans
        .iter()
        .map(|span| Span::styled(span.content.clone(), dim))
        .collect()
}

/// `! needs you · 2 min`
pub(crate) fn state_spans(agent: &Agent, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let theme = &context.model.theme;
    vec![
        Span::styled(
            format!("{} {}", state_glyph(&agent.state), agent.state.label()),
            theme.state(&agent.state),
        ),
        Span::styled(
            format!(
                " · {}",
                Elapsed::between(agent.state_since, context.model.now)
            ),
            theme.style(StyleRole::TextDim),
        ),
    ]
}

/// `billing › feat/invoices`, or the mission of a coordinator.
fn place_spans<'a>(agent: &'a Agent, context: &CardContext<'_>) -> Vec<Span<'a>> {
    let theme = &context.model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let mission = agent
        .mission
        .and_then(|mission| context.model.registry.mission(mission));
    if agent.role == Role::Coordinator
        && let Some(mission) = mission
    {
        return vec![
            Span::styled("◆ ", theme.role(Role::Coordinator)),
            Span::styled(mission.title.clone(), theme.style(StyleRole::Text)),
            Span::styled(format!(" · {}", mission.kind.label()), dim),
        ];
    }
    match (&agent.workspace.repo, &agent.workspace.branch) {
        (Some(repo), Some(branch)) => vec![
            Span::styled(repo.as_str(), theme.style(StyleRole::Text)),
            Span::styled(" › ", dim),
            Span::styled(branch.as_str(), theme.style(StyleRole::Text)),
        ],
        _ => vec![Span::styled(agent.workspace.label(), dim)],
    }
}

/// What the agent does or waits for.
fn doing_spans<'a>(agent: &'a Agent, context: &CardContext<'_>) -> Vec<Span<'a>> {
    let theme = &context.model.theme;
    let activity = agent.last_activity.as_ref().map(|a| a.summary.as_str());
    let detail = agent.detail.as_deref();
    let (text, style) = match agent.state.category() {
        StateCategory::NeedsInput | StateCategory::Failed => (
            detail.or(activity),
            theme.state(&agent.state).remove_modifier(Modifier::BOLD),
        ),
        StateCategory::Done | StateCategory::Idle => {
            (detail.or(activity), theme.style(StyleRole::Text))
        }
        _ => (activity.or(detail), theme.style(StyleRole::Text)),
    };
    let dim = theme.style(StyleRole::TextDim);
    match text {
        Some(text) => vec![Span::styled(text, style)],
        // volee hears what an agent does through the hooks of the agents it
        // launches: a session started elsewhere never reports any.
        None if agent.launched_by == LaunchedBy::Outside => {
            vec![Span::styled("activity not reported", dim)]
        }
        None => vec![Span::styled("no activity yet", dim)],
    }
}

/// Delivery for workers, progress for coordinators, scope for leads.
fn fourth_line(agent: &Agent, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let model = context.model;
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    match agent.role {
        Role::Lead => {
            let missions = model
                .registry
                .missions()
                .filter(|mission| {
                    mission.requested_by == Some(agent.id) && !mission.status.is_over()
                })
                .count();
            let agents = descendants(model, agent);
            vec![Span::styled(
                format!(
                    "{} · {}",
                    plural(missions, "mission"),
                    plural(agents, "agent")
                ),
                dim,
            )]
        }
        Role::Coordinator => {
            let mission = agent.mission.and_then(|id| model.registry.mission(id));
            let workers: Vec<&Agent> = agent
                .mission
                .map(|mission| model.registry.mission_agents(mission))
                .unwrap_or_default()
                .into_iter()
                .filter(|worker| worker.role == Role::Worker)
                .collect();
            // Delivery is the supervisor's verdict, checked on the evidence.
            let delivered = workers
                .iter()
                .filter(|worker| {
                    mission
                        .and_then(|mission| mission.assessments.get(&worker.id))
                        .is_some_and(|assessment| assessment.verdict == Verdict::Delivered)
                })
                .count();
            let waiting = workers
                .iter()
                .filter(|worker| worker.state.category() == StateCategory::NeedsInput)
                .count();
            let mut spans = vec![Span::styled(
                format!("{delivered}/{} delivered", workers.len()),
                dim,
            )];
            if waiting > 0 {
                spans.push(Span::styled(" · ", dim));
                spans.push(Span::styled(
                    format!("{waiting} need{} you", if waiting == 1 { "s" } else { "" }),
                    theme.style(StyleRole::StateNeedsInput),
                ));
            }
            spans
        }
        _ => delivery_spans(&agent.delivery, context),
    }
}

fn descendants(model: &Model, agent: &Agent) -> usize {
    model
        .registry
        .children(agent.id)
        .iter()
        .map(|child| 1 + descendants(model, child))
        .sum()
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// `3 commits · PR #42 ✓`: the delivery evidence, shortest first.
pub(crate) fn delivery_spans(delivery: &Delivery, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let theme = &context.model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let plain = theme.style(StyleRole::Text);
    let mut parts: Vec<Vec<Span<'static>>> = Vec::new();
    if delivery.commits > 0 {
        parts.push(vec![Span::styled(
            plural(usize::try_from(delivery.commits).unwrap_or(0), "commit"),
            plain,
        )]);
    }
    if let Some(pr) = &delivery.pull_request {
        let label = match pr.state {
            PrState::Draft => format!("draft PR #{}", pr.number),
            PrState::Open => format!("PR #{}", pr.number),
            PrState::Merged => format!("PR #{} merged", pr.number),
            PrState::Closed => format!("PR #{} closed", pr.number),
        };
        let mut part = vec![Span::styled(label, theme.style(StyleRole::BadgePr))];
        let checks = match pr.checks {
            ChecksState::Passing => Some(("✓", StyleRole::ChecksPassing)),
            ChecksState::Failing => Some(("✗", StyleRole::ChecksFailing)),
            ChecksState::Pending => Some(("…", StyleRole::ChecksPending)),
            ChecksState::None => None,
        };
        if let Some((glyph, role)) = checks {
            part.push(Span::styled(format!(" {glyph}"), theme.style(role)));
        }
        parts.push(part);
    } else if delivery.commits > 0 {
        parts.push(vec![match delivery.push {
            PushState::NoUpstream => Span::styled("not pushed", dim),
            PushState::UpToDate => Span::styled("pushed", plain),
            PushState::Ahead { commits } => Span::styled(
                format!("{commits} to push"),
                theme.style(StyleRole::ChecksPending),
            ),
        }]);
    }
    if delivery.merged_into_base && delivery.pull_request.is_none() {
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
        parts.push(vec![Span::styled("uncommitted", dim)]);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::INVOICE_PAGE;
    use crate::test_support::{id, model};

    fn doing(agent: &Agent, model: &Model) -> String {
        let context = CardContext::new(model);
        doing_spans(agent, &context)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn a_long_name_takes_the_room_of_the_role() {
        let long = "cockpit open launch failure";
        assert!(shows_role("broken-links", Role::Independent, true, 36));
        // A name Claude Code gave a session: its role gives way...
        assert!(!shows_role(long, Role::Independent, true, 36));
        assert!(shows_role(long, Role::Independent, false, 47));
        assert!(!shows_role(long, Role::Independent, false, 46));
        // ...but the role of an agent with a parent stays, whatever its name.
        assert!(shows_role(long, Role::Coordinator, true, 36));
        assert!(shows_role(long, Role::Worker, true, 20));
    }

    #[test]
    fn a_session_started_outside_volee_reports_no_activity() {
        let model = model(120, 40);
        let mut agent = model.registry.agent(id(INVOICE_PAGE)).unwrap().clone();
        agent.last_activity = None;
        agent.detail = None;
        agent.state = AgentState::Working;
        agent.launched_by = LaunchedBy::Human;
        assert_eq!(doing(&agent, &model), "no activity yet");
        agent.launched_by = LaunchedBy::Outside;
        assert_eq!(doing(&agent, &model), "activity not reported");
    }
}
