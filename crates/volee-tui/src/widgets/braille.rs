//! Lines drawn with braille dots: two columns and four rows of dots per
//! cell, so edges at any angle look smooth instead of stair-stepped.
//!
//! Lines are plotted into a raster first, each dot with a priority; when
//! several lines cross a cell, its dots merge and it takes the style of the
//! most important line. The raster is then written to the buffer in one go.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// The bit of the dot at (`x`, `y`) inside a cell, as in Unicode's braille
/// patterns (U+2800 plus the bits).
const DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// A point in dots: two per cell across, four per cell down.
pub(crate) type Dot = (i32, i32);

/// How a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pen {
    /// The style of the cells the line crosses.
    pub(crate) style: Style,
    /// Which line wins a shared cell: the higher, the more visible.
    pub(crate) priority: u8,
    /// How wide: one dot, two dots side by side, or two by two.
    pub(crate) width: u8,
    /// Every other pair of dots only.
    pub(crate) dashed: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Cell {
    bits: u8,
    style: Style,
    priority: u8,
}

/// Dots over an area of cells.
#[derive(Debug)]
pub(crate) struct Raster {
    area: Rect,
    cells: Vec<Cell>,
}

impl Raster {
    /// An empty raster over `area`.
    pub(crate) fn new(area: Rect) -> Self {
        Self {
            area,
            cells: vec![Cell::default(); usize::from(area.width) * usize::from(area.height)],
        }
    }

    /// The dot at the middle of a cell, in the raster's coordinates.
    pub(crate) fn center(column: u16, row: u16) -> Dot {
        (i32::from(column) * 2, i32::from(row) * 4 + 1)
    }

    /// The cell of a dot, if it is inside the raster.
    pub(crate) fn cell_of(&self, (x, y): Dot) -> Option<(u16, u16)> {
        let (column, row) = (x.div_euclid(2), y.div_euclid(4));
        let column = u16::try_from(column).ok()?;
        let row = u16::try_from(row).ok()?;
        (column < self.area.width && row < self.area.height).then_some((column, row))
    }

    /// Sets one dot.
    pub(crate) fn plot(&mut self, (x, y): Dot, pen: &Pen) {
        let Some((column, row)) = self.cell_of((x, y)) else {
            return;
        };
        let index = usize::from(row) * usize::from(self.area.width) + usize::from(column);
        let (dx, dy) = (x.rem_euclid(2), y.rem_euclid(4));
        let bit = DOTS[usize::from(dx == 1)][usize::try_from(dy).unwrap_or(0)];
        let cell = &mut self.cells[index];
        cell.bits |= bit;
        if pen.priority >= cell.priority {
            cell.priority = pen.priority;
            cell.style = pen.style;
        }
    }

    /// A straight line from `from` to `to`, both included (Bresenham).
    pub(crate) fn line(&mut self, from: Dot, to: Dot, pen: &Pen) {
        let (dx, dy) = ((to.0 - from.0).abs(), -(to.1 - from.1).abs());
        let (sx, sy) = ((to.0 - from.0).signum(), (to.1 - from.1).signum());
        let mostly_horizontal = dx >= -dy;
        let (mut x, mut y) = from;
        let mut error = dx + dy;
        let mut step = 0_u32;
        loop {
            if !pen.dashed || step % 4 < 2 {
                self.plot((x, y), pen);
                let beside = if mostly_horizontal {
                    (x, y + 1)
                } else {
                    (x + 1, y)
                };
                if pen.width >= 2 {
                    self.plot(beside, pen);
                }
                if pen.width >= 3 {
                    self.plot((x + 1, y + 1), pen);
                    self.plot(
                        if mostly_horizontal {
                            (x + 1, y)
                        } else {
                            (x, y + 1)
                        },
                        pen,
                    );
                }
            }
            if (x, y) == to {
                break;
            }
            let doubled = 2 * error;
            if doubled >= dy {
                error += dy;
                x += sx;
            }
            if doubled <= dx {
                error += dx;
                y += sy;
            }
            step += 1;
        }
    }

    /// The point a fraction `t` of the way from `from` to `to`.
    pub(crate) fn along(from: Dot, to: Dot, t: f64) -> Dot {
        let lerp = |a: i32, b: i32| {
            let value = f64::from(a) + (f64::from(b) - f64::from(a)) * t.clamp(0.0, 1.0);
            round(value)
        };
        (lerp(from.0, to.0), lerp(from.1, to.1))
    }

    /// Writes the dots to the buffer, over whatever is there.
    pub(crate) fn render(&self, buf: &mut Buffer) {
        for row in 0..self.area.height {
            for column in 0..self.area.width {
                let cell = self.cells
                    [usize::from(row) * usize::from(self.area.width) + usize::from(column)];
                if cell.bits == 0 {
                    continue;
                }
                let Some(symbol) = char::from_u32(0x2800 + u32::from(cell.bits)) else {
                    continue;
                };
                let mut text = [0; 4];
                if let Some(target) = buf.cell_mut((self.area.x + column, self.area.y + row)) {
                    target
                        .set_symbol(symbol.encode_utf8(&mut text))
                        .set_style(cell.style);
                }
            }
        }
    }
}

/// Rounds to the nearest integer; values here are dot coordinates, far
/// inside the range of `i32`.
#[allow(clippy::cast_possible_truncation)] // clamped to the range of i32 first
fn round(value: f64) -> i32 {
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pen() -> Pen {
        Pen {
            style: Style::default(),
            priority: 1,
            width: 1,
            dashed: false,
        }
    }

    fn symbols(buf: &Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn horizontal_and_vertical_lines_use_one_dot_row_or_column() {
        let area = Rect::new(0, 0, 4, 2);
        let mut raster = Raster::new(area);
        raster.line(Raster::center(0, 0), Raster::center(3, 0), &pen());
        raster.line(Raster::center(0, 1), Raster::center(0, 1), &pen());
        let mut buf = Buffer::empty(area);
        raster.render(&mut buf);
        assert_eq!(symbols(&buf), "⠒⠒⠒⠂\n⠂   ");
    }

    #[test]
    fn diagonals_are_smooth_and_points_outside_are_dropped() {
        let area = Rect::new(0, 0, 3, 2);
        let mut raster = Raster::new(area);
        raster.line((0, 0), (5, 7), &pen());
        raster.line((-4, -4), (-1, -1), &pen());
        let mut buf = Buffer::empty(area);
        raster.render(&mut buf);
        assert_eq!(symbols(&buf), "⠱⡀ \n ⠈⢆");
    }

    #[test]
    fn the_most_important_line_styles_a_shared_cell() {
        use ratatui::style::Color;
        let area = Rect::new(0, 0, 2, 1);
        let mut raster = Raster::new(area);
        let faint = Pen {
            style: Style::default().fg(Color::DarkGray),
            ..pen()
        };
        let hot = Pen {
            style: Style::default().fg(Color::Yellow),
            priority: 5,
            ..pen()
        };
        raster.line((0, 1), (3, 1), &hot);
        raster.line((0, 0), (3, 0), &faint);
        let mut buf = Buffer::empty(area);
        raster.render(&mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "⠛");
        assert_eq!(buf[(0, 0)].fg, Color::Yellow);
        assert_eq!(Raster::along((0, 0), (10, 4), 0.5), (5, 2));
    }
}
