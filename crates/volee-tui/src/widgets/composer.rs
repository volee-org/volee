//! The message composer: a window where the person writes to the selected
//! agent and chooses the kind of message. Its keys show on the hint line.
//!
//! ```text
//! ╭ Message to invoice-tools ─────────────────────────────────╮
//! │ in reply to "Should create_invoice accept a draft flag?"  │
//! │ kind  note  question  answer  contract  alert             │
//! │                                                           │
//! │ Yes, and keep it false by default.█                       │
//! ╰───────────────────────────────────────────────────────────╯
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};
use volee_core::Agent;
use volee_core::message::MessageKind;

use super::{panel, put_spans, truncated};
use crate::app::{Draft, MESSAGE_KINDS, Model};
use crate::text;
use crate::theme::StyleRole;

/// The widest the text gets, in cells.
const TEXT_WIDTH: u16 = 72;

/// Rows of text shown at least, and at most before the text scrolls.
const BODY_ROWS: (usize, usize) = (3, 12);

/// Draws the composer over `area`.
pub(crate) fn render(model: &Model, draft: &Draft, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let width = (TEXT_WIDTH + 4).min(area.width.saturating_sub(4));
    let text_width = width.saturating_sub(4).max(1);
    let field = &draft.field;
    let rows = text::wrap(field.text(), usize::from(text_width));
    let (cursor_row, cursor_column) = text::cursor_position(&rows, field.text(), field.cursor());
    let name = model.name_of(draft.to);
    let header = header(model, draft, &name);
    let body = rows
        .len()
        .max(cursor_row + 1)
        .clamp(BODY_ROWS.0, BODY_ROWS.1);
    // The borders, the header and a blank line, the text.
    let height = u16::try_from(2 + header.len() + 1 + body)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(4));
    if width < 12 || height < 5 {
        return;
    }
    let window = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Background));
    let mut title = vec![Span::raw(format!("Message to {name}"))];
    if draft.sending {
        title.push(Span::styled(" · sending…", theme.style(StyleRole::TextDim)));
    }
    let inner = panel(buf, window, title, true, theme);
    let x = inner.x + 1;
    let mut y = inner.y;

    for line in &header {
        put_spans(buf, x, y, &truncated(line, text_width), text_width);
        y += 1;
    }
    y += 1;

    let visible = usize::from(inner.bottom().saturating_sub(y));
    let offset = (cursor_row + 1).saturating_sub(visible);
    let text_style = theme.style(StyleRole::Text);
    if field.text().is_empty() {
        let hint = format!("Write to {name}. Markdown is fine.");
        put_spans(
            buf,
            x,
            y,
            &[Span::styled(hint, theme.style(StyleRole::Placeholder))],
            text_width,
        );
    }
    for (row, (start, end)) in (0u16..).zip(rows.iter().skip(offset).take(visible)) {
        buf.set_stringn(
            x,
            y + row,
            &field.text()[*start..*end],
            usize::from(text_width),
            text_style,
        );
    }
    if !draft.sending {
        let column = u16::try_from(cursor_column).unwrap_or(u16::MAX);
        let row = u16::try_from(cursor_row - offset).unwrap_or(u16::MAX);
        let cursor = theme
            .style(StyleRole::Input)
            .add_modifier(Modifier::REVERSED);
        if column <= text_width
            && let Some(cell) = buf.cell_mut((x + column, y + row))
        {
            cell.set_style(cursor);
        }
    }
}

/// The lines above the text: the question being answered, a warning when
/// the agent cannot receive messages, and the kinds of message.
fn header(model: &Model, draft: &Draft, name: &str) -> Vec<Vec<Span<'static>>> {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let mut lines = Vec::new();
    if let Some(question) = draft
        .question
        .filter(|_| draft.kind == MessageKind::Answer)
        .and_then(|id| model.registry.message(id))
    {
        let first_line = question.body.lines().next().unwrap_or_default();
        lines.push(vec![
            Span::styled("in reply to ", dim),
            Span::styled(format!("\"{first_line}\""), theme.style(StyleRole::Text)),
        ]);
    }
    if let Some(note) = model
        .registry
        .agent(draft.to)
        .and_then(|agent| unhanded(agent, name))
    {
        lines.push(vec![Span::styled(
            note,
            theme.style(StyleRole::Placeholder),
        )]);
    }
    let mut kinds = vec![Span::styled("kind ", dim)];
    for kind in MESSAGE_KINDS {
        let style = if kind == draft.kind {
            theme.style(StyleRole::Selection)
        } else {
            dim
        };
        kinds.push(Span::raw(" "));
        kinds.push(Span::styled(format!(" {} ", kind.label()), style));
    }
    lines.push(kinds);
    lines
}

