//! The messages window: the person's threads on the left, under the
//! questions waiting for them, and the selected thread on the right.
//!
//! ```text
//! ╭ Messages · 1 question for you · 2 unread ─────────────────────────────────────╮
//! │▌? invoice-tools asks you     1 min │ Inbox                                    │
//! │    Should create_invoice accept…   │                                          │
//! │ ● Inbox                      1 min │ invoice-tools → you · question · 1 min   │
//! │    question · invoice-tools: Sh…   │ Should create_invoice accept a draft     │
//! │   Invoicing                  6 min │ flag, or always issue a final invoice?   │
//! ╰───────────────────────────────────────────────────────────────────────────────╯
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};
use volee_core::message::{Message, MessageKind, Party};
use volee_core::time::Elapsed;
use volee_core::{Registry, ThreadId};

use super::{panel, put_spans, truncated};
use crate::app::Model;
use crate::inbox::{self, InboxView, Row, SummaryState};
use crate::text;
use crate::theme::StyleRole;

/// The window on a screen of `size`: most of it, between the top bar and
/// the hint line.
fn window(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(160);
    let height = area.height.saturating_sub(4);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// A party's name as people read it.
fn name(registry: &Registry, party: Party) -> String {
    match party {
        Party::Human => "you".to_owned(),
        Party::System => "volee".to_owned(),
        Party::Agent(agent) => registry
            .agent(agent)
            .map_or_else(|| agent.short(), |agent| agent.name.clone()),
    }
}

/// A thread's name in the list: the mission's title; for a thread between
/// the person and one agent, that agent, from the person's side (the daemon
/// titles it for agents: "the human, api (worker)"); else the thread's.
fn title(registry: &Registry, thread: ThreadId) -> String {
    let Some(record) = registry.thread(thread) else {
        return thread.short();
    };
    if let Some(mission) = record.mission.and_then(|mission| registry.mission(mission)) {
        return format!("◆ {}", mission.title);
    }
    match record.participants.as_slice() {
        [Party::Human, Party::Agent(agent)] | [Party::Agent(agent), Party::Human] => {
            let name = registry
                .agent(*agent)
                .map_or_else(|| agent.short(), |agent| agent.name.clone());
            format!("You and {name}")
        }
        _ => record.title.clone(),
    }
}

/// The style of a message kind.
fn kind_role(kind: MessageKind) -> StyleRole {
    match kind {
        MessageKind::Question => StyleRole::StateNeedsInput,
        MessageKind::Alert => StyleRole::StateFailed,
        MessageKind::Answer | MessageKind::Report => StyleRole::StateDone,
        MessageKind::Contract | MessageKind::Note => StyleRole::TextDim,
    }
}

/// Draws the window over `area`.
pub(crate) fn render(model: &Model, view: &InboxView, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let registry = &model.registry;
    let window = window(area);
    if window.width < 40 || window.height < 8 {
        return;
    }
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Background));
    let dim = theme.style(StyleRole::TextDim);
    let counts = inbox::counts(registry);
    let mut header = vec![Span::raw("Messages")];
    if counts.questions > 0 {
        header.push(Span::styled(
            format!(
                " · {} question{} for you",
                counts.questions,
                if counts.questions == 1 { "" } else { "s" }
            ),
            theme.style(StyleRole::StateNeedsInput),
        ));
    }
    if counts.unread > 0 {
        header.push(Span::styled(format!(" · {} unread", counts.unread), dim));
    }
    let inner = panel(buf, window, header, true, theme);
    let rows = inbox::rows(registry);
    let current = view.current(&rows);
    let list_width = (inner.width * 2 / 5).clamp(28, 56).min(inner.width / 2);
    let list = Rect::new(inner.x, inner.y, list_width, inner.height);
    let thread = Rect::new(
        inner.x + list_width + 1,
        inner.y,
        inner.width.saturating_sub(list_width + 1),
        inner.height,
    );
    for y in inner.y..inner.bottom() {
        buf.set_string(inner.x + list_width, y, "│", theme.style(StyleRole::Border));
    }
    render_list(model, &rows, current, view.reading, list, buf);
    match current.and_then(|row| inbox::thread_of(registry, row).map(|thread| (row, thread))) {
        Some((row, id)) => render_thread(model, view, row, id, thread, buf),
        None => {
            put_spans(
                buf,
                thread.x + 1,
                thread.y,
                &[Span::styled(
                    "No messages yet. Questions from agents land here.",
                    theme.style(StyleRole::Placeholder),
                )],
                thread.width.saturating_sub(1),
            );
        }
    }
}

