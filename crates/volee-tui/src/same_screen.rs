//! The proof asked of the one terminal component (ADR 0028): the same
//! screen, compared cell by cell between the preview, the full-screen view
//! and a tile of the wall.
//!
//! One agent, one screen. Each view is drawn with the test backend, and
//! every cell a view shows is compared, with its colors and its attributes,
//! with the same cell of the two other views and with the emulator's own
//! reading of the bytes. The screen holds what could make the views differ:
//! wide characters, a wide character on a view's edge, palette and RGB
//! colors, a colored background to the end of a row, a gap of blank rows,
//! and a blank in reverse video at the end of a row, which is how an
//! agent's interface draws its cursor.
//!
//! Three times: for an agent in a volee host, whose screen volee holds and
//! whose views show it row for row; for a session in a multiplexer's pane,
//! whose views show what the multiplexer captured of it, row for row too;
//! and for a session volee reads the replay of, whose views all show that
//! replay, a run of blank rows counted as one.

use std::fmt::Write as _;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use volee_core::time::Timestamp;
use volee_host::{Size, Update};
use volee_term::{Frame, Screen, StyledCell};

use crate::app::{Effect, Model, Msg, inner, update};
use crate::preview::adapted;
use crate::source::Reading;
use crate::test_support::{hosted_model, key, model, press, start, welcome};
use crate::theme::{StyleRole, Theme};
use crate::view::view;

/// The agent's terminal: smaller than the full-screen view of the
/// interface below, which then draws it cell for cell, and larger than the
/// preview and a tile, which crop it.
const AGENT: Size = Size {
    cols: 160,
    rows: 40,
};

/// The interface's terminal.
const INTERFACE: (u16, u16) = (170, 50);

/// The attributes a person sees on a cell.
const SEEN: Modifier = Modifier::BOLD
    .union(Modifier::DIM)
    .union(Modifier::ITALIC)
    .union(Modifier::UNDERLINED)
    .union(Modifier::REVERSED)
    .union(Modifier::CROSSED_OUT);

/// What an agent's interface writes: a banner, tool calls in colors, a row
/// of a diff with its background to the edge, a gap, an answer, the prompt
/// with its cursor block, and status lines. The terminal's own cursor is
/// hidden, as such an interface hides it.
fn screen_bytes() -> Vec<u8> {
    let width = usize::from(AGENT.cols);
    let rule = "─".repeat(width);
    let wide = "界".repeat(75);
    let padding = " ".repeat(width - 26);
    [
        "\x1b[?25l\x1b[2J\x1b[H",
        // A banner with wide characters.
        "\x1b[38;5;208m▐▛███▜▌\x1b[0m  \x1b[1mClaude Code\x1b[0m 漢字 🚀 ready\r\n",
        "\x1b[2m~/code/sandbox · main\x1b[0m\r\n\r\n",
        "\x1b[38;2;120;200;255m⏺\x1b[0m \x1b[1mBash\x1b[0m(cargo test)\r\n",
        "  \x1b[2m⎿\x1b[0m  \x1b[3m356 passed\x1b[0m, \x1b[4m0 failed\x1b[0m, \x1b[9mskipped\x1b[0m\r\n",
        // A row of a diff: its background runs to the edge of the terminal.
        "\x1b[48;2;20;60;30m\x1b[38;5;114m+ let total = sum(values);",
        &padding,
        "\x1b[0m\r\n",
        // A gap, as a replay or a tall terminal leaves.
        "\r\n\r\n\r\n\r\n\r\n",
        "\x1b[38;5;114m⏺\x1b[0m FRUIT\r\n\r\n",
        // Two rows of wide characters, starting on even columns then on odd
        // ones: whatever a view's width, its right edge cuts one in two.
        &wide,
        "\r\nx",
        &wide,
        "\r\n\x1b[2m",
        &rule,
        "\x1b[0m\r\n",
        // The prompt, and the block the interface draws as its cursor: a
        // blank in reverse video, last on its row.
        "❯ \x1b[7m \x1b[27m\r\n",
        "\x1b[2m",
        &rule,
        "\x1b[0m\r\n",
        "  \x1b[2m[model] sandbox on main\x1b[0m\r\n",
        "  \x1b[38;5;240m▭▭▭▭▭▭▭▭\x1b[0m 6% ctx\r\n",
        "  \x1b[1m-- INSERT --\x1b[0m \x1b[38;5;208m⏵⏵ auto mode on\x1b[0m",
    ]
    .concat()
    .into_bytes()
}

