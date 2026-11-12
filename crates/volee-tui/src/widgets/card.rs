//! Cards: one agent in the tree, at three levels of detail.
//!
//! A full card says what the agent is doing at a glance: each worker and
//! each session started on its own has a color of its own (border, name,
//! activity), leads and coordinators the color of their role, and the state
//! is a filled badge at the bottom.
//!
//! ```text
//! ╭─ trace-gateway ──── ⠏ ─╮   ╭─ invoicing ──────────── ─╮   ╭─ tower ─────────────────── ⠏ ─╮
//! │ gateway › audit/p95-l… │   │ ◆ Invoicing · change     │   │ Read missions/invoicing/pla… │
//! │ sonnet 5.5 · ◆ 2       │   │ Waiting for 3 workers    │   │ 2 missions · 9 agents        │
//! │ ▂▅▇▃▁▃▆█▅▂▁▃▅▇▆▃▂▁▃▅   │   │ ■■□ 1/3 delivered        │   │ platform › main · opus       │
//! │ ctx ▱▱▱▱▱▱▱▱▱▱▱▱▱▱ ?   │   │ ! 2 need you             │   │  WORKING  25 min             │
//! │ ⠏ Read src/middlewa…   │   │  IDLE  14 min            │   ╰──────────────────────────────╯
//! │ +31 −8      PR #42 ✓   │   ╰──────────────────────────╯
//! │  WORKING  11 min       │
//! ╰────────────────────────╯
//! ```
//!
//! Subagents hang under their parent's card as one-line satellites.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use volee_core::delivery::{ChecksState, Delivery, PrState};
use volee_core::mission::Verdict;
use volee_core::role::Role;
use volee_core::state::StateCategory;
use volee_core::time::Elapsed;
use volee_core::{Agent, AgentId, AgentState};

use super::canvas::Canvas;
use super::{state_glyph, truncated};
use crate::activity;
use crate::app::{Model, Motion};
use crate::text;
use crate::theme::{StyleRole, glow, mix};
use crate::views::hierarchy::{SceneNode, Zoom};

mod large;

/// The width of a compact card, and of every card in the network view.
pub(crate) const COMPACT_WIDTH: u16 = 24;

/// The longest name shown on a dot or a satellite.
pub(crate) const NAME_MAX: usize = 18;

/// The width and the height of an agent's card at a level of detail, its
/// satellites left out: a lead's card is the widest and the shortest, a
/// worker's the narrowest and the tallest, so that a mission's workers fit
/// side by side.
pub(crate) fn size(zoom: Zoom, role: Role, name: &str) -> (u16, u16) {
    match zoom {
        Zoom::Large => match role {
            Role::Lead => (44, 10),
            Role::Coordinator => (38, 11),
            Role::Worker | Role::Subagent | Role::Independent => (38, 14),
        },
        Zoom::Cards => match role {
            Role::Lead => (36, 6),
            Role::Coordinator => (28, 7),
            Role::Worker | Role::Subagent | Role::Independent => (24, 9),
        },
        Zoom::Compact => (COMPACT_WIDTH, 3),
        Zoom::Dots => (dot_width(name, 2), 1),
    }
}

/// The width of a dot label (`prefix` 2: glyph and space), or of a satellite
/// line under a dot (`prefix` 5: branch, space, glyph and space).
pub(crate) fn dot_width(name: &str, prefix: u16) -> u16 {
    prefix + u16::try_from(text::width(name).min(NAME_MAX)).unwrap_or(0)
}

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

    /// The model the frame is drawn from.
    pub(crate) const fn model(&self) -> &'a Model {
        self.model
    }

    /// The color of an agent: its own for a worker or a session started on
    /// its own, its role's for a lead or a coordinator, its parent's for a
    /// subagent.
    pub(crate) fn accent(&self, agent: &Agent) -> Style {
        let theme = &self.model.theme;
        match agent.role {
            Role::Lead | Role::Coordinator => theme.role(agent.role),
            Role::Subagent => {
                // The nearest ancestor that is not a subagent.
                let mut current = agent.parent;
                for _ in 0..16 {
                    let Some(parent) = current.and_then(|id| self.model.registry.agent(id)) else {
                        break;
                    };
                    if parent.role != Role::Subagent {
                        return self.accent(parent);
                    }
                    current = parent.parent;
                }
                theme.agent_color(agent.id)
            }
            Role::Worker | Role::Independent => theme.agent_color(agent.id),
        }
    }
}

