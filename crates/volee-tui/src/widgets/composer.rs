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
    // Sessions started outside volee have no hooks to receive messages until
    // they are adopted; an agent of another harness that joined through MCP
    // reads them itself.
    if let Some(agent) = model
        .registry
        .agent(draft.to)
        .filter(|agent| !agent.managed())
    {
        let note = if agent.joined() {
            format!("{name} joined through MCP: it reads this when it calls read_messages.")
        } else {
            format!("{name} was started outside volee: it receives messages once adopted (A).")
        };
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

#[cfg(test)]
mod tests {
    use super::*;

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