/// The rows: two lines each, the selected one marked.
fn render_list(
    model: &Model,
    rows: &[Row],
    current: Option<Row>,
    reading: bool,
    area: Rect,
    buf: &mut Buffer,
) {
    let theme = &model.theme;
    let registry = &model.registry;
    let dim = theme.style(StyleRole::TextDim);
    let visible = usize::from(area.height / 2);
    let index = current
        .and_then(|row| rows.iter().position(|r| *r == row))
        .unwrap_or(0);
    let offset = (index + 1).saturating_sub(visible);
    let room = area.width.saturating_sub(2);
    for (slot, row) in (0u16..).zip(rows.iter().skip(offset).take(visible)) {
        let y = area.y + slot * 2;
        let selected = Some(*row) == current;
        if selected {
            let style = if reading {
                theme.style(StyleRole::TextDim)
            } else {
                theme.style(StyleRole::Selection)
            };
            buf.set_style(Rect::new(area.x, y, area.width, 2), style);
            buf.set_string(area.x, y, "▌", theme.style(StyleRole::BorderFocused));
        }
        let (first, second, age) = match row {
            Row::Question(id) => {
                let Some(question) = registry.message(*id) else {
                    continue;
                };
                (
                    vec![
                        Span::styled("? ", theme.style(StyleRole::StateNeedsInput)),
                        Span::styled(name(registry, question.from), theme.style(StyleRole::Text)),
                        Span::styled(" asks you", dim),
                    ],
                    vec![Span::styled(
                        question.body.lines().next().unwrap_or_default().to_owned(),
                        theme.style(StyleRole::Text),
                    )],
                    question.at,
                )
            }
            Row::Thread(thread) => {
                let unread = inbox::unread(registry, Some(*thread)).len();
                let mark = if unread > 0 {
                    Span::styled("● ", theme.style(StyleRole::StateNeedsInput))
                } else {
                    Span::raw("  ")
                };
                let mut first = vec![
                    mark,
                    Span::styled(title(registry, *thread), theme.style(StyleRole::Text)),
                ];
                if unread > 1 {
                    first.push(Span::styled(format!(" ({unread})"), dim));
                }
                let messages = registry.thread_messages(*thread);
                let second = messages.last().map_or_else(
                    || vec![Span::styled("no messages yet", dim)],
                    |last| {
                        vec![
                            Span::styled(last.kind.label(), theme.style(kind_role(last.kind))),
                            Span::styled(format!(" · {}: ", name(registry, last.from)), dim),
                            Span::styled(
                                last.body.lines().next().unwrap_or_default().to_owned(),
                                dim,
                            ),
                        ]
                    },
                );
                (first, second, inbox::last_activity(registry, *thread))
            }
        };
        let age = Elapsed::between(age, model.now).to_string();
        let age_width = u16::try_from(text::width(&age)).unwrap_or(0);
        let title_room = room.saturating_sub(age_width + 1);
        put_spans(
            buf,
            area.x + 1,
            y,
            &truncated(&first, title_room),
            title_room,
        );
        buf.set_string(area.x + 1 + room - age_width, y, &age, dim);
        let mut second_line = vec![Span::raw("  ")];
        second_line.extend(second);
        put_spans(buf, area.x + 1, y + 1, &truncated(&second_line, room), room);
    }
}

/// The selected thread: its title, the summary when asked for, then its
/// messages, the latest at the bottom.
fn render_thread(
    model: &Model,
    view: &InboxView,
    row: Row,
    thread: ThreadId,
    area: Rect,
    buf: &mut Buffer,
) {
    let theme = &model.theme;
    let registry = &model.registry;
    let width = usize::from(area.width.saturating_sub(2));
    let x = area.x + 1;
    let mut y = area.y;
    put_spans(
        buf,
        x,
        y,
        &[Span::styled(
            title(registry, thread),
            theme.style(StyleRole::Title),
        )],
        area.width.saturating_sub(1),
    );
    y += 1;
    if let Some((asked, state)) = &view.summary
        && *asked == thread
    {
        let (text, style) = match state {
            SummaryState::Loading => (
                "Summarizing…".to_owned(),
                theme.style(StyleRole::Placeholder),
            ),
            SummaryState::Ready(summary) => (
                format!(
                    "Summary ({}, {} message{}): {}",
                    summary.model,
                    summary.messages,
                    if summary.messages == 1 { "" } else { "s" },
                    summary.text
                ),
                theme.style(StyleRole::Notice),
            ),
            SummaryState::Failed(error) => (
                format!("No summary: {error}"),
                theme.style(StyleRole::Placeholder),
            ),
        };
        let lines = text::wrap(&text, width);
        for (start, end) in lines.iter().take(6) {
            buf.set_stringn(x, y, &text[*start..*end], width, style);
            y += 1;
        }
    }
    y += 1;
    if y >= area.bottom() {
        return;
    }
    let highlighted = match row {
        Row::Question(id) => Some(id),
        Row::Thread(_) => None,
    };
    let lines = message_lines(model, &registry.thread_messages(thread), highlighted, width);
    let visible = usize::from(area.bottom() - y);
    let scroll = usize::from(view.scroll).min(lines.len().saturating_sub(visible));
    let end = lines.len() - scroll;
    let start = end.saturating_sub(visible);
    for (offset, line) in (0u16..).zip(&lines[start..end]) {
        put_spans(buf, x, y + offset, line, area.width.saturating_sub(1));
    }
    if start > 0 {
        buf.set_string(
            area.right().saturating_sub(2),
            area.y + 1,
            "▲",
            theme.style(StyleRole::TextDim),
        );
    }
}