/// The emulator's own reading of the bytes, in a terminal of `size`: the
/// reference.
fn reference(size: Size) -> Frame {
    let mut screen = Screen::new(size.cols, size.rows, 0).unwrap();
    screen.feed(&screen_bytes());
    screen.frame().unwrap()
}

/// The same screen as a multiplexer's capture of a pane comes (volee-mux):
/// each row placed with a cursor move, and the cursor left after the last
/// row with text, as far right as the widest row.
fn captured() -> String {
    let bytes = String::from_utf8(screen_bytes()).unwrap();
    // The capture starts on a blank screen: what clears it is left out.
    let drawn = bytes
        .strip_prefix("\x1b[?25l\x1b[2J\x1b[H")
        .expect("the screen starts by clearing itself");
    let rows: Vec<&str> = drawn.split("\r\n").collect();
    let mut output = String::new();
    for (number, row) in (1..).zip(&rows) {
        // Writing to a `String` cannot fail.
        let _ = write!(output, "\x1b[0m\x1b[{number};1H{row}");
    }
    let _ = write!(output, "\x1b[0m\x1b[{};{}H", rows.len(), AGENT.cols);
    output
}

fn draw(model: &Model) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(model.size.0, model.size.1)).unwrap();
    terminal.draw(|frame| view(model, frame)).unwrap();
    terminal.backend().buffer().clone()
}

/// The connection an update asked for, if any.
fn opened(effects: &[Effect]) -> Option<u64> {
    effects.iter().find_map(|effect| match effect {
        Effect::LiveOpen { link, .. } => Some(*link),
        _ => None,
    })
}

/// The host of `link` answers with the agent's screen.
fn answer(model: &mut Model, link: u64) {
    update(
        model,
        Msg::LiveOpened {
            link,
            welcome: welcome(AGENT, true),
        },
    );
    update(
        model,
        Msg::LiveUpdate {
            link,
            update: Update::Snapshot(screen_bytes()),
        },
    );
}

/// The rows of the reference a view that crops shows in `height` rows: the
/// last ones down to the last row that shows something, as they are, gaps
/// included.
fn cropped_rows(frame: &Frame, height: usize) -> Vec<usize> {
    let last = frame
        .rows
        .iter()
        .rposition(|row| !row.iter().all(StyledCell::shows_nothing))
        .unwrap_or(0);
    let first = (last + 1).saturating_sub(height);
    (first..=last).collect()
}

/// The rows of the reference a view of a replay shows in `height` rows: as
/// for a held screen, but a run of rows that show nothing counts as one (a
/// replay leaves blank what it never redrew).
fn replayed_rows(frame: &Frame, height: usize) -> Vec<usize> {
    let empty = |row: &Vec<StyledCell>| row.iter().all(StyledCell::shows_nothing);
    let last = frame.rows.iter().rposition(|row| !empty(row)).unwrap_or(0);
    let mut rows: Vec<usize> = Vec::new();
    for index in 0..=last {
        let repeated = empty(&frame.rows[index])
            && rows
                .last()
                .is_some_and(|before| empty(&frame.rows[*before]));
        if !repeated {
            rows.push(index);
        }
    }
    let first = rows.len().saturating_sub(height);
    rows[first..].to_vec()
}

/// One view: its name, its buffer, where the screen is drawn in it, and
/// which rows of the reference its rows show.
struct Seen {
    name: &'static str,
    buffer: Buffer,
    area: Rect,
    rows: Vec<usize>,
}

