//! What to do to open a session that runs in a terminal of its own
//! (ADR 0034): the person must press a key in another terminal, so the steps
//! are spelled out in a window, with where that terminal is, while volee
//! waits. The first sentence volee had for it, on the bottom line alone, did
//! not say where to go nor that the arrow was a key, and was not understood.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::{panel, put_spans};
use crate::app::{Model, Prompt};
use crate::keymap::{Action, Context};
use crate::text;
use crate::theme::StyleRole;

/// The columns of the text inside the window.
const TEXT_WIDTH: u16 = 62;

/// The rows of the text inside the window.
const TEXT_HEIGHT: u16 = 13;

/// Draws the window over `area` while volee waits for a session to leave
/// its terminal, when the terminal is large enough for it. A smaller one has
/// the bottom line alone ([`super::hints`]).
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    let Some(Prompt::AwaitMove(agent)) = model.prompt else {
        return;
    };
    let Some(agent) = model.registry.agent(agent) else {
        return;
    };
    let (width, height) = (TEXT_WIDTH + 4, TEXT_HEIGHT + 2);
    // The top bar and the two rows of hints stay in sight around it.
    if area.width < width + 2 || area.height < height + 4 {
        return;
    }
    let window = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let theme = &model.theme;
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Background));
    let title = vec![Span::raw(format!("Open {}", agent.name))];
    let inner = panel(buf, window, title, true, theme);

    let words = theme.style(StyleRole::Text);
    let dim = theme.style(StyleRole::TextDim);
    let step = theme.style(StyleRole::HintKey);
    let strong = theme.style(StyleRole::Input);
    let dir = agent.workspace.cwd.display().to_string();
    // "    claude, in " comes before it.
    let dir = text::truncate_start(&dir, usize::from(TEXT_WIDTH) - 15).into_owned();
    let cancel = model.keymap.key_of(Context::Confirm, Action::Back, "esc");
    let waiting = "volee is waiting for that key.";
    let gap = usize::from(TEXT_WIDTH)
        .saturating_sub(text::width(waiting) + text::width(&cancel) + " cancel".len());
    let plain = |line: &'static str| vec![Span::styled(line, words)];
    let lines: [Vec<Span<'_>>; TEXT_HEIGHT as usize] = [
        plain("It runs in a terminal of its own: only that terminal can show"),
        plain("it. Claude Code hands it over at one key, and nothing stops:"),
        Vec::new(),
        vec![
            Span::styled(" 1", step),
            Span::styled("  Go to the terminal where it runs:", words),
        ],
        vec![
            Span::styled("    claude, in ", words),
            Span::styled(dir, strong),
        ],
        vec![
            Span::styled(" 2", step),
            Span::styled("  Press the ", words),
            Span::styled("left arrow key", strong),
            Span::styled(" there, with nothing typed in its", words),
        ],
        vec![
            Span::styled("    input box. Claude Code shows ", words),
            Span::styled("← for agents", strong),
            Span::styled(" under it.", words),
        ],
        vec![
            Span::styled(" 3", step),
            Span::styled("  Come back here: volee opens it by itself.", words),
        ],
        Vec::new(),
        vec![Span::styled(
            "The conversation moves to Claude Code's background, where its",
            dim,
        )],
        vec![Span::styled(
            "work goes on. Its terminal still shows it: Escape twice there.",
            dim,
        )],
        Vec::new(),
        vec![
            Span::styled(waiting, words),
            Span::raw(" ".repeat(gap)),
            Span::styled(cancel, step),
            Span::styled(" cancel", theme.style(StyleRole::HintText)),
        ],
    ];
    for (row, line) in (0u16..).zip(&lines) {
        put_spans(buf, inner.x + 1, inner.y + row, line, TEXT_WIDTH);
    }
}
