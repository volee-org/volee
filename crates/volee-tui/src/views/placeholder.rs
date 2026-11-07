//! The views that arrive in the next milestone: a sketch and a sentence
//! about what they will show.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::app::{Model, Tab};
use crate::text;
use crate::theme::StyleRole;

fn sketch(tab: Tab) -> (&'static [&'static str], &'static str) {
    match tab {
        Tab::Network => (
            &[
                "●───────────●",
                "│ ╲       ╱ │",
                "│   ╲   ╱   │",
                "│     ●     │",
                "│   ╱   ╲   │",
                "●───────────●",
            ],
            "Who talks to whom: messages between agents, as a living graph.",
        ),
        Tab::Repos => (
            &[
                "main  ──●────●────●────●──",
                "           ╲              ",
                "feat        ●────●  PR #42",
            ],
            "Repositories and worktrees: branches, changes, and what is safe to tidy.",
        ),
        Tab::Timeline | Tab::Hierarchy => (
            &["──●──────●───●──────────●───▸"],
            "What happened when: every event, the newest first.",
        ),
    }
}

/// Draws the placeholder of `tab` in `area`.
pub(crate) fn render(model: &Model, tab: Tab, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let (drawing, sentence) = sketch(tab);
    let lines: Vec<(String, StyleRole)> = drawing
        .iter()
        .map(|line| ((*line).to_owned(), StyleRole::Edge))
        .chain([
            (String::new(), StyleRole::Text),
            (tab.title().to_owned(), StyleRole::Title),
            (sentence.to_owned(), StyleRole::Text),
            (
                "Coming in the next milestone.".to_owned(),
                StyleRole::Placeholder,
            ),
        ])
        .collect();
    let height = u16::try_from(lines.len()).unwrap_or(0);
    let top = area.y + area.height.saturating_sub(height) / 2;
    for (y, (line, role)) in (top..area.bottom()).zip(&lines) {
        let line = text::truncate(line, usize::from(area.width));
        let width = u16::try_from(text::width(&line)).unwrap_or(0);
        let x = area.x + area.width.saturating_sub(width) / 2;
        buf.set_string(x, y, &line, theme.style(*role));
    }
}
