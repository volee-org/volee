//! The theme picker: a small popup anchored to the bottom right of the
//! screen, like a which-key popup, listing the themes with the selected one
//! highlighted. The caller applies the selected theme to the whole screen
//! (live preview) and passes that same theme here.
//!
//! ```text
//!                      ╭ Theme ───────────────────╮
//!                      │   catppuccin-mocha       │
//!                      │ ▸ catppuccin-macchiato   │
//!                      │   catppuccin-frappe      │
//!                      │   catppuccin-latte       │
//!                      │   terminal               │
//!                      │ enter keep · esc cancel  │
//!                      ╰──────────────────────────╯
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::panel;
use crate::text;
use crate::theme::{StyleRole, Theme};

/// The hint at the bottom of the popup.
const FOOTER: &str = "enter keep · esc cancel";

/// Cells around the longest name: two borders, the marker and a space on
/// each side of it.
const CHROME: usize = 2 + 1 + 1 + 2;

/// Draws the picker at the bottom right of `area`.
///
/// `names` are the themes, `selected` the highlighted one, `theme` the theme
/// to draw with. The popup is as tall as the list plus its borders and the
/// footer, and as wide as the longest name; when `area` is too small the list
/// scrolls around the selection, and the footer goes first.
pub(crate) fn render<S: AsRef<str>>(
    names: &[S],
    selected: usize,
    theme: &Theme,
    area: Rect,
    buf: &mut Buffer,
) {
    let area = area.intersection(buf.area);
    let longest = names
        .iter()
        .map(|name| text::width(name.as_ref()))
        .max()
        .unwrap_or(0)
        .max(text::width(FOOTER));
    let width = u16::try_from(longest + CHROME)
        .unwrap_or(u16::MAX)
        .min(area.width);
    let wanted = u16::try_from(names.len() + 3).unwrap_or(u16::MAX);
    let height = wanted.min(area.height);
    if width < 8 || height < 3 {
        return;
    }
    let window = Rect::new(area.right() - width, area.bottom() - height, width, height);
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Background));
    let inner = panel(buf, window, vec![Span::raw("Theme")], true, theme);
    // The footer needs a row of its own besides one row of names.
    let footer = inner.height >= 2;
    let rows = usize::from(inner.height) - usize::from(footer);
    let first = first_visible(names.len(), selected, rows);
    let room = usize::from(inner.width).saturating_sub(1);
    for (row, (index, name)) in names.iter().enumerate().skip(first).take(rows).enumerate() {
        let y = inner.y + u16::try_from(row).unwrap_or(0);
        let chosen = index == selected;
        let style = if chosen {
            let style = theme.style(StyleRole::Selection);
            buf.set_style(Rect::new(inner.x, y, inner.width, 1), style);
            style
        } else {
            theme.style(StyleRole::Text)
        };
        let marker = if chosen { "▸" } else { " " };
        let label = text::truncate(name.as_ref(), room.saturating_sub(2));
        buf.set_stringn(inner.x + 1, y, format!("{marker} {label}"), room, style);
    }
    if footer {
        let hint = text::truncate(FOOTER, room);
        buf.set_stringn(
            inner.x + 1,
            inner.bottom() - 1,
            hint,
            room,
            theme.style(StyleRole::TextDim),
        );
    }
}

/// The index of the first name shown when `rows` of `count` fit, keeping
/// `selected` near the middle.
fn first_visible(count: usize, selected: usize, rows: usize) -> usize {
    let selected = selected.min(count.saturating_sub(1));
    selected
        .saturating_sub(rows / 2)
        .min(count.saturating_sub(rows))
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    const NAMES: [&str; 6] = [
        "catppuccin-mocha",
        "catppuccin-macchiato",
        "catppuccin-frappe",
        "catppuccin-latte",
        "terminal",
        "dusk",
    ];

    fn terminal(names: &[&str], selected: usize, width: u16, height: u16) -> Terminal<TestBackend> {
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(names, selected, &theme, area, frame.buffer_mut());
            })
            .unwrap();
        terminal
    }

    fn draw(names: &[&str], selected: usize, width: u16, height: u16) -> String {
        terminal(names, selected, width, height)
            .backend()
            .to_string()
    }

    #[test]
    fn the_popup_sits_at_the_bottom_right() {
        assert_snapshot!(draw(&NAMES, 1, 60, 14));
    }

    #[test]
    fn a_short_area_scrolls_around_the_selection() {
        assert_snapshot!(draw(&NAMES, 4, 40, 7));
    }

    #[test]
    fn a_tiny_area_drops_the_footer_then_the_popup() {
        assert_snapshot!(draw(&NAMES, 5, 30, 4));
        assert_snapshot!(draw(&NAMES, 0, 30, 3));
        assert_snapshot!(draw(&NAMES, 0, 30, 2));
        assert_snapshot!(draw(&NAMES, 0, 6, 10));
    }

    #[test]
    fn a_narrow_area_cuts_the_names() {
        assert_snapshot!(draw(&NAMES, 1, 16, 12));
    }

    #[test]
    fn the_selected_row_uses_the_selection_style() {
        let theme = Theme::default();
        let selection = theme.style(StyleRole::Selection);
        let terminal = terminal(&NAMES, 2, 60, 14);
        let buffer = terminal.backend().buffer();
        // The popup spans rows 5 to 13; its names start at row 6.
        let selected = buffer[(40, 8)].style();
        let other = buffer[(40, 7)].style();
        assert_eq!(selected.bg, selection.bg);
        assert_ne!(other.bg, selection.bg);
    }

    #[test]
    fn the_window_follows_the_selection() {
        assert_eq!(first_visible(6, 0, 3), 0);
        assert_eq!(first_visible(6, 3, 3), 2);
        assert_eq!(first_visible(6, 5, 3), 3);
        assert_eq!(first_visible(2, 1, 5), 0);
        assert_eq!(first_visible(0, 0, 3), 0);
    }
}