impl Seen {
    /// Whether this view's right edge cuts the wide character at a cell of
    /// the reference in two.
    fn cuts(&self, frame: &Frame, row: usize, column: usize) -> bool {
        let wide = frame.rows[row]
            .get(column + 1)
            .is_some_and(|next| next.continuation);
        wide && self.rows.contains(&row) && column + 1 == usize::from(self.area.width)
    }

    /// The cell this view draws for a cell of the reference, when it shows
    /// it: the row is one of its rows, the column is inside its width, and
    /// a wide character is not cut by its edge.
    fn cell(&self, frame: &Frame, row: usize, column: usize) -> Option<&Cell> {
        let line = self.rows.iter().position(|shown| *shown == row)?;
        let wide = frame.rows[row]
            .get(column + 1)
            .is_some_and(|next| next.continuation);
        let needed = column + if wide { 2 } else { 1 };
        if needed > usize::from(self.area.width) || line >= usize::from(self.area.height) {
            return None;
        }
        let x = self.area.x + u16::try_from(column).ok()?;
        let y = self.area.y + u16::try_from(line).ok()?;
        self.buffer.cell((x, y))
    }
}

/// What a person sees of a cell: its character, its background, its
/// attributes, and its color when the cell draws with it.
fn look(cell: &Cell) -> (String, Option<Color>, Color, Modifier) {
    let symbol = cell.symbol().to_owned();
    let attributes = cell.modifier & SEEN;
    let draws = !symbol.trim().is_empty()
        || attributes.intersects(Modifier::REVERSED | Modifier::UNDERLINED | Modifier::CROSSED_OUT);
    (symbol, draws.then_some(cell.fg), cell.bg, attributes)
}

/// Where the preview panel and the full-screen view of a screen read
/// through the source draw it: inside their frame, one column in.
fn inside(panel: Rect) -> Rect {
    let panel = inner(panel);
    Rect::new(panel.x + 1, panel.y, panel.width - 1, panel.height)
}

/// The three views of an agent in a volee host, drawn one after the other
/// by the keys a person presses.
fn views_of_a_held_screen() -> (Frame, Vec<Seen>) {
    let frame = reference(AGENT);
    let (mut model, agent) = hosted_model(INTERFACE.0, INTERFACE.1);
    let tick = |model: &mut Model, seconds: i64| {
        let at: Timestamp = start().plus_millis(seconds * 1_000);
        update(model, Msg::Tick(at))
    };

    // The preview: the interface follows the selected agent's host.
    let link = opened(&tick(&mut model, 1)).expect("the preview follows the host");
    answer(&mut model, link);
    let area = inside(model.layout.preview.expect("a preview panel"));
    let preview = Seen {
        name: "the preview",
        buffer: draw(&model),
        area,
        rows: cropped_rows(&frame, usize::from(area.height)),
    };

    // Full size: `z`, on the same connection.
    assert_eq!(opened(&press(&mut model, "z")), None);
    let area = crate::edit::screen_area(model.size);
    assert!(
        AGENT.cols <= area.width && AGENT.rows <= area.height,
        "the full-screen view must hold the agent's screen cell for cell"
    );
    let full = Seen {
        name: "the full-screen view",
        buffer: draw(&model),
        area,
        rows: (0..usize::from(AGENT.rows)).collect(),
    };

    // The wall: back to the list, then `5`; the tile has its own connection.
    key(&mut model, KeyCode::Esc);
    let link = opened(&press(&mut model, "5")).expect("the tile follows the host");
    answer(&mut model, link);
    let wall = model.wall.as_ref().expect("the wall");
    let index = wall
        .tiles
        .iter()
        .position(|tile| tile.agent == agent)
        .expect("the agent's tile");
    let area = inner(crate::tiles::areas(model.size, wall.tiles.len()).1[index]);
    let tile = Seen {
        name: "the tile of the wall",
        buffer: draw(&model),
        area,
        rows: cropped_rows(&frame, usize::from(area.height)),
    };
    (frame, vec![preview, full, tile])
}