/// What the composer says of an agent that nothing hands a message to, when
/// `agent` is one. Sessions started outside volee have no hooks to receive
/// messages: the person talks to them by opening them. An agent of another
/// harness that joined through MCP reads its messages itself. A program
/// started with `volee run` is typed into, and once it ended nothing reads
/// what is sent to it.
fn unhanded(agent: &Agent, name: &str) -> Option<String> {
    if agent.joined() {
        return Some(format!(
            "{name} joined through MCP: it reads this when it calls read_messages."
        ));
    }
    if agent.generic() {
        return Some(if agent.state.is_live() {
            format!(
                "{name} was started with volee run, which hands it no message: open it (enter) to talk to it."
            )
        } else {
            format!("{name} was started with volee run and ended: nothing resumes it to read this.")
        });
    }
    (!agent.managed()).then(|| {
        format!(
            "{name} was started outside volee, which hands it no message: open it (enter) to talk to it."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ulid::Ulid;
    use volee_core::harness::{HarnessKind, SessionKey, SessionOrigin};
    use volee_core::role::{LaunchedBy, Role};
    use volee_core::state::AgentState;
    use volee_core::time::Timestamp;
    use volee_core::workspace::{Location, Workspace};
    use volee_core::{AgentId, Event, EventKind, Registry};

    /// An agent as its registration, a session when it has one, and a state
    /// make it.
    fn agent(
        role: Role,
        launched_by: LaunchedBy,
        harness: HarnessKind,
        location: Location,
        session: Option<SessionKey>,
        state: AgentState,
    ) -> Agent {
        let id = AgentId::from_ulid(Ulid::from_parts(1, 1));
        let mut events = vec![EventKind::AgentRegistered {
            agent: id,
            name: "billing".into(),
            role,
            harness,
            launched_by,
            parent: None,
            mission: None,
            workspace: Workspace::default(),
            location,
        }];
        if let Some(session) = session {
            events.push(EventKind::SessionLinked {
                agent: id,
                session,
                origin: SessionOrigin::Discovered,
            });
        }
        events.push(EventKind::StateChanged {
            agent: id,
            state,
            detail: None,
        });
        let events: Vec<Event> = events
            .into_iter()
            .map(|kind| Event::new(Timestamp::from_millis(1), kind))
            .collect();
        Registry::from_events(&events)
            .unwrap()
            .agent(id)
            .unwrap()
            .clone()
    }

    #[test]
    fn the_composer_says_who_is_handed_no_message_and_why() {
        let host = || Location::Host { id: "h1".into() };
        let opencode = || HarnessKind::Other("opencode".into());
        let note = |agent: &Agent| unhanded(agent, "billing");

        // An agent volee launched or adopted is handed its messages.
        let launched = agent(
            Role::Lead,
            LaunchedBy::Human,
            HarnessKind::ClaudeCode,
            host(),
            Some(SessionKey::claude("s")),
            AgentState::Idle,
        );
        assert_eq!(note(&launched), None);

        // A program started with `volee run`: typed into while it runs.
        let run = |state| {
            agent(
                Role::Independent,
                LaunchedBy::Human,
                opencode(),
                host(),
                None,
                state,
            )
        };
        let running = note(&run(AgentState::Idle)).unwrap();
        assert!(running.contains("started with volee run"), "{running}");
        assert!(running.contains("open it (enter)"), "{running}");
        // Once it ended, nothing can be opened and nothing reads the
        // message.
        let ended = note(&run(AgentState::Stopped)).unwrap();
        assert!(ended.contains("nothing resumes it"), "{ended}");
        assert!(!ended.contains("open it"), "{ended}");

        // A session started outside volee, and an agent that joined through
        // MCP, keep their own words.
        let outside = agent(
            Role::Independent,
            LaunchedBy::Outside,
            opencode(),
            Location::Terminal,
            Some(SessionKey {
                harness: opencode(),
                id: "pid-1".into(),
            }),
            AgentState::Idle,
        );
        assert!(note(&outside).unwrap().contains("started outside volee"));
        let joined = agent(
            Role::Independent,
            LaunchedBy::Outside,
            HarnessKind::Other("codex".into()),
            Location::Unknown,
            None,
            AgentState::Idle,
        );
        assert!(note(&joined).unwrap().contains("joined through MCP"));
    }

    fn rows(text: &str, width: usize) -> Vec<&str> {
        text::wrap(text, width)
            .into_iter()
            .map(|(start, end)| &text[start..end])
            .collect()
    }

    #[test]
    fn lines_wrap_at_spaces_and_keep_breaks() {
        assert_eq!(
            rows("keep it false by default", 10),
            ["keep it", "false by", "default"]
        );
        assert_eq!(rows("one\n\ntwo\n", 10), ["one", "", "two", ""]);
        assert_eq!(rows("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
        assert_eq!(rows("exactly10c more", 10), ["exactly10c", "more"]);
        assert_eq!(rows("日本語", 4), ["日本", "語"]);
        assert_eq!(rows("", 10), [""]);
    }

    #[test]
    fn the_cursor_follows_the_rows() {
        let text = "keep it false";
        let wrapped = text::wrap(text, 10);
        assert_eq!(text::cursor_position(&wrapped, text, 0), (0, 0));
        assert_eq!(
            text::cursor_position(&wrapped, text, 7),
            (0, 7),
            "on the space"
        );
        assert_eq!(text::cursor_position(&wrapped, text, 8), (1, 0));
        assert_eq!(text::cursor_position(&wrapped, text, text.len()), (1, 5));
        let text = "abcdefghij";
        let wrapped = text::wrap(text, 5);
        assert_eq!(
            text::cursor_position(&wrapped, text, 5),
            (1, 0),
            "soft break"
        );
        let text = "one\n";
        let wrapped = text::wrap(text, 10);
        assert_eq!(text::cursor_position(&wrapped, text, 4), (1, 0));
    }
}
