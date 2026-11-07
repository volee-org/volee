//! A captured screen as terminal output: each row placed with a cursor move,
//! and the cursor left after the last row with text, at least as far right
//! as the widest row. A reader that sizes its emulated terminal from the
//! cursor moves, as the interface's preview does, then draws the rows where
//! the pane drew them, without wrapping them.

use std::fmt::Write as _;

/// `content` (rows separated by line feeds, with color sequences) as
/// terminal output that draws them.
pub(crate) fn redraw(content: &str) -> String {
    let content = content.strip_suffix('\n').unwrap_or(content);
    let rows: Vec<&str> = content
        .split('\n')
        .map(|row| row.strip_suffix('\r').unwrap_or(row))
        .collect();
    let mut output = String::with_capacity(content.len() + rows.len() * 12);
    let mut width = 1;
    let mut last_text = 1;
    for (number, row) in (1..).zip(&rows) {
        // Writing to a `String` cannot fail.
        let _ = write!(output, "\x1b[0m\x1b[{number};1H{row}");
        let (cells, text) = measure(row);
        width = width.max(cells);
        if text {
            last_text = number;
        }
    }
    let _ = write!(output, "\x1b[0m\x1b[{last_text};{width}H");
    output
}

/// An upper bound of the cells `row` takes, and whether it holds any text.
/// Escape sequences take no cell, ASCII characters one, others two (wide
/// characters do, and a few more cells only widen the emulated terminal).
fn measure(row: &str) -> (usize, bool) {
    let mut cells = 0;
    let mut text = false;
    let mut chars = row.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // A control sequence (`ESC [`) ends with its final byte, an
            // operating system command (`ESC ]`) with BEL or `ESC \`; any
            // other escape takes the one character after it.
            if chars.next_if_eq(&'[').is_some() {
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            } else if chars.next_if_eq(&']').is_some() {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        if c.is_control() {
            continue;
        }
        cells += if c.is_ascii() { 1 } else { 2 };
        text |= !c.is_whitespace();
    }
    (cells, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_placed_and_the_cursor_ends_after_the_text() {
        let output = redraw("ab\n\x1b[31mcd\x1b[m\n   \n");
        assert_eq!(
            output,
            "\x1b[0m\x1b[1;1Hab\
             \x1b[0m\x1b[2;1H\x1b[31mcd\x1b[m\
             \x1b[0m\x1b[3;1H   \
             \x1b[0m\x1b[2;3H"
        );
    }

    #[test]
    fn widths_ignore_escapes_and_count_wide_characters_twice() {
        assert_eq!(measure("\x1b[38;2;1;2;3mab\x1b[m"), (2, true));
        assert_eq!(measure("▐▛█"), (6, true));
        assert_eq!(measure("    "), (4, false));
        assert_eq!(measure("\x1b]0;title\x07x"), (1, true));
        assert_eq!(measure("\x1b]8;;https://x\x1b\\y"), (1, true));
        assert_eq!(measure(""), (0, false));
    }

    #[test]
    fn an_empty_capture_is_one_blank_row() {
        assert_eq!(redraw(""), "\x1b[0m\x1b[1;1H\x1b[0m\x1b[1;1H");
    }
}