/// How a card looks.
struct Look {
    /// The name, and what is drawn in the agent's color.
    accent: Style,
    /// The border: the agent's color, pulsing while the agent waits for the
    /// person or just failed, lavender when selected, flashing when the
    /// person writes to it.
    border: Style,
    /// Heavy lines: selected, or waiting for the person.
    heavy: bool,
    selected: bool,
    dimmed: bool,
}

/// How long a failed agent's border pulses before it stays red.
pub(crate) const FAILED_PULSE_MILLIS: i64 = 60_000;

/// The border of an agent's card.
fn border_of(agent: &Agent, accent: Style, selected: bool, context: &CardContext<'_>) -> Style {
    let model = context.model;
    let theme = &model.theme;
    let edge = theme.fg(StyleRole::Edge, Color::Reset);
    let moving = model.motion == Motion::Full;
    let now = model.now.as_millis();
    let state_color = theme.state(&agent.state).fg.unwrap_or(Color::Reset);
    let since = now - agent.state_since.as_millis();
    let mut border = match agent.state {
        AgentState::NeedsInput { .. } => {
            let level = if moving {
                0.35 + 0.65 * glow(now, 1_000)
            } else {
                1.0
            };
            Style::default().fg(mix(edge, state_color, level))
        }
        AgentState::Failed if moving && since < FAILED_PULSE_MILLIS => {
            Style::default().fg(mix(edge, state_color, 0.5 + 0.5 * glow(now, 600)))
        }
        _ if selected => Style::default().fg(theme.fg(StyleRole::CardSelected, state_color)),
        _ => accent.remove_modifier(Modifier::BOLD),
    };
    if selected {
        border = border.add_modifier(Modifier::BOLD);
    }
    let flash = model.pulses.flash(agent.id, model.now);
    if moving && flash > 0.0 {
        let current = border.fg.unwrap_or(edge);
        border.fg = Some(mix(current, theme.fg(StyleRole::CardFlash, current), flash));
    }
    border
}

