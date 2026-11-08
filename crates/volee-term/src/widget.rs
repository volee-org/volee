//! Draw a screen into a ratatui buffer, at its own size.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;

use crate::Frame;

/// Draws a [`Frame`] cell by cell (ADR 0021). The screen is never resized
/// for the view: when the area is smaller, the view keeps the rows from the
/// cursor down to the last drawn row in sight, and cuts columns on the right.
#[derive(Debug)]
pub struct FrameView<'a> {
    frame: &'a Frame,
}

impl<'a> FrameView<'a> {
    /// A view of `frame`.
    pub fn new(frame: &'a Frame) -> Self {
        Self { frame }
    }

    /// The first row of the frame shown in an area `height` rows high.
    pub fn first_row(&self, height: u16) -> usize {
        first_row(self.frame, usize::from(height))
    }

    /// Where the cursor lands in `area`, if it is shown.
    pub fn cursor_in(&self, area: Rect) -> Option<Position> {
        let (x, y) = self.frame.cursor?;
        let top = self.first_row(area.height);
        let y = usize::from(y).checked_sub(top)?;
        let y = u16::try_from(y).ok().filter(|y| *y < area.height)?;
        (x < area.width).then(|| Position::new(area.x + x, area.y + y))
    }
}

fn first_row(frame: &Frame, height: usize) -> usize {
    let total = frame.rows.len();
    if total <= height {
        return 0;
    }
    let cursor = frame.cursor.map(|(_, y)| usize::from(y));
    let bottom = frame
        .last_drawn_row()
        .into_iter()
        .chain(cursor)
        .max()
        .unwrap_or(0);
    let first = (bottom + 1).saturating_sub(height);
    // Never cut the cursor's row off the top.
    match cursor {
        Some(cursor) if cursor < first => cursor,
        _ => first,
    }
    .min(total - height)
}

impl Widget for FrameView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let top = self.first_row(area.height);
        for (dy, row) in self
            .frame
            .rows
            .iter()
            .skip(top)
            .take(usize::from(area.height))
            .enumerate()
        {
            let Ok(dy) = u16::try_from(dy) else { break };
            for (dx, cell) in row.iter().take(usize::from(area.width)).enumerate() {
                let Ok(dx) = u16::try_from(dx) else { break };
                let target = &mut buf[(area.x + dx, area.y + dy)];
                target.set_symbol(if cell.symbol.is_empty() { " " } else { &cell.symbol });
                target.set_style(cell.style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StyledCell;

    fn frame(lines: &[&str], cursor: Option<(u16, u16)>) -> Frame {
        Frame {
            rows: lines
                .iter()
                .map(|line| {
                    line.chars()
                        .map(|c| StyledCell {
                            symbol: c.to_string(),
                            style: ratatui::style::Style::default(),
                        })
                        .collect()
                })
                .collect(),
            cursor,
        }
    }

    #[test]
    fn a_small_area_keeps_the_bottom_of_what_is_drawn() {
        let f = frame(&["a", "b", "c", "> ", "status", "", ""], Some((2, 3)));
        assert_eq!(FrameView::new(&f).first_row(3), 2);
    }

    #[test]
    fn the_cursor_is_never_cut_off_the_top() {
        let f = frame(&["> ", "", "", "", "", "", "far below"], Some((2, 0)));
        assert_eq!(FrameView::new(&f).first_row(3), 0);
    }

    #[test]
    fn a_large_area_shows_everything() {
        let f = frame(&["a", "b"], None);
        assert_eq!(FrameView::new(&f).first_row(10), 0);
    }

    #[test]
    fn drawing_cuts_columns_and_places_the_cursor() {
        let f = frame(&["hello world", "> typed"], Some((7, 1)));
        let area = Rect::new(0, 0, 5, 2);
        let mut buf = Buffer::empty(area);
        let view = FrameView::new(&f);
        assert_eq!(view.cursor_in(area), None, "column 7 is cut");
        view.render(area, &mut buf);
        assert_eq!(buf, Buffer::with_lines(["hello", "> typ"]));
    }
}
