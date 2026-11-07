//! The menu of programs to open an agent's worktree with: the ones found or
//! configured, each with its key, and how to add the others.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::{panel, put_spans, truncated};
use crate::app::Model;
use crate::keymap::Context;
use crate::text;
use crate::theme::StyleRole;
use crate::tools::{self, ToolKind, ToolMenu};

/// Draws the menu over `area`.
pub(crate) fn render(model: &Model, menu: ToolMenu, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let Some(agent) = model.registry.agent(menu.agent) else {
        return;
    };
    let width = 76.min(area.width.saturating_sub(4));
    let height = 10.min(area.height.saturating_sub(2));
    if width < 30 || height < 6 {
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
    let title = vec![Span::raw(format!("Open the worktree of {}", agent.name))];
    let inner = panel(buf, window, title, true, theme);
    let room = inner.width.saturating_sub(2);
    let dim = theme.style(StyleRole::TextDim);
    for (row, kind) in (0u16..).zip(ToolKind::ALL) {
        let key = model
            .keymap
            .first_key(Context::Tools, kind.action())
            .map(|key| key.to_string())
            .unwrap_or_default();
        let tool = model.settings.tools.iter().find(|tool| tool.kind == kind);
        let mut spans = vec![
            Span::styled(format!("{key:<3}"), theme.style(StyleRole::HintKey)),
            Span::styled(format!("{:<8}", kind.title()), theme.style(StyleRole::Text)),
        ];
        match tool {
            Some(tool) => {
                let command = tools::take_over(tool, agent);
                let mut line = command.program;
                for arg in command.args {
                    line.push(' ');
                    line.push_str(&arg);
                }
                spans.push(Span::styled(
                    tool.label.clone(),
                    theme.style(StyleRole::Input),
                ));
                spans.push(Span::styled(format!("  {line}"), dim));
            }
            None => spans.push(Span::styled(
                format!("none found; set one under [tui.tools] {}", config_key(kind)),
                theme.style(StyleRole::Placeholder),
            )),
        }
        put_spans(
            buf,
            inner.x + 1,
            inner.y + row,
            &truncated(&spans, room),
            room,
        );
    }
    let dir = tools::dir_of(agent).display().to_string();
    let place = format!(
        "in {}",
        text::truncate_start(&dir, usize::from(room.saturating_sub(3)))
    );
    put_spans(
        buf,
        inner.x + 1,
        inner.bottom().saturating_sub(1),
        &[Span::styled(place, dim)],
        room,
    );
}

/// The key of a kind under `[tui.tools]`.
const fn config_key(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Editor => "editor",
        ToolKind::Diff => "diff",
        ToolKind::Git => "git",
        ToolKind::Ide => "ide",
    }
}
