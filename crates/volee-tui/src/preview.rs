//! The preview: an agent's terminal screen, inside volee.
//!
//! Harnesses replay a session's screen as raw terminal output (for Claude
//! Code, `claude logs` at about 200x50, with cursor addressing and colors).
//! That output goes through a terminal emulator of that size, and the
//! resulting cells are drawn, cropped to the panel: the bottom of the screen
//! stays in view, since that is where the latest output and the prompt are.

use std::num::NonZeroU16;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::theme::{ColorDepth, StyleRole, Theme};

/// The size of the emulated terminal.
const ROWS: u16 = 50;
const COLUMNS: u16 = 200;

/// One cell of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewCell {
    symbol: String,
    style: Style,
    /// The right half of a wide character, drawn by its left half.
    continuation: bool,
}

/// A parsed screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PreviewScreen {
    /// Rows of cells, without trailing blanks.
    rows: Vec<Vec<PreviewCell>>,
    /// The last row worth showing: the cursor's or the last with text.
    anchor: u16,
}

impl PreviewScreen {
    /// Runs raw terminal output through the emulator.
    pub(crate) fn parse(output: &str, depth: ColorDepth) -> Self {
        let (Some(rows), Some(columns)) = (NonZeroU16::new(ROWS), NonZeroU16::new(COLUMNS)) else {
            return Self::default();
        };
        let mut parser = vt100::Parser::new(rows, columns, 0);
        parser.process(output.as_bytes());
        let screen = parser.screen();
        let mut parsed: Vec<Vec<PreviewCell>> = Vec::with_capacity(usize::from(ROWS));
        let mut last_text = 0;
        for row in 0..ROWS {
            let mut cells = Vec::new();
            for column in 0..COLUMNS {
                let Some(cell) = screen.cell(row, column) else {
                    break;
                };
                cells.push(PreviewCell {
                    symbol: if cell.has_contents() {
                        cell.contents().to_owned()
                    } else {
                        " ".to_owned()
                    },
                    style: style_of(cell, depth),
                    continuation: cell.is_wide_continuation(),
                });
            }
            while cells
                .last()
                .is_some_and(|cell| cell.symbol == " " && cell.style.bg.is_none())
            {
                cells.pop();
            }
            if !cells.is_empty() {
                last_text = row;
            }
            parsed.push(cells);
        }
        let (cursor_row, _) = screen.cursor_position();
        Self {
            rows: parsed,
            anchor: last_text.max(cursor_row).min(ROWS - 1),
        }
    }

    /// Draws the screen in `area`, bottom-anchored, `scroll` rows up.
    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, scroll: u16, theme: &Theme) {
        let base = theme.style(StyleRole::Text);
        let bottom = self.anchor.saturating_sub(scroll);
        let top = (bottom + 1).saturating_sub(area.height);
        for (y, row) in (area.y..area.bottom()).zip(top..=bottom) {
            let Some(cells) = self.rows.get(usize::from(row)) else {
                continue;
            };
            for (x, cell) in (area.x..area.right()).zip(cells) {
                if cell.continuation {
                    continue;
                }
                let style = base.patch(cell.style);
                if x + u16::try_from(crate::text::width(&cell.symbol)).unwrap_or(1) > area.right() {
                    break;
                }
                buf.set_stringn(x, y, &cell.symbol, usize::from(area.right() - x), style);
            }
        }
    }

    /// The text of the screen, for tests.
    #[cfg(test)]
    pub(crate) fn text(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| {
                row.iter()
                    .filter(|cell| !cell.continuation)
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .collect()
    }
}

fn color(color: vt100::Color, depth: ColorDepth) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(Color::Indexed(index)),
        vt100::Color::Rgb(r, g, b) => Some(depth.adapt(Color::Rgb(r, g, b))),
    }
}

fn style_of(cell: &vt100::Cell, depth: ColorDepth) -> Style {
    let mut style = Style {
        fg: color(cell.fgcolor(), depth),
        bg: color(cell.bgcolor(), depth),
        ..Style::default()
    };
    for (on, modifier) in [
        (cell.bold(), Modifier::BOLD),
        (cell.dim(), Modifier::DIM),
        (cell.italic(), Modifier::ITALIC),
        (cell.underline(), Modifier::UNDERLINED),
        (cell.inverse(), Modifier::REVERSED),
    ] {
        if on {
            style = style.add_modifier(modifier);
        }
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_become_styled_cells() {
        let screen = PreviewScreen::parse(
            "\x1b[2J\x1b[H\x1b[1mbold\x1b[0m \x1b[38;5;111mblue\x1b[0m\r\nnext",
            ColorDepth::TrueColor,
        );
        let text = screen.text();
        assert_eq!(text[0], "bold blue");
        assert_eq!(text[1], "next");
        assert_eq!(screen.rows[0][0].style.add_modifier, Modifier::BOLD);
        assert_eq!(screen.rows[0][5].style.fg, Some(Color::Indexed(111)));
        assert_eq!(screen.anchor, 1);
    }

    #[test]
    fn cursor_addressing_and_scrolling_are_honoured() {
        use std::fmt::Write as _;
        let mut output = String::from("\x1b[10;5Hhere");
        let screen = PreviewScreen::parse(&output, ColorDepth::TrueColor);
        assert_eq!(screen.text()[9], "    here");
        for line in 0..60 {
            write!(output, "\r\nline {line}").unwrap();
        }
        let screen = PreviewScreen::parse(&output, ColorDepth::TrueColor);
        assert_eq!(screen.text()[49], "line 59");
        assert_eq!(screen.anchor, 49);
    }

    #[test]
    fn rgb_colors_follow_the_color_depth() {
        let screen = PreviewScreen::parse("\x1b[38;2;255;0;0mred", ColorDepth::Ansi256);
        assert_eq!(screen.rows[0][0].style.fg, Some(Color::Indexed(196)));
    }

    #[test]
    fn the_bottom_of_the_screen_stays_in_view() {
        use std::fmt::Write as _;
        let mut output = String::new();
        for n in 0..20 {
            write!(output, "row {n}\r\n").unwrap();
        }
        let screen = PreviewScreen::parse(&output, ColorDepth::TrueColor);
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, 0, &Theme::default());
        let row = |buf: &Buffer, y: u16| -> String {
            (0..10).map(|x| buf[(x, y)].symbol().to_owned()).collect()
        };
        // The cursor sits on the empty row after "row 19".
        assert_eq!(row(&buf, 0), "row 18    ");
        assert_eq!(row(&buf, 1), "row 19    ");
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, 5, &Theme::default());
        assert_eq!(row(&buf, 2), "row 15    ");
    }
}
