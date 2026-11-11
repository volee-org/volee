//! Draw a screen into a ratatui buffer, at its own size.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;

use crate::{Frame, StyledCell};

/// Draws a [`Frame`] cell by cell (ADR 0021). The screen is never resized
/// for the view. When the area has fewer rows than the screen, runs of blank
/// rows shrink to one, then the view keeps the rows from the bottom of what
/// is drawn (or the cursor) upwards. Columns that do not fit are cut on the
/// right.
#[derive(Debug)]
pub struct FrameView<'a> {
    frame: &'a Frame,
}

impl<'a> FrameView<'a> {
    /// A view of `frame`.
    pub fn new(frame: &'a Frame) -> Self {
        Self { frame }
    }

    /// The frame's rows shown in an area `height` rows high, top to bottom.
    pub fn rows_shown(&self, height: u16) -> Vec<usize> {
        rows_shown(self.frame, usize::from(height))
    }

    /// Where the cursor lands in `area`, if it is shown.
    pub fn cursor_in(&self, area: Rect) -> Option<Position> {
        let (x, y) = self.frame.cursor?;
        let line = self
            .rows_shown(area.height)
            .iter()
            .position(|row| *row == usize::from(y))?;
        let line = u16::try_from(line).ok()?;
        (x < area.width).then(|| Position::new(area.x + x, area.y + line))
    }
}

fn is_blank(row: &[StyledCell]) -> bool {
    row.iter().all(StyledCell::shows_nothing)
}

fn rows_shown(frame: &Frame, height: usize) -> Vec<usize> {
    let total = frame.rows.len();
    if total <= height {
        return (0..total).collect();
    }
    let cursor = frame.cursor.map(|(_, y)| usize::from(y));
    let bottom = frame
        .last_drawn_row()
        .into_iter()
        .chain(cursor)
        .max()
        .unwrap_or(0)
        .min(total.saturating_sub(1));
    // Shrink runs of blank rows to one, but never drop the cursor's row.
    let mut rows: Vec<usize> = Vec::with_capacity(bottom + 1);
    let mut previous_blank = false;
    for index in 0..=bottom {
        let blank = is_blank(&frame.rows[index]) && Some(index) != cursor;
        if blank && previous_blank {
            continue;
        }
        previous_blank = blank;
        rows.push(index);
    }
    if rows.len() <= height {
        return rows;
    }
    let mut first = rows.len() - height;
    // Never cut the cursor's row off the top.
    if let Some(at) = cursor.and_then(|cursor| rows.iter().position(|row| *row == cursor))
        && at < first
    {
        first = at;
    }
    rows[first..first + height].to_vec()
}

impl Widget for FrameView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (dy, index) in self.rows_shown(area.height).into_iter().enumerate() {
            let Ok(dy) = u16::try_from(dy) else { break };
            let row = &self.frame.rows[index];
            for (dx, cell) in row.iter().take(usize::from(area.width)).enumerate() {
                let Ok(dx) = u16::try_from(dx) else { break };
                // The cell before it draws both halves of a wide character.
                if cell.continuation {
                    continue;
                }
                let target = &mut buf[(area.x + dx, area.y + dy)];
                target.set_symbol(if cell.symbol.is_empty() {
                    " "
                } else {
                    &cell.symbol
                });
                target.set_style(cell.style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(lines: &[&str], cursor: Option<(u16, u16)>) -> Frame {
        Frame {
            rows: lines
                .iter()
                .map(|line| {
                    line.chars()
                        .map(|c| StyledCell {
                            symbol: if c == ' ' {
                                String::new()
                            } else {
                                c.to_string()
                            },
                            style: ratatui::style::Style::default(),
                            continuation: false,
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
        assert_eq!(FrameView::new(&f).rows_shown(3), [2, 3, 4]);
    }

    #[test]
    fn blank_runs_shrink_before_anything_is_cut() {
        // A conversation at the top, the prompt at the bottom of a tall
        // screen: the gap between them shrinks to one row.
        let f = frame(
            &["answer", "", "", "", "", "", "> ", "status"],
            Some((2, 6)),
        );
        assert_eq!(FrameView::new(&f).rows_shown(4), [0, 1, 6, 7]);
    }

    #[test]
    fn the_cursor_is_never_cut_off_the_top() {
        let f = frame(&["> ", "x", "y", "z", "w", "v", "far below"], Some((2, 0)));
        assert_eq!(FrameView::new(&f).rows_shown(3), [0, 1, 2]);
    }

    #[test]
    fn a_cursor_block_alone_on_its_row_is_not_a_blank_row() {
        use ratatui::style::{Modifier, Style};
        // Blank rows around a row that holds only a blank in reverse video,
        // which is how an agent's interface draws its cursor.
        let mut f = frame(&["answer", "", "", " ", "", "", "status"], None);
        f.rows[3][0].style = Style::new().add_modifier(Modifier::REVERSED);
        // The runs of blank rows around it shrink; its own row stays.
        assert_eq!(FrameView::new(&f).rows_shown(5), [0, 1, 3, 4, 6]);
    }

    #[test]
    fn a_large_area_shows_everything() {
        let f = frame(&["a", "", "", "b"], None);
        assert_eq!(FrameView::new(&f).rows_shown(10), [0, 1, 2, 3]);
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

    #[test]
    fn the_cursor_follows_shrunk_rows() {
        let f = frame(&["a", "", "", "", "> ", "s"], Some((2, 4)));
        let view = FrameView::new(&f);
        assert_eq!(view.rows_shown(4), [0, 1, 4, 5]);
        assert_eq!(
            view.cursor_in(Rect::new(0, 0, 10, 4)),
            Some(Position::new(2, 2))
        );
    }
}