/// The three views of a session volee does not host: each shows what the
/// source read, `reading`, and of the screen the rows `shown` tells for
/// its height.
fn views_of_a_reading(
    reading: &Reading,
    shown: fn(&Frame, usize) -> Vec<usize>,
) -> (Frame, Vec<Seen>) {
    // What the source read is emulated at the size its output tells, at
    // least 200 by 50: wider and taller than what it wrote, which changes
    // no cell of it.
    let frame = reference(Size {
        cols: 200,
        rows: 50,
    });
    let mut model = model(INTERFACE.0, INTERFACE.1);
    let agent = model.selected.expect("an agent is selected");
    let read = |model: &mut Model| {
        update(
            model,
            Msg::PreviewLoaded {
                agent,
                result: Ok(reading.clone()),
            },
        );
    };

    read(&mut model);
    let area = inside(model.layout.preview.expect("a preview panel"));
    let preview = Seen {
        name: "the preview",
        buffer: draw(&model),
        area,
        rows: shown(&frame, usize::from(area.height)),
    };

    // Full size: the same reading, its long rows wrapped. None of these
    // rows is longer than the view.
    press(&mut model, "z");
    read(&mut model);
    let window = Rect::new(0, 0, INTERFACE.0, INTERFACE.1 - 1);
    let area = inside(window);
    assert!(AGENT.cols <= area.width, "no row of the reading wraps here");
    let full = Seen {
        name: "the full-screen view",
        buffer: draw(&model),
        area,
        rows: shown(&frame, usize::from(area.height)),
    };

    key(&mut model, KeyCode::Esc);
    press(&mut model, "5");
    read(&mut model);
    let wall = model.wall.as_ref().expect("the wall");
    let index = wall
        .tiles
        .iter()
        .position(|tile| tile.agent == agent)
        .expect("the agent's tile");
    let area = inner(crate::tiles::areas(model.size, wall.tiles.len()).1[index]);
    let tile = Seen {
        name: "the tile of the wall",
        buffer: draw(&model),
        area,
        rows: shown(&frame, usize::from(area.height)),
    };
    (frame, vec![preview, full, tile])
}

