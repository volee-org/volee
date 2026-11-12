//! Large cards, the tree's most detailed level: everything a card shows,
//! more of it, what the agent did before its current action, and the last
//! message it sent or got.
//!
//! ```text
//! ╭─ trace-gateway ──────────────── ⠹ ─╮
//! │ gateway › audit/p95-latency          │
//! │ sonnet 4.5 · ◆ 2 · claude-code       │
//! │     ▃   ▅▄ ▆  ▄▅   ▄ ▅▆▄      ▄    │
//! │ ▂▃▅▄█▆▃▂█████▄▂▃██▅▃█████▃▂▃▅▄█▆▃▂ │
//! │ ctx ▰▰▰▰▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱ 26%  │
//! │ ⠹ Read src/middleware/auth.rs        │
//! │ Grep "p95" src/                      │
//! │ Read src/middleware/rate_limit.rs    │
//! │ ← latency-audit: check the p95 of…   │
//! │ +31 −8 ✉ 1                           │
//! │ 2 commits · pushed · PR #42 open     │
//! │  WORKING  11 min                     │
//! ╰──────────────────────────────────────╯
//! ```

use ratatui::text::Span;
use volee_core::Agent;
use volee_core::message::Party;

use super::{
    CardContext, Look, badge_spans, context_spans, diff_spans, doing_spans, frame, launched_spans,
    lines, model_spans, place_spans, progress_spans, short_model, skyline, spinner,
};
use crate::text;
use crate::theme::StyleRole;
use crate::widgets::canvas::Canvas;

/// A worker, or a session started on its own.
pub(super) fn worker(
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
        (width, 14),
        look,
        (&agent.name, &right),
        context,
    );
    let inner = width.saturating_sub(4);
    let [first, second] = earlier_spans(agent, context);
    let rows = [
        place_spans(agent, context),
        model_and_harness(agent, context),
        Vec::new(),
        Vec::new(),
        context_spans(agent, inner, context),
        doing_spans(agent, context),
        first,
        second,
        message_spans(agent, context),
        diff_spans(agent, context),
        delivery_spans(agent, context),
        badge_spans(agent, context),
    ];
    lines(canvas, (x, y), width, &rows, look, context);
    skyline(canvas, (x + 2, y + 3), (inner, 2), agent, look, context);
}

/// A mission's coordinator: its mission and goal, what it waits for, how
/// far its workers got.
pub(super) fn coordinator(
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
        (width, 11),
        look,
        (&agent.name, &right),
        context,
    );
    let inner = width.saturating_sub(4);
    let (progress, waiting) = progress_spans(agent, context);
    let [goal, goal_more] = goal_spans(agent, inner, context);
    let rows = [
        place_spans(agent, context),
        goal,
        goal_more,
        doing_spans(agent, context),
        progress,
        waiting,
        model_spans(agent, context),
        Vec::new(),
        badge_spans(agent, context),
    ];
    lines(canvas, (x, y), width, &rows, look, context);
    skyline(canvas, (x + 2, y + 8), (inner, 1), agent, look, context);
}

/// A lead: what it does and did, what it launched, where it runs, its last
/// message.
pub(super) fn lead(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    width: u16,
    agent: &Agent,
    look: &Look,
    context: &CardContext<'_>,
) {
    let dim = context.model().theme.style(StyleRole::TextDim);
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
        (width, 10),
        look,
        (&agent.name, &right),
        context,
    );
    let inner = width.saturating_sub(4);
    let [first, second] = earlier_spans(agent, context);
    let rows = [
        doing_spans(agent, context),
        first,
        second,
        launched_spans(agent, context),
        place_spans(agent, context),
        message_spans(agent, context),
        Vec::new(),
        badge_spans(agent, context),
    ];
    lines(canvas, (x, y), width, &rows, look, context);
    skyline(canvas, (x + 2, y + 7), (inner, 1), agent, look, context);
}

/// The model, the subagents at work and the harness: `sonnet 4.5 · ◆ 2 ·
/// claude-code`.
fn model_and_harness(agent: &Agent, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let dim = context.model().theme.style(StyleRole::TextDim);
    let mut spans = model_spans(agent, context);
    spans.push(Span::styled(format!(" · {}", agent.harness), dim));
    spans
}

/// The two activities before the current one, the latest first, dimmed.
fn earlier_spans(agent: &Agent, context: &CardContext<'_>) -> [Vec<Span<'static>>; 2] {
    let model = context.model();
    let dim = model.theme.style(StyleRole::TextDim);
    let mut earlier = model
        .activity
        .earlier(agent.id)
        .map(|summary| vec![Span::styled(summary.to_owned(), dim)]);
    [
        earlier.next().unwrap_or_default(),
        earlier.next().unwrap_or_default(),
    ]
}

/// The last message the agent sent (`→ to: …`) or got (`← from: …`).
fn message_spans(agent: &Agent, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let model = context.model();
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let me = Party::Agent(agent.id);
    let last = model
        .registry
        .messages()
        .filter(|message| message.from == me || message.recipients().any(|to| to == me))
        .max_by_key(|message| message.at);
    let Some(message) = last else {
        return vec![Span::styled("no messages yet", dim)];
    };
    let name = |party: Party| match party {
        Party::Agent(id) => model.name_of(id),
        Party::Human => "you".to_owned(),
        Party::System => "volee".to_owned(),
    };
    let (arrow, other) = if message.from == me {
        (
            "→ ",
            message.recipients().next().map_or_else(String::new, name),
        )
    } else {
        ("← ", name(message.from))
    };
    let body = message.body.lines().next().unwrap_or_default().to_owned();
    vec![
        Span::styled(arrow, dim),
        Span::styled(other, theme.style(StyleRole::Text)),
        Span::styled(": ", dim),
        Span::styled(body, dim),
    ]
}

/// How the work is delivered, in words: `2 commits · pushed · PR #42 open`,
/// or that there is nothing to deliver yet.
fn delivery_spans(agent: &Agent, context: &CardContext<'_>) -> Vec<Span<'static>> {
    let dim = context.model().theme.style(StyleRole::TextDim);
    let delivery = &agent.delivery;
    let nothing = delivery.commits == 0
        && delivery.pull_request.is_none()
        && !delivery.merged_into_base
        && !delivery.dirty;
    let text = if nothing {
        "no commits yet".to_owned()
    } else {
        delivery.summary()
    };
    vec![Span::styled(text, dim)]
}

/// A mission's goal on two lines, the second cut with `…` when it goes on.
fn goal_spans(agent: &Agent, width: u16, context: &CardContext<'_>) -> [Vec<Span<'static>>; 2] {
    let model = context.model();
    let dim = model.theme.style(StyleRole::TextDim);
    let Some(goal) = agent
        .mission
        .and_then(|id| model.registry.mission(id))
        .map(|mission| mission.goal.clone())
    else {
        return [Vec::new(), Vec::new()];
    };
    let goal = goal.split_whitespace().collect::<Vec<_>>().join(" ");
    let rows = text::wrap(&goal, usize::from(width));
    let line = |index: usize| -> Vec<Span<'static>> {
        let Some((start, end)) = rows.get(index) else {
            return Vec::new();
        };
        let mut piece = goal[*start..*end].to_owned();
        if index == 1 && rows.len() > 2 {
            piece = format!(
                "{}…",
                text::truncate(&piece, usize::from(width).saturating_sub(1))
            );
        }
        vec![Span::styled(piece, dim)]
    };
    [line(0), line(1)]
}