impl Look {
    fn of(agent: &Agent, context: &CardContext<'_>) -> Self {
        let theme = &context.model.theme;
        let selected = context.model.selected == Some(agent.id);
        let dimmed = !context.model.matches(agent);
        let accent = if dimmed {
            theme.style(StyleRole::TextDim)
        } else {
            match agent.state.category() {
                // What calls for the person shows from afar.
                StateCategory::NeedsInput | StateCategory::Failed => theme.state(&agent.state),
                StateCategory::Stopped | StateCategory::Lost if !selected => {
                    theme.style(StyleRole::TextDim)
                }
                _ => context.accent(agent),
            }
        };
        let border = if dimmed {
            accent
        } else {
            border_of(agent, accent, selected, context)
        };
        Self {
            accent,
            border,
            heavy: selected || matches!(agent.state, AgentState::NeedsInput { .. }),
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
            .map_or_else(Style::default, |agent| Self::of(agent, context).border())
    }

    const fn border(&self) -> Style {
        self.border
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
    let width = node.place.width;
    match zoom {
        Zoom::Large => match agent.role {
            Role::Lead => large::lead(canvas, (x, y), width, agent, &look, context),
            Role::Coordinator => {
                large::coordinator(canvas, (x, y), width, agent, &look, context);
            }
            Role::Worker | Role::Subagent | Role::Independent => {
                large::worker(canvas, (x, y), width, agent, &look, context);
            }
        },
        Zoom::Cards => match agent.role {
            Role::Lead => lead(canvas, (x, y), width, agent, &look, context),
            Role::Coordinator => coordinator(canvas, (x, y), width, agent, &look, context),
            Role::Worker | Role::Subagent | Role::Independent => {
                worker(canvas, (x, y), width, agent, &look, context);
            }
        },
        Zoom::Compact => compact(canvas, x, y, width, agent, &look, context),
        Zoom::Dots => dot(canvas, x, y, width, agent, &look, context),
    }
    satellites(canvas, zoom, node, agent, context);
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

/// A rounded box, or a heavy one when selected, with the agent's name at
/// the left of its top border and `right` (model, spinner) at the right.
fn frame(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    (width, height): (u16, u16),
    look: &Look,
    (name, right): (&str, &[Span<'_>]),
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
    ] = if look.heavy {
        ["┏", "┓", "┗", "┛", "━", "┃"]
    } else {
        ["╭", "╮", "╰", "╯", "─", "│"]
    };
    if look.selected {
        let selected = context.style(StyleRole::CardSelected);
        let fill = Style {
            bg: selected.bg,
            ..Style::default()
        };
        canvas.paint(x, y, width, height, fill);
        // The marker at the left of the selected card.
        canvas.put(
            x - 1,
            y + 1,
            "▶",
            Style {
                bg: None,
                ..selected.add_modifier(Modifier::BOLD)
            },
        );
    }
    let border = look.border();
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

    // ╭─ name ─────── right ─╮
    let right_width: usize = right.iter().map(|span| text::width(&span.content)).sum();
    let right_width = u16::try_from(right_width).unwrap_or(0);
    let right_room = if right_width > 0 { right_width + 2 } else { 0 };
    // At least one line cell between the name and the right part.
    let room = width.saturating_sub(7 + right_room);
    let name = Span::styled(
        text::truncate(name, usize::from(room)).into_owned(),
        look.or_dim(look.accent.add_modifier(Modifier::BOLD), context),
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

/// The spinner of a working agent; none when the person asked for reduced
/// motion.
fn spinner(agent: &Agent, context: &CardContext<'_>) -> Option<Span<'static>> {
    let moving = matches!(agent.state, AgentState::Working | AgentState::Launching);
    (moving && context.model.motion == Motion::Full).then(|| {
        Span::styled(
            context.spinner.to_string(),
            context.model.theme.state(&agent.state),
        )
    })
}

/// The lines inside a card, from its first inner row, each cut to the
/// card's inner width.
fn lines(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    width: u16,
    rows: &[Vec<Span<'_>>],
    look: &Look,
    context: &CardContext<'_>,
) {
    let inner = width.saturating_sub(4);
    for (dy, line) in (1..).zip(rows) {
        let line = truncated(&dim_all(line, look, context), inner);
        canvas.spans(x + 2, y + dy, &line, inner);
    }
}

/// A worker, or a session started on its own: where it works, its model,
/// its activity, its context, what it does, what it changed, its state.
fn worker(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    width: u16,
    agent: &Agent,
    look: &Look,
    context: &CardContext<'_>,
) {
    let right: Vec<Span<'_>> = spinner(agent, context).into_iter().collect();
    frame(
        canvas,
        (x, y),
        (width, 9),
        look,
        (&agent.name, &right),
        context,
    );
    let inner = width.saturating_sub(4);
    let rows = [
        place_spans(agent, context),
        model_spans(agent, context),
        Vec::new(),
        context_spans(agent, inner, context),
        doing_spans(agent, context),
        Vec::new(),
        badge_spans(agent, context),
    ];
    lines(canvas, (x, y), width, &rows, look, context);
    activity_bars(canvas, (x + 2, y + 3), inner, agent, look, context);
    changes(canvas, (x + 2, y + 6), inner, agent, look, context);
}

/// A mission's coordinator: the mission, what it waits for, how far its
/// workers got, its state.
fn coordinator(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    width: u16,
    agent: &Agent,
    look: &Look,
    context: &CardContext<'_>,
) {
    let right: Vec<Span<'_>> = spinner(agent, context).into_iter().collect();
    frame(
        canvas,
        (x, y),
        (width, 7),
        look,
        (&agent.name, &right),
        context,
    );
    let (progress, waiting) = progress_spans(agent, context);
    let rows = [
        place_spans(agent, context),
        doing_spans(agent, context),
        progress,
        waiting,
        badge_spans(agent, context),
    ];
    lines(canvas, (x, y), width, &rows, look, context);
}

/// A lead: what it does, what it launched, where it runs, its state.
fn lead(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    width: u16,
    agent: &Agent,
    look: &Look,
    context: &CardContext<'_>,
) {
    let dim = context.style(StyleRole::TextDim);
    let mut right = Vec::new();
    if let Some(model) = agent.model.as_deref() {
        right.push(Span::styled(short_model(model), dim));
    }
    if let Some(spinner) = spinner(agent, context) {
        if !right.is_empty() {
            right.push(Span::raw(" "));
        }
        right.push(spinner);
    }
    frame(
        canvas,
        (x, y),
        (width, 6),
        look,
        (&agent.name, &right),
        context,
    );
    let rows = [
        doing_spans(agent, context),
        launched_spans(agent, context),
        place_spans(agent, context),
        badge_spans(agent, context),
    ];
    lines(canvas, (x, y), width, &rows, look, context);
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
    let right: Vec<Span<'_>> = spinner(agent, context).into_iter().collect();
    frame(
        canvas,
        (x, y),
        (width, 3),
        look,
        (&agent.name, &right),
        context,
    );
    lines(
        canvas,
        (x, y),
        width,
        &[badge_spans(agent, context)],
        look,
        context,
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
        Span::styled(name, context.accent(agent)),
    ];
    if look.selected {
        let selection = context.style(StyleRole::Selection);
        for span in &mut spans {
            span.style = span.style.patch(selection);
        }
    }
    canvas.spans(x, y, &dim_all(&spans, look, context), width);
}

/// Subagents under the card: `├─ ▸ explore-handlers  Grep "p95"`, in the
/// color of the agent they work for.
fn satellites(
    canvas: &mut Canvas<'_>,
    zoom: Zoom,
    node: &SceneNode,
    parent: &Agent,
    context: &CardContext<'_>,
) {
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
    let color = context.accent(parent);
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
            Span::styled(text::truncate(&agent.name, NAME_MAX).into_owned(), color),
        ];
        // What the subagent does, when there is room for a few words of it.
        let used = 2 + text::width(&agent.name).min(NAME_MAX);
        if matches!(zoom, Zoom::Cards | Zoom::Large) && usize::from(room) >= used + 14 {
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

/// The state as a filled badge, then how long it has lasted:
/// ` WORKING  11 min`.
pub(crate) fn badge_spans(agent: &Agent, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let theme = &context.model.theme;
    vec![
        badge(&agent.state, context),
        Span::styled(
            format!(
                " {}",
                Elapsed::between(agent.state_since, context.model.now)
            ),
            theme.style(StyleRole::TextDim),
        ),
    ]
}

/// ` WORKING `: the state's word on the state's color.
pub(crate) fn badge(state: &AgentState, context: &CardContext<'_>) -> Span<'static> {
    Span::styled(
        format!(" {} ", state.label().to_uppercase()),
        context
            .model
            .theme
            .state(state)
            .add_modifier(Modifier::REVERSED | Modifier::BOLD),
    )
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
        (Some(repo), Some(branch)) if agent.role != Role::Lead => vec![
            Span::styled(repo.as_str(), theme.style(StyleRole::Text)),
            Span::styled(" › ", dim),
            Span::styled(branch.as_str(), theme.style(StyleRole::Text)),
        ],
        // A lead works where it was started, not on a branch of its own.
        _ => vec![Span::styled(agent.workspace.label(), dim)],
    }
}

/// `sonnet 5.5 · ◆ 2`: the model, and the subagents at work.
fn model_spans(agent: &Agent, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let theme = &context.model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let mut spans = vec![match agent.model.as_deref() {
        Some(model) => Span::styled(short_model(model), dim),
        None => Span::styled("model ?", dim),
    }];
    let helpers = context
        .model
        .registry
        .children(agent.id)
        .iter()
        .filter(|child| child.role == Role::Subagent && child.state.is_live())
        .count();
    if helpers > 0 {
        spans.push(Span::styled(" · ", dim));
        spans.push(Span::styled(
            format!("◆ {helpers}"),
            theme.role(Role::Subagent),
        ));
    }
    spans
}

/// A model as people say it: `claude-sonnet-4-5-20250929` reads
/// `sonnet 4.5`, `opus[1m]` reads `opus`.
pub(crate) fn short_model(model: &str) -> String {
    let model = model.split('[').next().unwrap_or(model).trim();
    let model = model.strip_prefix("claude-").unwrap_or(model);
    let mut parts = model.split('-');
    let family = parts.next().unwrap_or(model);
    // Short numbers are the version; a long one is a date.
    let version: Vec<&str> = parts
        .take_while(|part| part.len() <= 2 && part.chars().all(|c| c.is_ascii_digit()))
        .collect();
    if version.is_empty() {
        family.to_owned()
    } else {
        format!("{family} {}", version.join("."))
    }
}

/// `ctx ▱▱▱▱▱▱▱▱▱▱▱▱▱ ?`: how much of its context the agent used, when its
/// harness says so; a question mark otherwise.
fn context_spans(agent: &Agent, inner: u16, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let theme = &context.model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let empty = theme.style(StyleRole::Edge);
    let bar = usize::from(inner.saturating_sub(9)).clamp(4, 30);
    let Some(percent) = agent.context else {
        return vec![
            Span::styled("ctx ", dim),
            Span::styled("▱".repeat(bar), empty),
            Span::styled(" ?", dim),
        ];
    };
    let filled = (usize::from(percent) * bar).div_ceil(100).min(bar);
    // Over four fifths, the conversation is about to be compacted.
    let color = if percent > 80 {
        theme.style(StyleRole::ChecksPending)
    } else {
        context.accent(agent)
    };
    vec![
        Span::styled("ctx ", dim),
        Span::styled("▰".repeat(filled), color),
        Span::styled("▱".repeat(bar - filled), empty),
        Span::styled(format!(" {percent}%"), dim),
    ]
}

/// The activity of the last minutes, one bucket per column, in the agent's
/// color while it works and dimmed once it stopped.
fn activity_bars(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    width: u16,
    agent: &Agent,
    look: &Look,
    context: &CardContext<'_>,
) {
    skyline(canvas, (x, y), (width, 1), agent, look, context);
}

/// An agent's skyline, `rows` high: a column per bucket of time, in the
/// agent's color while it worked, the color of what waits while it waited
/// for the person, red when it failed, faded once it stopped.
fn skyline(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    (width, rows): (u16, u16),
    agent: &Agent,
    look: &Look,
    context: &CardContext<'_>,
) {
    let model = context.model;
    let theme = &model.theme;
    let edge = theme.fg(StyleRole::Edge, Color::Reset);
    let rest = Style::default().fg(theme.fg(StyleRole::Border, edge));
    let accent = context.accent(agent).fg.unwrap_or(edge);
    let waiting = theme.style(StyleRole::StateNeedsInput).fg.unwrap_or(accent);
    let failed = theme.style(StyleRole::StateFailed).fg.unwrap_or(accent);
    // A finished agent's skyline fades: it tells what it did, not what it does.
    let over = matches!(
        agent.state.category(),
        StateCategory::Done | StateCategory::Stopped | StateCategory::Lost
    );
    let shade = |color: Color| match (over, theme.base()) {
        (true, Some(base)) => mix(color, base, 0.55),
        _ => color,
    };
    let rows = rows.max(1);
    for (dx, bar) in (0..).zip(
        model
            .activity
            .skyline(agent.id, model.now, usize::from(width)),
    ) {
        let style = match bar.phase {
            activity::Phase::Work => Style::default().fg(shade(mix(edge, accent, 0.85))),
            activity::Phase::Wait => Style::default().fg(shade(mix(edge, waiting, 0.7))),
            activity::Phase::Fail => Style::default().fg(shade(failed)),
            activity::Phase::Rest if bar.glyph == '▁' => rest,
            activity::Phase::Rest => Style::default().fg(shade(mix(edge, accent, 0.5))),
        };
        let style = look.or_dim(style, context);
        // The bar's height in eighths of a cell, spread over the rows from
        // the bottom one up.
        let level = activity::BARS
            .iter()
            .position(|glyph| *glyph == bar.glyph)
            .map_or(1, |index| index + 1);
        // The baseline stays a baseline however tall the skyline.
        let height = if level == 1 {
            1
        } else {
            level * usize::from(rows)
        };
        for row in 0..rows {
            let below = usize::from(row) * 8;
            let glyph = match height.saturating_sub(below) {
                0 => ' ',
                filled => activity::BARS[filled.min(8) - 1],
            };
            let mut buffer = [0; 4];
            canvas.put(
                x + dx,
                y + i32::from(rows - 1 - row),
                glyph.encode_utf8(&mut buffer),
                style,
            );
        }
    }
}

/// `+31 −8          PR #42 ✓`: the lines changed at the left, how the work
/// is delivered at the right.
fn changes(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    width: u16,
    agent: &Agent,
    look: &Look,
    context: &CardContext<'_>,
) {
    let left = diff_spans(agent, context);
    let left = dim_all(&left, look, context);
    let used = canvas.spans(x, y, &truncated(&left, width), width);
    let right = dim_all(&delivery_short(&agent.delivery, context), look, context);
    let right_width: u16 = right
        .iter()
        .map(|span| u16::try_from(text::width(&span.content)).unwrap_or(0))
        .sum();
    if used + 1 + right_width <= width {
        canvas.spans(x + i32::from(width - right_width), y, &right, right_width);
    }
}

/// The lines an agent changed (`+31 −8`, or `no changes`) and the messages
/// waiting for it (`✉ 1`).
fn diff_spans(agent: &Agent, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let theme = &context.model.theme;
    let diff = agent.delivery.diff;
    let mut left = if diff.insertions + diff.deletions > 0 {
        vec![
            Span::styled(
                format!("+{}", diff.insertions),
                theme.style(StyleRole::DiffAdded),
            ),
            Span::raw(" "),
            Span::styled(
                format!("\u{2212}{}", diff.deletions),
                theme.style(StyleRole::DiffRemoved),
            ),
        ]
    } else {
        vec![Span::styled("no changes", theme.style(StyleRole::TextDim))]
    };
    let waiting = context.model.registry.pending_for(agent.id).len();
    if waiting > 0 {
        left.push(Span::raw(" "));
        left.push(Span::styled(
            format!("\u{2709} {waiting}"),
            theme.style(StyleRole::CardMail),
        ));
    }
    left
}

/// The delivery in a few cells: the pull request, else the commits.
fn delivery_short(delivery: &Delivery, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let theme = &context.model.theme;
    if let Some(pr) = &delivery.pull_request {
        let label = match pr.state {
            PrState::Draft => format!("draft #{}", pr.number),
            PrState::Open => format!("PR #{}", pr.number),
            PrState::Merged => format!("#{} merged", pr.number),
            PrState::Closed => format!("#{} closed", pr.number),
        };
        let mut spans = vec![Span::styled(label, theme.style(StyleRole::BadgePr))];
        if let Some((glyph, role)) = checks(pr.checks) {
            spans.push(Span::styled(format!(" {glyph}"), theme.style(role)));
        }
        return spans;
    }
    if delivery.merged_into_base {
        return vec![Span::styled("merged", theme.style(StyleRole::StateDone))];
    }
    if delivery.commits > 0 {
        return vec![Span::styled(
            plural(usize::try_from(delivery.commits).unwrap_or(0), "commit"),
            theme.style(StyleRole::Text),
        )];
    }
    Vec::new()
}

fn checks(state: ChecksState) -> Option<(&'static str, StyleRole)> {
    match state {
        ChecksState::Passing => Some(("✓", StyleRole::ChecksPassing)),
        ChecksState::Failing => Some(("✗", StyleRole::ChecksFailing)),
        ChecksState::Pending => Some(("…", StyleRole::ChecksPending)),
        ChecksState::None => None,
    }
}

/// What the agent does or waits for, after a spinner while it works.
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
    let mut spans = Vec::new();
    if agent.state.category() == StateCategory::NeedsInput {
        spans.push(Span::styled("! ", theme.state(&agent.state)));
    } else if let Some(spinner) = spinner(agent, context) {
        spans.push(spinner);
        spans.push(Span::raw(" "));
    }
    match text {
        Some(text) => spans.push(Span::styled(text, style)),
        // volee hears what an agent does through the hooks of the agents it
        // launches or adopts: a session started elsewhere never reports any.
        None if !agent.managed() => spans.push(Span::styled("activity not reported", dim)),
        None => spans.push(Span::styled("no activity yet", dim)),
    }
    spans
}

/// `■■□ 1/3 delivered` and `! 2 need you`: how far a mission's workers got,
/// and how many wait for the person.
fn progress_spans(
    agent: &Agent,
    context: &CardContext<'_>,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let model = context.model;
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let mission = agent.mission.and_then(|id| model.registry.mission(id));
    let workers: Vec<&Agent> = agent
        .mission
        .map(|mission| model.registry.mission_agents(mission))
        .unwrap_or_default()
        .into_iter()
        .filter(|worker| worker.role == Role::Worker)
        .collect();
    let delivered = |worker: &Agent| {
        // Delivery is the supervisor's verdict, checked on the evidence.
        mission
            .and_then(|mission| mission.assessments.get(&worker.id))
            .is_some_and(|assessment| assessment.verdict == Verdict::Delivered)
    };
    let mut progress: Vec<Span<'static>> = workers
        .iter()
        .take(12)
        .map(|worker| {
            if delivered(worker) {
                Span::styled("■", theme.style(StyleRole::StateDone))
            } else if worker.state.category() == StateCategory::NeedsInput {
                Span::styled("■", theme.state(&worker.state))
            } else {
                Span::styled("□", dim)
            }
        })
        .collect();
    let done = workers.iter().filter(|worker| delivered(worker)).count();
    if !progress.is_empty() {
        progress.push(Span::raw(" "));
    }
    progress.push(Span::styled(
        format!("{done}/{} delivered", workers.len()),
        dim,
    ));
    let waiting = workers
        .iter()
        .filter(|worker| worker.state.category() == StateCategory::NeedsInput)
        .count();
    let waiting = if waiting > 0 {
        vec![Span::styled(
            format!(
                "! {waiting} need{} you",
                if waiting == 1 { "s" } else { "" }
            ),
            theme.style(StyleRole::StateNeedsInput),
        )]
    } else {
        let model = agent.model.as_deref().map(short_model);
        vec![Span::styled(model.unwrap_or_default(), dim)]
    };
    (progress, waiting)
}

/// `2 missions · 9 agents`: what a lead launched and that still runs.
fn launched_spans(agent: &Agent, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let model = context.model;
    let missions = model
        .registry
        .missions()
        .filter(|mission| mission.requested_by == Some(agent.id) && !mission.status.is_over())
        .count();
    let agents = descendants(model, agent.id);
    vec![Span::styled(
        format!(
            "{} · {}",
            plural(missions, "mission"),
            plural(agents, "agent")
        ),
        model.theme.style(StyleRole::TextDim),
    )]
}

fn descendants(model: &Model, agent: AgentId) -> usize {
    model
        .registry
        .children(agent)
        .iter()
        .map(|child| 1 + descendants(model, child.id))
        .sum()
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
    use super::*;
    use crate::demo::INVOICE_PAGE;
    use crate::test_support::{id, model};
    use volee_core::role::LaunchedBy;

    fn doing(agent: &Agent, model: &Model) -> String {
        let context = CardContext::new(model);
        doing_spans(agent, &context)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn a_session_started_outside_volee_reports_no_activity() {
        let model = model(120, 40);
        let mut agent = model.registry.agent(id(INVOICE_PAGE)).unwrap().clone();
        agent.last_activity = None;
        agent.detail = None;
        agent.state = AgentState::Idle;
        agent.launched_by = LaunchedBy::Human;
        assert_eq!(doing(&agent, &model), "no activity yet");
        agent.launched_by = LaunchedBy::Outside;
        assert_eq!(doing(&agent, &model), "activity not reported");
    }

    #[test]
    fn models_read_as_people_say_them() {
        assert_eq!(short_model("claude-sonnet-4-5-20250929"), "sonnet 4.5");
        assert_eq!(short_model("claude-opus-5-5"), "opus 5.5");
        assert_eq!(short_model("opus[1m]"), "opus");
        assert_eq!(short_model("haiku"), "haiku");
        assert_eq!(short_model("claude-fable-5-2"), "fable 5.2");
        assert_eq!(short_model("gpt-5"), "gpt 5");
    }

    #[test]
    fn every_role_has_a_card_that_fits_its_lines() {
        for role in [
            Role::Lead,
            Role::Coordinator,
            Role::Worker,
            Role::Independent,
            Role::Subagent,
        ] {
            let (width, height) = size(Zoom::Cards, role, "name");
            assert!(width >= 24 && height >= 6, "{role:?}");
        }
        // Five workers side by side, as a mission of three and one of two
        // with their gaps, fit in a tree 128 columns wide.
        let worker = size(Zoom::Cards, Role::Worker, "w").0;
        assert!(5 * worker + 4 * 2 <= 128);
    }

    /// The rows of a card drawn alone at the large zoom.
    fn large_card(model: &Model, member: usize) -> Vec<String> {
        let agent = model.registry.agent(id(member)).unwrap();
        let (width, height) = size(Zoom::Large, agent.role, &agent.name);
        let area = ratatui::layout::Rect::new(0, 0, width + 2, height);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        let mut canvas = Canvas::new(&mut buffer, area, (-1, 0));
        let node = SceneNode {
            agent: agent.id,
            place: crate::views::tree_layout::Placement {
                x: 0,
                y: 0,
                width,
                height,
                anchor: width / 2,
            },
            card_height: height,
            satellites: Vec::new(),
            children: Vec::new(),
            stacked: false,
        };
        draw(&mut canvas, Zoom::Large, &node, &CardContext::new(model));
        (0..height)
            .map(|y| {
                (1..=width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn large_cards_show_more_and_fit_their_frames() {
        let model = model(200, 50);
        for member in [
            crate::demo::TOWER,
            crate::demo::INVOICING,
            crate::demo::TRACE_GATEWAY,
        ] {
            let rows = large_card(&model, member);
            let last = rows.len() - 1;
            assert!(
                rows[0].starts_with('╭') || rows[0].starts_with('┏'),
                "{rows:#?}"
            );
            assert!(
                rows[last].starts_with('╰') || rows[last].starts_with('┗'),
                "{rows:#?}"
            );
            assert!(
                ["WORKING", "IDLE", "NEEDS YOU"]
                    .iter()
                    .any(|badge| rows[last - 1].contains(badge)),
                "the badge closes the card: {rows:#?}"
            );
        }
        let gateway = large_card(&model, crate::demo::TRACE_GATEWAY);
        let text = gateway.join("\n");
        assert!(text.contains("claude-code"), "the harness: {text}");
        assert!(text.contains("audit/p95"), "the whole place: {text}");
        // Two rows of skyline, and the activities before the current one.
        let bars = |row: &String| row.chars().filter(|c| activity::BARS.contains(c)).count();
        assert!(bars(&gateway[3]) > 0 && bars(&gateway[4]) > 0, "{text}");
        assert!(text.contains("Grep") || text.contains("Read"), "{text}");
        let coordinator = large_card(&model, crate::demo::INVOICING).join("\n");
        assert!(coordinator.contains("Invoicing"), "{coordinator}");
    }

    #[test]
    fn a_worker_keeps_its_color_and_a_lead_wears_its_role() {
        let model = model(120, 40);
        let context = CardContext::new(&model);
        let page = model.registry.agent(id(INVOICE_PAGE)).unwrap();
        assert_eq!(
            context.accent(page),
            model.theme.agent_color(page.id),
            "the same each time"
        );
        let lead = model
            .registry
            .agents()
            .find(|agent| agent.role == Role::Lead)
            .unwrap();
        assert_eq!(context.accent(lead), model.theme.role(Role::Lead));
    }

    fn border_of_card(model: &Model, agent: AgentId) -> Option<Color> {
        let context = CardContext::new(model);
        Look::of(model.registry.agent(agent).unwrap(), &context)
            .border()
            .fg
    }

    /// A message from the person to `to`, posted now in the demo's first
    /// thread, not delivered yet.
    fn written_to(model: &mut Model, to: AgentId) {
        let first = (*model.registry.unanswered_questions().first().unwrap()).clone();
        let message = volee_core::message::Message {
            id: volee_core::MessageId::from_ulid(ulid::Ulid::from_parts(u64::MAX >> 16, 7)),
            from: volee_core::message::Party::Human,
            body: "Use the draft flag.".into(),
            options: Vec::new(),
            in_reply_to: None,
            at: model.now,
            receipts: vec![volee_core::message::Receipt {
                to: volee_core::message::Party::Agent(to),
                delivered_at: None,
                read_at: None,
            }],
            ..first
        };
        let now = model.now;
        crate::app::update(
            model,
            crate::app::Msg::Events(vec![volee_core::Event::new(
                now,
                volee_core::event::EventKind::MessagePosted { message },
            )]),
        );
    }

    #[test]
    fn a_card_that_waits_for_the_person_pulses_and_one_written_to_flashes() {
        let mut model = model(200, 50);
        let schema = id(crate::demo::INVOICE_SCHEMA);
        let waiting = model.registry.agent(schema).unwrap().state.clone();
        assert!(matches!(waiting, AgentState::NeedsInput { .. }));
        let bright = model.theme.state(&waiting).fg;
        // A pulse of a second: dim at its start, the state's color half way.
        let second = model.now.as_millis() - model.now.as_millis().rem_euclid(1_000);
        model.now = volee_core::time::Timestamp::from_millis(second);
        let dim = border_of_card(&model, schema);
        model.now = volee_core::time::Timestamp::from_millis(second + 500);
        assert_eq!(border_of_card(&model, schema), bright);
        assert_ne!(dim, bright, "it pulses");
        // Reduced motion: it stays bright.
        model.motion = Motion::Reduced;
        model.now = volee_core::time::Timestamp::from_millis(second);
        assert_eq!(border_of_card(&model, schema), bright);
        model.motion = Motion::Full;

        let page = id(INVOICE_PAGE);
        let before = border_of_card(&model, page);
        written_to(&mut model, page);
        assert_eq!(
            border_of_card(&model, page),
            model.theme.style(StyleRole::CardFlash).fg,
            "it flashes when the person writes to it"
        );
        model.now = model.now.plus_millis(crate::pulses::FLASH_MILLIS);
        assert_eq!(border_of_card(&model, page), before, "then fades");
    }

    #[test]
    fn messages_waiting_for_an_agent_show_on_its_card() {
        let mut model = model(200, 50);
        let page = id(INVOICE_PAGE);
        written_to(&mut model, page);
        let area = ratatui::layout::Rect::new(0, 0, 200, 50);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        crate::views::hierarchy::render(&model, area, &mut buffer);
        let rect = model.scene.rect_of(page).unwrap();
        let (ox, oy) = model.scene_offset;
        let rows: Vec<String> = (rect.y..rect.bottom())
            .map(|y| {
                (rect.x..rect.right())
                    .filter_map(|x| {
                        let (cx, cy) = (i32::from(x) - ox, i32::from(y) - oy);
                        let (cx, cy) = (u16::try_from(cx).ok()?, u16::try_from(cy).ok()?);
                        Some(buffer[(cx, cy)].symbol().to_owned())
                    })
                    .collect()
            })
            .collect();
        assert!(
            rows.iter().any(|row| row.contains("\u{2709} 1")),
            "{rows:#?}"
        );
    }
}