/// The lines of a thread's messages: a header per message, its text
/// wrapped, a blank line between messages.
fn message_lines(
    model: &Model,
    messages: &[&Message],
    highlighted: Option<volee_core::MessageId>,
    width: usize,
) -> Vec<Vec<Span<'static>>> {
    let theme = &model.theme;
    let registry = &model.registry;
    let dim = theme.style(StyleRole::TextDim);
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    for message in messages {
        if !lines.is_empty() {
            lines.push(Vec::new());
        }
        let unread = message.from != Party::Human
            && message
                .receipt(Party::Human)
                .is_some_and(|receipt| receipt.read_at.is_none());
        let to: Vec<String> = message
            .recipients()
            .map(|party| name(registry, party))
            .collect();
        let mut header = Vec::new();
        if unread {
            header.push(Span::styled("● ", theme.style(StyleRole::StateNeedsInput)));
        }
        let sender = theme.style(StyleRole::Text).add_modifier(Modifier::BOLD);
        header.push(Span::styled(name(registry, message.from), sender));
        header.push(Span::styled(format!(" → {}", to.join(", ")), dim));
        header.push(Span::styled(" · ", dim));
        header.push(Span::styled(
            message.kind.label(),
            theme.style(kind_role(message.kind)),
        ));
        header.push(Span::styled(
            format!(" · {} ago", Elapsed::between(message.at, model.now)),
            dim,
        ));
        lines.push(header);
        let body_style = if highlighted == Some(message.id) {
            theme.style(StyleRole::Input)
        } else {
            theme.style(StyleRole::Text)
        };
        for (start, end) in text::wrap(&message.body, width) {
            lines.push(vec![Span::styled(
                message.body[start..end].to_owned(),
                body_style.patch(Style::default()),
            )]);
        }
        // The answers a question offers, with the digit that picks each,
        // until the person answered it.
        if !message.options.is_empty() && registry.answer_to(message.id).is_none() {
            let key = theme.style(StyleRole::HintKey);
            for (number, option) in (1..).zip(&message.options) {
                lines.push(vec![
                    Span::styled(format!("  {number} "), key),
                    Span::styled(
                        text::truncate(option, width.saturating_sub(4)).into_owned(),
                        theme.style(StyleRole::Text),
                    ),
                ]);
            }
            let hint = "press its number to answer, or reply in your own words";
            for (start, end) in text::wrap(hint, width.saturating_sub(2)) {
                lines.push(vec![Span::styled(format!("  {}", &hint[start..end]), dim)]);
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use volee_core::event::{Event, EventKind};
    use volee_core::harness::HarnessKind;
    use volee_core::message::Thread;
    use volee_core::role::{LaunchedBy, Role};
    use volee_core::time::Timestamp;
    use volee_core::workspace::{Location, Workspace};
    use volee_core::{AgentId, ThreadId};

    use super::*;

    fn opened(id: u128, title: &str, participants: Vec<Party>) -> Event {
        Event::new(
            Timestamp::from_millis(2),
            EventKind::ThreadOpened {
                thread: Thread {
                    id: ThreadId::from_ulid(ulid::Ulid::from_parts(2, id)),
                    title: title.to_owned(),
                    mission: None,
                    participants,
                    opened_at: Timestamp::from_millis(2),
                },
            },
        )
    }

    #[test]
    fn a_thread_with_one_agent_reads_from_the_person_s_side() {
        let api = AgentId::from_ulid(ulid::Ulid::from_parts(1, 1));
        let registry = Registry::from_events(&[
            Event::new(
                Timestamp::from_millis(1),
                EventKind::AgentRegistered {
                    agent: api,
                    name: "api".into(),
                    role: Role::Worker,
                    harness: HarnessKind::ClaudeCode,
                    launched_by: LaunchedBy::Human,
                    parent: None,
                    mission: None,
                    workspace: Workspace::at("/work/api"),
                    location: Location::Background,
                },
            ),
            opened(
                1,
                "the human, api (worker)",
                vec![Party::Human, Party::Agent(api)],
            ),
            opened(2, "Inbox", vec![Party::Human]),
            opened(3, "volee", vec![Party::System, Party::Human]),
        ])
        .unwrap();
        let thread = |id| ThreadId::from_ulid(ulid::Ulid::from_parts(2, id));
        assert_eq!(title(&registry, thread(1)), "You and api");
        assert_eq!(title(&registry, thread(2)), "Inbox");
        assert_eq!(title(&registry, thread(3)), "volee");
    }
}
