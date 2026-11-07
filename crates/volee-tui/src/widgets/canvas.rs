//! Drawing in scene coordinates, clipped to a window of the screen.
//!
//! The hierarchy is a scene larger than the screen; the canvas maps scene
//! cells to screen cells through an offset and silently drops what falls
//! outside the window, so cards can be partly visible while scrolling.

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::UnicodeWidthChar;

/// A window over a scene.
#[derive(Debug)]
pub(crate) struct Canvas<'a> {
    buf: &'a mut Buffer,
    area: Rect,
    /// The scene cell shown at the top left of `area`.
    origin: (i32, i32),
}

impl<'a> Canvas<'a> {
    /// A canvas that shows the scene from `origin` in `area`.
    pub(crate) fn new(buf: &'a mut Buffer, area: Rect, origin: (i32, i32)) -> Self {
        Self { buf, area, origin }
    }

    fn position(&self, x: i32, y: i32) -> Option<(u16, u16)> {
        let column = i32::from(self.area.x) + x - self.origin.0;
        let row = i32::from(self.area.y) + y - self.origin.1;
        let inside = column >= i32::from(self.area.x)
            && column < i32::from(self.area.right())
            && row >= i32::from(self.area.y)
            && row < i32::from(self.area.bottom());
        inside
            .then(|| Some((u16::try_from(column).ok()?, u16::try_from(row).ok()?)))
            .flatten()
    }

    fn cell(&mut self, x: i32, y: i32) -> Option<&mut Cell> {
        let position = self.position(x, y)?;
        self.buf.cell_mut(position)
    }

    /// Writes one symbol.
    pub(crate) fn put(&mut self, x: i32, y: i32, symbol: &str, style: Style) {
        if let Some(cell) = self.cell(x, y) {
            cell.set_symbol(symbol).set_style(style);
        }
    }

    /// The symbol at a scene cell, if it is visible.
    pub(crate) fn symbol(&self, x: i32, y: i32) -> Option<&str> {
        let position = self.position(x, y)?;
        self.buf.cell(position).map(Cell::symbol)
    }

    /// Paints a rectangle's style without changing its symbols.
    pub(crate) fn paint(&mut self, x: i32, y: i32, width: u16, height: u16, style: Style) {
        for row in y..y + i32::from(height) {
            for column in x..x + i32::from(width) {
                if let Some(cell) = self.cell(column, row) {
                    cell.set_style(style);
                }
            }
        }
    }

    /// Writes text from (`x`, `y`) on at most `max` cells; returns the cells
    /// used. Wide characters that do not fit are dropped.
    pub(crate) fn text(&mut self, x: i32, y: i32, text: &str, style: Style, max: u16) -> u16 {
        let mut used: u16 = 0;
        for c in text.chars() {
            let width = u16::try_from(c.width().unwrap_or(0)).unwrap_or(0);
            if width == 0 {
                continue;
            }
            if used + width > max {
                break;
            }
            let column = x + i32::from(used);
            let fits = (0..i32::from(width)).all(|dx| self.position(column + dx, y).is_some());
            if fits {
                let mut buffer = [0; 4];
                self.put(column, y, c.encode_utf8(&mut buffer), style);
                // Like ratatui, the cells under a wide character stay blank.
                for dx in 1..i32::from(width) {
                    self.put(column + dx, y, " ", style);
                }
            } else {
                // Half-visible wide character: show the visible half blank.
                for dx in 0..i32::from(width) {
                    self.put(column + dx, y, " ", style);
                }
            }
            used += width;
        }
        used
    }

    /// Writes styled spans one after another; returns the cells used.
    pub(crate) fn spans(&mut self, x: i32, y: i32, spans: &[Span<'_>], max: u16) -> u16 {
        let mut used = 0;
        for span in spans {
            used += self.text(
                x + i32::from(used),
                y,
                &span.content,
                span.style,
                max - used,
            );
            if used >= max {
                break;
            }
        }
        used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn text_is_clipped_to_the_window() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 2));
        let mut canvas = Canvas::new(&mut buf, Rect::new(1, 0, 4, 2), (2, 0));
        canvas.text(0, 0, "abcdefgh", Style::default(), 8);
        canvas.text(0, 5, "hidden", Style::default(), 6);
        assert_eq!(rows(&buf), [" cdef ", "      "]);
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 1));
        let mut canvas = Canvas::new(&mut buf, Rect::new(0, 0, 5, 1), (0, 0));
        assert_eq!(canvas.text(0, 0, "日本語", Style::default(), 5), 4);
        assert_eq!(canvas.symbol(0, 0), Some("日"));
        assert_eq!(canvas.symbol(2, 0), Some("本"));
        assert_eq!(canvas.symbol(4, 0), Some(" "));
    }
}
