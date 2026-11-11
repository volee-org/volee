//! The wall of the volee hosts of the machine (ADR 0021), as a program of
//! its own: `volee host wall` shows every running host as a tile of its
//! live screen, whatever it runs. The interface has its own wall, of the
//! agents of its list, wherever they run (`tiles.rs`, ADR 0028); the two
//! share their keys.

use std::io;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::Frame as UiFrame;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use volee_host::{AttachEnd, HostDir, Watch};

use crate::keymap::{Action as Keyed, Context, KeyCombo, Keymap};
use volee_term::{Frame, FrameView};

/// The most tiles shown at once.
const MAX_TILES: usize = 9;
/// How often the directory of hosts is read again.
const RESCAN: Duration = Duration::from_secs(1);

/// Runs the wall until the person quits. `client` names this client in the
/// hosts' logs; `only`, when not empty, limits the wall to these host ids
/// (the agents of the interface's current list); `keymap` gives the wall's
/// keys (the `wall` context) and the leave key of a take over.
pub fn run(dir: &HostDir, client: &str, only: &[String], keymap: &Keymap) -> io::Result<()> {
    // `init` also restores the terminal if the wall panics.
    let mut terminal = ratatui::init();
    let mut wall = Wall::new(
        dir.clone(),
        client.to_owned(),
        only.to_vec(),
        keymap.clone(),
    );
    let result = wall.run(&mut terminal);
    ratatui::try_restore()?;
    result
}

struct Tile {
    id: String,
    name: Option<String>,
    command: String,
    ended: bool,
    watch: Option<Watch>,
    frame: Option<Frame>,
}

impl Tile {
    fn label(&self) -> String {
        let short = self.id.get(..9).unwrap_or(&self.id);
        let state = match &self.watch {
            Some(watch) if watch.exit().is_some() => "ended".to_owned(),
            Some(watch) if watch.closed() => "gone".to_owned(),
            Some(watch) => watch.title().unwrap_or("running").to_owned(),
            None => "connecting".to_owned(),
        };
        match &self.name {
            Some(name) => format!(" {name} · {state} "),
            None => format!(" {short} · {} · {state} ", self.command),
        }
    }
}

struct Wall {
    dir: HostDir,
    client: String,
    only: Vec<String>,
    keymap: Keymap,
    tiles: Vec<Tile>,
    selected: usize,
    zoom: bool,
    scanned: Option<Instant>,
    status: String,
}

impl Wall {
    fn new(dir: HostDir, client: String, only: Vec<String>, keymap: Keymap) -> Self {
        let status = keymap
            .hints(&[Context::Wall])
            .iter()
            .map(|hint| format!("{} {}", hint.keys, hint.text))
            .collect::<Vec<_>>()
            .join(" · ");
        Self {
            dir,
            client,
            only,
            keymap,
            tiles: Vec::new(),
            selected: 0,
            zoom: false,
            scanned: None,
            status,
        }
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut dirty = true;
        loop {
            if self.scanned.is_none_or(|at| at.elapsed() >= RESCAN) {
                dirty |= self.rescan();
            }
            dirty |= self.pump();
            if dirty {
                terminal.draw(|frame| self.draw(frame))?;
                dirty = false;
            }
            if !event::poll(Duration::from_millis(30))? {
                continue;
            }
            dirty = true;
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match self.key(key) {
                Action::None => {}
                Action::Quit => return Ok(()),
                Action::TakeOver => {
                    ratatui::try_restore()?;
                    self.take_over();
                    *terminal = ratatui::init();
                    terminal.clear()?;
                }
            }
        }
    }

    /// Follows the hosts that come and go. Returns whether the tiles changed.
    fn rescan(&mut self) -> bool {
        self.scanned = Some(Instant::now());
        let Ok(records) = self.dir.list() else {
            return false;
        };
        let live: Vec<_> = records
            .into_iter()
            .filter(|(record, live)| {
                *live && (self.only.is_empty() || self.only.contains(&record.id))
            })
            .collect();
        let before: Vec<String> = self.tiles.iter().map(|tile| tile.id.clone()).collect();
        let selected = self.tiles.get(self.selected).map(|tile| tile.id.clone());
        self.tiles
            .retain(|tile| live.iter().any(|(record, _)| record.id == tile.id));
        for (record, _) in live {
            match self.tiles.iter_mut().find(|tile| tile.id == record.id) {
                Some(tile) => tile.ended = record.exit.is_some(),
                None => self.tiles.push(Tile {
                    name: record.name.clone(),
                    command: record.command.join(" "),
                    ended: record.exit.is_some(),
                    id: record.id,
                    watch: None,
                    frame: None,
                }),
            }
        }
        // Running hosts first; the order of each group stays.
        self.tiles.sort_by_key(|tile| tile.ended);
        self.selected = selected
            .and_then(|id| self.tiles.iter().position(|tile| tile.id == id))
            .unwrap_or(self.selected)
            .min(self.tiles.len().saturating_sub(1));
        let order_changed = self.tiles.iter().map(|tile| &tile.id).ne(before.iter());
        // Only tiles on screen keep a watcher.
        let shown = self.shown();
        for (index, tile) in self.tiles.iter_mut().enumerate() {
            if shown.contains(&index) {
                if tile.watch.is_none() {
                    tile.watch = Watch::start(&self.dir.socket(&tile.id), &self.client).ok();
                }
            } else {
                tile.watch = None;
                tile.frame = None;
            }
        }
        order_changed
    }