/// How many rows that show nothing follow one another, at most, among the
/// rows a view shows.
fn longest_gap(frame: &Frame, view: &Seen) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for row in &view.rows {
        if frame.rows[*row].iter().all(StyledCell::shows_nothing) {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// What a comparison found.
struct Compared {
    /// Each difference, said for a person.
    differences: Vec<String>,
    /// How many cells each view showed.
    cells: [usize; 3],
    /// How many times two views showed the same cell of the screen.
    shared: usize,
    /// How many wide characters each view's edge cut in two.
    cut: [usize; 3],
}

/// A cell of the screen as the interface draws it, whatever the view: on
/// the interface's background, in its text color, with the cell's own
/// colors and attributes over them.
fn as_drawn(cell: &StyledCell, theme: &Theme) -> Cell {
    let mut drawn = Cell::default();
    drawn.set_style(theme.style(StyleRole::Background));
    drawn.set_symbol(if cell.symbol.is_empty() {
        " "
    } else {
        cell.symbol.as_str()
    });
    drawn.set_style(
        theme
            .style(StyleRole::Text)
            .patch(adapted(cell.style, theme.color_depth())),
    );
    drawn
}

/// Compares every cell of the screen that a view shows, in everything a
/// person sees of it: with the emulator's own cell, and with the same cell
/// of each other view.
fn compare(frame: &Frame, views: &[Seen]) -> Compared {
    let theme = Theme::default();
    let mut found = Compared {
        differences: Vec::new(),
        cells: [0; 3],
        shared: 0,
        cut: [0; 3],
    };
    for (row, cells) in frame.rows.iter().enumerate() {
        for (column, cell) in cells.iter().enumerate() {
            if cell.continuation {
                continue;
            }
            for (index, view) in views.iter().enumerate() {
                found.cut[index] += usize::from(view.cuts(frame, row, column));
            }
            let wanted = as_drawn(cell, &theme);
            let drawn: Vec<(usize, &Cell)> = views
                .iter()
                .enumerate()
                .filter_map(|(index, view)| Some((index, view.cell(frame, row, column)?)))
                .collect();
            // Against the emulator: a view that all the others follow into
            // the same mistake is still wrong.
            for (index, seen) in &drawn {
                found.cells[*index] += 1;
                if look(seen) != look(&wanted) {
                    found.differences.push(format!(
                        "row {row} column {column}: {} shows {:?}, the screen holds {:?}",
                        views[*index].name,
                        look(seen),
                        look(&wanted)
                    ));
                }
            }
            // Between views, two by two: everything a person sees of the cell.
            for (first, a) in drawn.iter().enumerate() {
                for b in &drawn[first + 1..] {
                    found.shared += 1;
                    if look(a.1) != look(b.1) {
                        found.differences.push(format!(
                            "row {row} column {column}: {} shows {:?}, {} shows {:?}",
                            views[a.0].name,
                            look(a.1),
                            views[b.0].name,
                            look(b.1)
                        ));
                    }
                }
            }
        }
    }
    found
}

/// Fails with the differences, then checks that the comparison compared
/// something: an empty comparison proves nothing.
#[track_caller]
fn assert_the_same(found: &Compared, views: &[Seen]) {
    assert!(
        found.differences.is_empty(),
        "{} cells differ, of {:?} compared:\n{}",
        found.differences.len(),
        found.cells,
        found
            .differences
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    // The views differ in size, so that the comparison is not of three
    // copies of one drawing: the preview and the tile crop.
    assert!(
        views[0].area.width < AGENT.cols && views[2].area.width < AGENT.cols,
        "the preview {:?} and the tile {:?} must be narrower than the screen",
        views[0].area,
        views[2].area
    );
    // Every view took part, on many cells, and views met on many of them.
    assert!(
        found.cells.iter().all(|count| *count > 300),
        "{:?}",
        found.cells
    );
    assert!(found.shared > 600, "{}", found.shared);
    // The edge of each view that crops fell in the middle of a wide
    // character, which the comparison then leaves out for that view.
    assert!(
        found.cut[0] > 0 && found.cut[2] > 0,
        "wide characters cut: {:?}",
        found.cut
    );
}

#[test]
fn a_held_screen_is_the_same_cell_by_cell_in_the_preview_full_size_and_on_the_wall() {
    let (frame, views) = views_of_a_held_screen();
    let found = compare(&frame, &views);
    assert_the_same(&found, &views);
    // The gap of blank rows is in every view that reaches it, as it is on
    // the screen: the tile shows a row that only a gap separates from the
    // row above the gap.
    let gap = frame
        .rows
        .iter()
        .position(|row| row.iter().all(StyledCell::shows_nothing))
        .expect("the screen has a blank row");
    assert!(views[2].rows.contains(&gap) || views[0].rows.contains(&gap));
}

#[test]
fn a_capture_is_the_same_cell_by_cell_in_the_preview_full_size_and_on_the_wall() {
    // What a multiplexer captured of a pane is the pane's screen: the
    // source says so, and the views show its rows as they are.
    let reading = Reading::screen(captured(), None);
    let (frame, views) = views_of_a_reading(&reading, cropped_rows);
    let found = compare(&frame, &views);
    assert_the_same(&found, &views);
    // The blank rows of the gap are in every view, as in the pane: all
    // five full size, and as many as fit in the views that show the
    // screen's last rows. A reading taken for a replay would show one.
    assert_eq!(longest_gap(&frame, &views[1]), 5, "{}", views[1].name);
    for view in &views {
        assert!(longest_gap(&frame, view) >= 2, "{}", view.name);
    }
}

#[test]
fn a_replay_is_the_same_cell_by_cell_in_the_preview_full_size_and_on_the_wall() {
    let text = String::from_utf8(screen_bytes()).unwrap();
    let (frame, views) = views_of_a_reading(&Reading::replay(text), replayed_rows);
    let found = compare(&frame, &views);
    assert_the_same(&found, &views);
    // A run of blank rows counts as one in every view.
    for view in &views {
        assert_eq!(longest_gap(&frame, view), 1, "{}", view.name);
    }
}
