//! The changes window: an agent's changes against its base branch, as a
//! summary (`git diff --stat`) or the full patch, colored by line.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::{panel, put_spans};
use crate::app::{DiffView, Model};
use crate::theme::{StyleRole, Theme};

/// The window for `lines` lines on a screen of `size`: as tall as the text,
/// up to the room between the top bar and the hint line.
fn window(size: (u16, u16), lines: usize) -> Rect {
    let (width, height) = size;
    let window_width = width.saturating_sub(4).min(140);
    let window_height = u16::try_from(lines)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .max(8)
        .min(height.saturating_sub(4));
    Rect::new(
        (width - window_width) / 2,
        (height - window_height) / 2,
        window_width,
        window_height,
    )
}

/// What the window says when the diff goes on beyond what it holds.
const TRUNCATED: &str = "… the diff goes on: see the rest in the agent's worktree.";

/// The lines the window shows.
fn lines(diff: &DiffView) -> Vec<&str> {
    match &diff.changes {
        Some(changes) if !changes.text.trim().is_empty() => {
            let mut lines: Vec<&str> = changes.text.lines().collect();
            if changes.truncated {
                lines.push(TRUNCATED);
            }
            lines
        }
        _ => Vec::new(),
    }
}

/// How far the window can scroll on a screen of `size`.
pub(crate) fn max_scroll(diff: &DiffView, size: (u16, u16)) -> u16 {
    let count = lines(diff).len();
    let visible = usize::from(window(size, count).height.saturating_sub(2));
    u16::try_from(count.saturating_sub(visible)).unwrap_or(u16::MAX)
}

/// Draws the window over `area`.
pub(crate) fn render(model: &Model, diff: &DiffView, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let lines = lines(diff);
    let window = window((area.width, area.height), lines.len()).intersection(area);
    if window.width < 10 || window.height < 3 {
        return;
    }
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Background));
    let dim = theme.style(StyleRole::TextDim);
    let mut title = vec![
        Span::raw("Changes"),
        Span::styled(format!(" · {}", model.name_of(diff.agent)), dim),
        Span::styled(
            if diff.full {
                " · full patch"
            } else {
                " · summary"
            },
            dim,
        ),
    ];
    if diff.loading {
        title.push(Span::styled(" · loading", dim));
    }
    let inner = panel(buf, window, title, true, theme);
    let x = inner.x + 1;
    let room = inner.width.saturating_sub(2);
    let message = match (&diff.changes, &diff.error) {
        (_, Some(error)) => Some(format!("No changes to show: {error}")),
        (Some(_), None) if lines.is_empty() => {
            Some("No changes against the base branch.".to_owned())
        }
        (None, None) => Some("Loading the changes…".to_owned()),
        (Some(_), None) => None,
    };
    if let Some(message) = message {
        put_spans(
            buf,
            x,
            inner.y,
            &[Span::styled(message, theme.style(StyleRole::Placeholder))],
            room,
        );
        return;
    }
    let visible = usize::from(inner.height);
    let scroll = usize::from(diff.scroll).min(lines.len().saturating_sub(visible));
    for (row, line) in (0u16..).zip(lines.iter().skip(scroll).take(visible)) {
        put_spans(buf, x, inner.y + row, &colored(line, theme), room);
    }
    if scroll + visible < lines.len() {
        buf.set_string(
            window.right().saturating_sub(3),
            window.bottom().saturating_sub(1),
            "▼",
            dim,
        );
    }
}

/// A line of a patch or of a summary, with its colors.
fn colored<'a>(line: &'a str, theme: &Theme) -> Vec<Span<'a>> {
    let style = |role| theme.style(role);
    if line.starts_with("diff ")
        || line.starts_with("index ")
        || line.starts_with("+++")
        || line.starts_with("---")
    {
        return vec![Span::styled(line, style(StyleRole::Title))];
    }
    if line.starts_with("@@") {
        return vec![Span::styled(line, style(StyleRole::TextDim))];
    }
    if line == TRUNCATED {
        return vec![Span::styled(line, style(StyleRole::Placeholder))];
    }
    if line.starts_with('+') {
        return vec![Span::styled(line, style(StyleRole::DiffAdded))];
    }
    if line.starts_with('-') {
        return vec![Span::styled(line, style(StyleRole::DiffRemoved))];
    }
    // A summary line, ` src/lib.rs | 12 ++++----`: the bar in colors.
    if let Some((file, graph)) = line.split_once(" | ")
        && let (count, bar) = graph.split_once(' ').unwrap_or((graph, ""))
        && bar.chars().all(|c| c == '+' || c == '-')
    {
        let plus = bar.len() - bar.trim_start_matches('+').len();
        return vec![
            Span::styled(file, style(StyleRole::Text)),
            Span::styled(" | ", style(StyleRole::TextDim)),
            Span::styled(count, style(StyleRole::Text)),
            Span::raw(if bar.is_empty() { "" } else { " " }),
            Span::styled(&bar[..plus], style(StyleRole::DiffAdded)),
            Span::styled(&bar[plus..], style(StyleRole::DiffRemoved)),
        ];
    }
    vec![Span::styled(line, style(StyleRole::Text))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Changes;

    fn texts(line: &str) -> Vec<String> {
        colored(line, &Theme::default())
            .into_iter()
            .map(|span| span.content.into_owned())
            .filter(|text| !text.is_empty())
            .collect()
    }

    #[test]
    fn summary_bars_split_into_additions_and_removals() {
        assert_eq!(
            texts(" src/lib.rs | 12 ++++----"),
            [" src/lib.rs", " | ", "12", " ", "++++", "----"]
        );
        assert_eq!(texts(" 2 files changed"), [" 2 files changed"]);
        assert_eq!(
            texts(" logo.png | Bin 0 -> 1234 bytes"),
            [" logo.png | Bin 0 -> 1234 bytes"]
        );
        assert_eq!(texts("+let total = 0;"), ["+let total = 0;"]);
    }

    #[test]
    fn the_window_scrolls_only_as_far_as_the_text_goes() {
        let mut diff = DiffView {
            agent: volee_core::AgentId::from_ulid(ulid::Ulid::from_parts(1, 1)),
            full: true,
            changes: Some(Changes {
                text: "line\n".repeat(30),
                truncated: false,
            }),
            error: None,
            loading: false,
            scroll: 0,
        };
        // 24 rows: a window of 20, 18 inside.
        assert_eq!(max_scroll(&diff, (80, 24)), 12);
        assert_eq!(max_scroll(&diff, (80, 60)), 0);
        // A cut diff says so on one more line.
        diff.changes.as_mut().unwrap().truncated = true;
        assert_eq!(max_scroll(&diff, (80, 24)), 13);
        assert_eq!(lines(&diff).last(), Some(&TRUNCATED));
    }
}