    fn pump(&mut self) -> bool {
        let mut changed = false;
        for tile in &mut self.tiles {
            if let Some(watch) = &mut tile.watch
                && watch.pump()
            {
                tile.frame = watch.frame();
                changed = true;
            }
        }
        changed
    }

    /// The indexes of the tiles on screen: the page that holds the selected
    /// one, or the selected one alone when zoomed.
    fn shown(&self) -> std::ops::Range<usize> {
        if self.zoom {
            return self.selected..(self.selected + 1).min(self.tiles.len());
        }
        let page = self.selected / MAX_TILES;
        let start = page * MAX_TILES;
        start..(start + MAX_TILES).min(self.tiles.len())
    }

    fn key(&mut self, key: KeyEvent) -> Action {
        let columns = grid(self.shown().len()).0.max(1);
        let last = self.tiles.len().saturating_sub(1);
        let action = self
            .keymap
            .action(&[Context::Wall], KeyCombo::from_event(&key));
        match action {
            Some(Keyed::Back) if self.zoom => self.zoom = false,
            Some(Keyed::Back | Keyed::Quit) => return Action::Quit,
            Some(Keyed::FullScreen) => self.zoom = !self.zoom,
            Some(Keyed::TakeOver) if !self.tiles.is_empty() => return Action::TakeOver,
            Some(Keyed::Left) => self.selected = self.selected.saturating_sub(1),
            Some(Keyed::Right) => self.selected = (self.selected + 1).min(last),
            Some(Keyed::Up) => self.selected = self.selected.saturating_sub(columns),
            Some(Keyed::Down) => self.selected = (self.selected + columns).min(last),
            Some(Keyed::First) => self.selected = 0,
            Some(Keyed::Last) => self.selected = last,
            _ => {}
        }
        // A new page or the zoom changes which tiles need a watcher.
        self.scanned = None;
        Action::None
    }

    fn take_over(&mut self) {
        let Some(tile) = self.tiles.get(self.selected) else {
            return;
        };
        let id = tile.id.clone();
        let leave = self.keymap.leave_letter();
        self.status = match volee_host::attach_leaving(&self.dir.socket(&id), &self.client, leave) {
            Ok(AttachEnd::Detached) => format!("back from {id}; it keeps running"),
            Ok(AttachEnd::Exited(_)) => format!("{id} ended"),
            Ok(AttachEnd::InputTaken) => format!("{id} was taken over from another terminal"),
            Ok(AttachEnd::HostGone) => format!("the host of {id} went away"),
            Err(error) => format!("cannot take over {id}: {error}"),
        };
    }

    fn draw(&self, frame: &mut UiFrame) {
        let [body, footer] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
        let count = self.tiles.len();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {count} hosts "),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::raw(self.status.as_str()),
            ])),
            footer,
        );
        if self.tiles.is_empty() {
            frame.render_widget(
                Paragraph::new("No host runs. Start one with: volee host spawn -- <command>"),
                body,
            );
            return;
        }
        let shown = self.shown();
        let areas = tile_areas(body, shown.len());
        for (index, area) in shown.zip(areas) {
            self.draw_tile(frame, index, area);
        }
    }

    fn draw_tile(&self, frame: &mut UiFrame, index: usize, area: Rect) {
        let tile = &self.tiles[index];
        let selected = index == self.selected;
        let border = if selected {
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        let block = Block::bordered()
            .border_type(if selected {
                BorderType::Thick
            } else {
                BorderType::Rounded
            })
            .border_style(border)
            .title(tile.label());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        match &tile.frame {
            Some(screen) => {
                let view = FrameView::new(screen);
                let cursor = view.cursor_in(inner);
                frame.render_widget(view, inner);
                if selected
                    && self.zoom
                    && let Some(cursor) = cursor
                {
                    frame.set_cursor_position(cursor);
                }
            }
            None => frame.render_widget(Paragraph::new("waiting for the screen..."), inner),
        }
    }
}

enum Action {
    None,
    Quit,
    TakeOver,
}

/// Columns and rows of a grid for `count` tiles.
fn grid(count: usize) -> (usize, usize) {
    match count {
        0 => (0, 0),
        1 => (1, 1),
        2 => (2, 1),
        3 | 4 => (2, 2),
        5 | 6 => (3, 2),
        _ => (3, 3),
    }
}

/// The areas of `count` tiles that share `area`, row by row.
fn tile_areas(area: Rect, count: usize) -> Vec<Rect> {
    let (columns, rows) = grid(count);
    let row_areas = Layout::vertical(vec![Constraint::Fill(1); rows]).split(area);
    row_areas
        .iter()
        .flat_map(|row| {
            Layout::horizontal(vec![Constraint::Fill(1); columns])
                .split(*row)
                .to_vec()
        })
        .take(count)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grids_hold_their_tiles() {
        for count in 0..=MAX_TILES {
            let (columns, rows) = grid(count);
            assert!(columns * rows >= count, "{count}");
        }
        assert_eq!(tile_areas(Rect::new(0, 0, 100, 40), 3).len(), 3);
        assert_eq!(
            tile_areas(Rect::new(0, 0, 100, 40), 4)[3],
            Rect::new(50, 20, 50, 20)
        );
    }
}
