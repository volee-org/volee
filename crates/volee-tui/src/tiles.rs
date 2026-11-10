//! The wall of screens, inside the interface (ADR 0028): the agents of the
//! list that run, each as a tile of its screen, as many to a page as the
//! terminal shows at a readable size.
//!
//! A tile shows its agent where it runs. An agent in a volee host is
//! followed live, through a connection that only watches. Any other is read
//! through the source, as the preview is, again every few seconds while it
//! works. No tile takes an agent's input or resizes its terminal: opening
//! the selected one does, as from the list.
//!
//! The wall's selection is the interface's: moving on the wall moves in the
//! list, and the keys the list has for its selected agent (open it, show it
//! full size) act on the selected tile. What is opened from the wall comes
//! back to the wall.

use std::path::PathBuf;

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use volee_core::AgentId;
use volee_core::time::Timestamp;

use crate::app::{Effect, Model, Overlay};
use crate::keymap::Action;
use crate::live::{Live, Phase};
use crate::preview::PreviewScreen;
use crate::source::SourceError;

/// How long a screen read through the source stays before it is read again,
/// while its agent works. Longer than the preview's two seconds: a page
/// holds several of them.
const REFRESH_MILLIS: u64 = 5_000;

/// The smallest tile worth reading, border included: a row or a column of
/// the wall holds up to three of them. A tile shows the bottom of a screen,
/// where an agent's interface keeps its prompt and its status lines: under
/// fourteen rows, little of what the agent says is left above them.
const TILE: (u16, u16) = (40, 14);

/// The size past which a row or a column holds a fourth tile: a terminal
/// that large shows sixteen screens at the size a small one shows four.
const ROOMY_TILE: (u16, u16) = (72, 20);

/// The wall, while it shows or while what was opened from it does.
#[derive(Debug, Default)]
pub(crate) struct Wall {
    /// The tiles of the page shown, in the list's order.
    pub(crate) tiles: Vec<Tile>,
    /// How many agents the wall holds, on all its pages.
    pub(crate) count: usize,
    /// Where the selected agent is among them.
    pub(crate) position: usize,
    /// How many tiles a page holds, in the terminal as it is.
    page_size: usize,
    /// Something covers the wall: its connections are closed until it shows
    /// again.
    covered: bool,
}

impl Wall {
    /// The page shown, from 0, and how many pages there are.
    pub(crate) fn pages(&self) -> (usize, usize) {
        let size = self.page_size.max(1);
        (self.position / size, self.count.div_ceil(size))
    }
}

/// One screen of the wall.
#[derive(Debug)]
pub(crate) struct Tile {
    /// Whose screen.
    pub(crate) agent: AgentId,
    /// How the wall knows it.
    pub(crate) screen: TileScreen,
}

/// How a tile knows its agent's screen.
#[derive(Debug)]
pub(crate) enum TileScreen {
    /// Followed live, in the agent's volee host.
    Live(Live),
    /// Read through the source, where the agent runs elsewhere.
    Read(Reading),
}

/// A screen read through the source.
#[derive(Debug, Default)]
pub(crate) struct Reading {
    /// The screen, once read.
    pub(crate) screen: Option<PreviewScreen>,
    /// Why it could not be read.
    pub(crate) error: Option<String>,
    /// A request is under way.
    loading: bool,
    /// When the screen was last asked for, or read.
    asked_at: Option<Timestamp>,
    /// The agent's state then: a change asks again, so that the end of a
    /// turn is read.
    state: &'static str,
    /// The source cannot show this agent's screen: it is not asked again.
    unsupported: bool,
}

/// Where the wall draws in a terminal of `size`: one row that says what it
/// holds, then `count` tiles down to the hint line.
pub(crate) fn areas(size: (u16, u16), count: usize) -> (Rect, Vec<Rect>) {
    let (width, height) = size;
    let header = Rect::new(0, 0, width, height.min(1));
    let body = Rect::new(0, 1, width, height.saturating_sub(2));
    let (columns, rows) = grid(size, count);
    let tiles = Layout::vertical(vec![Constraint::Fill(1); rows])
        .split(body)
        .iter()
        .flat_map(|row| {
            Layout::horizontal(vec![Constraint::Fill(1); columns])
                .split(*row)
                .to_vec()
        })
        .take(count)
        .collect();
    (header, tiles)
}

/// The most columns and rows of tiles a terminal of `size` shows: as many
/// as stay readable, three at most, and four in a terminal large enough for
/// roomy ones.
fn capacity(size: (u16, u16)) -> (usize, usize) {
    let fit = |cells: u16, tile: u16, roomy: u16| -> usize {
        if cells / 4 >= roomy {
            4
        } else {
            usize::from(cells / tile).clamp(1, 3)
        }
    };
    (
        fit(size.0, TILE.0, ROOMY_TILE.0),
        fit(size.1.saturating_sub(2), TILE.1, ROOMY_TILE.1),
    )
}

/// How many tiles a page holds in a terminal of `size`.
fn page_size(size: (u16, u16)) -> usize {
    let (columns, rows) = capacity(size);
    columns * rows
}

/// Columns and rows of the grid of `count` tiles, a page of them at most:
/// as square as the terminal allows, so that tiles keep its proportions.
fn grid(size: (u16, u16), count: usize) -> (usize, usize) {
    if count == 0 {
        return (0, 0);
    }
    let (most_columns, most_rows) = capacity(size);
    let square = (1..=count).find(|side| side * side >= count).unwrap_or(1);
    let columns = square
        .min(most_columns)
        .max(count.div_ceil(most_rows))
        .max(1);
    (columns, count.div_ceil(columns))
}

/// The agents of the list that run, in the list's order.
fn running(model: &Model) -> Vec<AgentId> {
    model
        .rail
        .iter()
        .filter(|entry| {
            model
                .registry
                .agent(entry.agent)
                .is_some_and(|agent| agent.state.is_live())
        })
        .map(|entry| entry.agent)
        .collect()
}

/// The socket of the volee host an agent runs in.
fn host_socket(model: &Model, agent: AgentId) -> Option<PathBuf> {
    let hosts = model.hosts.as_ref()?;
    let host = model.registry.agent(agent)?.location.host()?;
    Some(hosts.socket(host))
}

/// Whether the wall follows `agent`'s screen live.
pub(crate) fn follows(model: &Model, agent: AgentId) -> bool {
    model.wall.as_ref().is_some_and(|wall| {
        wall.tiles.iter().any(|tile| {
            tile.agent == agent
                && matches!(&tile.screen, TileScreen::Live(live) if live.phase == Phase::Live)
        })
    })
}

/// Opens the wall on the agents of the list that run. The update that
/// opens it ends with [`settle`], which fills its first page.
pub(crate) fn open(model: &mut Model, effects: &mut Vec<Effect>) {
    let agents = running(model);
    let Some(first) = agents.first().copied() else {
        model.notify(
            "No agent of the list runs: the wall shows the screens of the agents that do.",
            false,
        );
        return;
    };
    if !model
        .selected
        .is_some_and(|selected| agents.contains(&selected))
    {
        model.select(first, effects);
    }
    // The wall follows every screen of its page itself.
    crate::edit::close(model, effects);
    model.wall = Some(Wall::default());
    model.overlay = Some(Overlay::Wall);
}

/// Closes the wall and its connections.
pub(crate) fn close(model: &mut Model, effects: &mut Vec<Effect>) {
    if let Some(mut wall) = model.wall.take() {
        release(&mut wall, effects);
    }
    if model.overlay == Some(Overlay::Wall) {
        model.overlay = None;
    }
}

/// Drops the tiles and closes their connections.
fn release(wall: &mut Wall, effects: &mut Vec<Effect>) {
    for tile in wall.tiles.drain(..) {
        if let TileScreen::Live(live) = tile.screen {
            effects.push(Effect::LiveClose { link: live.link });
        }
    }
}

/// Something covers the wall: its connections close, its screens stay as
/// they were. Returns whether that changed anything.
fn cover(wall: &mut Wall, effects: &mut Vec<Effect>) -> bool {
    if wall.covered {
        return false;
    }
    wall.covered = true;
    for tile in &wall.tiles {
        if let TileScreen::Live(live) = &tile.screen {
            effects.push(Effect::LiveClose { link: live.link });
        }
    }
    true
}

/// A key of the wall.
pub(crate) fn perform(model: &mut Model, action: Action, effects: &mut Vec<Effect>) {
    if action == Action::Back {
        close(model, effects);
        return;
    }
    let agents = running(model);
    // The selected agent stopped running a moment ago: the end of this
    // update moves the selection, the next key acts on it.
    let Some(position) = model
        .selected
        .and_then(|selected| agents.iter().position(|agent| *agent == selected))
    else {
        return;
    };
    let last = agents.len() - 1;
    let page = page_size(model.size);
    let page_start = position / page * page;
    let on_page = (agents.len() - page_start).min(page);
    let columns = grid(model.size, on_page).0.max(1);
    let row = (position - page_start) / columns;
    let last_row = (on_page - 1) / columns;
    let target = match action {
        Action::Left => position.saturating_sub(1),
        Action::Right => (position + 1).min(last),
        // The row above, on the page before when this is a page's first.
        Action::Up if position >= columns => position - columns,
        // The row below, on the next page when this is a page's last; the
        // last tile when the row below is shorter.
        Action::Down if row < last_row || page_start + on_page <= last => {
            (position + columns).min(last)
        }
        Action::First => 0,
        Action::Last => last,
        Action::FullScreen => {
            crate::app::open_full_screen(model, effects);
            return;
        }
        Action::TakeOver => {
            crate::app::open_agent(model, agents[position], effects);
            return;
        }
        _ => return,
    };
    model.select(agents[target], effects);
}

/// The mouse on the wall: a click selects a tile, a second one soon after
/// opens its agent, as in the list.
pub(crate) fn on_mouse(model: &mut Model, mouse: MouseEvent, effects: &mut Vec<Effect>) -> bool {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return false;
    }
    let Some(wall) = &model.wall else {
        return false;
    };
    let (_, areas) = areas(model.size, wall.tiles.len());
    let at = Position::new(mouse.column, mouse.row);
    let Some(agent) = areas
        .iter()
        .zip(&wall.tiles)
        .find(|(area, _)| area.contains(at))
        .map(|(_, tile)| tile.agent)
    else {
        return false;
    };
    model.select(agent, effects);
    if model.double_click(agent) {
        crate::app::open_agent(model, agent, effects);
    }
    true
}

/// The live screen of the tile whose connection is `link`.
pub(crate) fn live_mut(model: &mut Model, link: u64) -> Option<&mut Live> {
    model
        .wall
        .as_mut()?
        .tiles
        .iter_mut()
        .find_map(|tile| match &mut tile.screen {
            TileScreen::Live(live) if live.link == link => Some(live),
            _ => None,
        })
}

/// The source read an agent's screen: its tile shows it. Returns whether a
/// tile took it.
pub(crate) fn loaded(
    model: &mut Model,
    agent: AgentId,
    result: &Result<String, SourceError>,
) -> bool {
    let depth = model.theme.color_depth();
    let now = model.now;
    let reading = model.wall.as_mut().and_then(|wall| {
        wall.tiles
            .iter_mut()
            .find_map(|tile| match &mut tile.screen {
                TileScreen::Read(reading) if tile.agent == agent => Some(reading),
                _ => None,
            })
    });
    let Some(reading) = reading else {
        return false;
    };
    reading.loading = false;
    // Counted from the answer: a slow source is not asked faster than it
    // answers.
    reading.asked_at = Some(now);
    match result {
        Ok(text) => {
            reading.screen = Some(PreviewScreen::parse(text, depth));
            reading.error = None;
            reading.unsupported = false;
        }
        Err(error) => {
            reading.unsupported = matches!(error, SourceError::Unsupported(_));
            reading.error = Some(error.to_string());
        }
    }
    true
}

/// After every update: the wall shows again once what was opened from it
/// closed, and its tiles follow the list and the selection. Returns whether
/// the wall changed.
pub(crate) fn settle(model: &mut Model, effects: &mut Vec<Effect>) -> bool {
    if model.wall.is_none() {
        return false;
    }
    match model.overlay {
        Some(Overlay::Wall) => {}
        // What was opened from the wall closed: the wall shows again, and
        // follows every screen itself.
        None => {
            crate::edit::close(model, effects);
            model.overlay = Some(Overlay::Wall);
        }
        // Something covers the wall: nothing of it shows, nothing is
        // followed.
        Some(_) => {
            return model.wall.as_mut().is_some_and(|wall| cover(wall, effects));
        }
    }
    sync(model, effects)
}

/// Brings the tiles in line with the list and the selection: the page that
/// holds the selected agent, a connection or a first read for each tile
/// that appears, the connection of each tile that leaves closed, and the
/// screens read through the source read again when they are due.
fn sync(model: &mut Model, effects: &mut Vec<Effect>) -> bool {
    let agents = running(model);
    let Some(mut wall) = model.wall.take() else {
        return false;
    };
    if agents.is_empty() {
        release(&mut wall, effects);
        if model.overlay == Some(Overlay::Wall) {
            model.overlay = None;
        }
        model.notify(
            "No agent of the list runs any more: the wall closed.",
            false,
        );
        return true;
    }
    // The selection stays on the wall: an agent that stopped running gives
    // it to the tile that takes its place.
    let position = model
        .selected
        .and_then(|selected| agents.iter().position(|agent| *agent == selected))
        .unwrap_or_else(|| wall.position.min(agents.len() - 1));
    if model.selected != Some(agents[position]) {
        model.select(agents[position], effects);
    }
    let mut changed = wall.position != position || wall.count != agents.len();
    wall.position = position;
    wall.count = agents.len();
    let page = page_size(model.size);
    changed |= wall.page_size != page;
    wall.page_size = page;
    let page_start = position / page * page;
    let shown = &agents[page_start..(page_start + page).min(agents.len())];

    let uncovered = std::mem::take(&mut wall.covered);
    let mut before = std::mem::take(&mut wall.tiles);
    for agent in shown {
        let kept = before
            .iter()
            .position(|tile| tile.agent == *agent)
            .map(|index| before.swap_remove(index));
        let (tile, new) = tile_for(model, *agent, kept, uncovered, effects);
        changed |= new;
        wall.tiles.push(tile);
    }
    // What left the page.
    for tile in before {
        if let TileScreen::Live(live) = tile.screen
            && !uncovered
        {
            effects.push(Effect::LiveClose { link: live.link });
        }
        changed = true;
    }
    changed |= read_due(model, &mut wall, effects);
    model.wall = Some(wall);
    changed
}

/// Watches the volee host at `socket`, on a new connection.
fn watch(model: &mut Model, socket: PathBuf, effects: &mut Vec<Effect>) -> u64 {
    model.next_link += 1;
    effects.push(Effect::LiveOpen {
        link: model.next_link,
        socket,
        edit: false,
        size: crate::edit::edit_size(model),
    });
    model.next_link
}

/// The tile of `agent`: the one the page had, when the agent is still where
/// it was, or a new one. `uncovered` tells that the wall shows again after
/// something covered it: its connections were closed. Returns the tile and
/// whether it is new.
fn tile_for(
    model: &mut Model,
    agent: AgentId,
    kept: Option<Tile>,
    uncovered: bool,
    effects: &mut Vec<Effect>,
) -> (Tile, bool) {
    let socket = host_socket(model, agent);
    let (screen, new) = match (kept.map(|tile| tile.screen), socket) {
        // In its host, as before.
        (Some(TileScreen::Live(mut live)), Some(socket)) => {
            if uncovered {
                live.reconnect(watch(model, socket, effects), false);
            }
            (TileScreen::Live(live), false)
        }
        // Elsewhere, as before.
        (Some(screen @ TileScreen::Read(_)), None) => (screen, false),
        // New on the page, or moved into or out of a volee host.
        (kept, socket) => {
            if let Some(TileScreen::Live(live)) = kept
                && !uncovered
            {
                effects.push(Effect::LiveClose { link: live.link });
            }
            let screen = match socket {
                Some(socket) => {
                    TileScreen::Live(Live::new(agent, watch(model, socket, effects), false))
                }
                None => TileScreen::Read(Reading::default()),
            };
            (screen, true)
        }
    };
    (Tile { agent, screen }, new)
}

/// Asks the source for the screens that are due: never read, read before
/// their agent changed state, or read a while ago for an agent that works.
fn read_due(model: &Model, wall: &mut Wall, effects: &mut Vec<Effect>) -> bool {
    if model.offline.is_some() {
        return false;
    }
    let mut asked = false;
    for tile in &mut wall.tiles {
        let TileScreen::Read(reading) = &mut tile.screen else {
            continue;
        };
        let Some(agent) = model.registry.agent(tile.agent) else {
            continue;
        };
        if reading.loading || reading.unsupported {
            continue;
        }
        let state = agent.state.label();
        let due = match reading.asked_at {
            None => true,
            Some(_) if reading.state != state => true,
            Some(at) => agent.state.is_busy() && at.millis_until(model.now) >= REFRESH_MILLIS,
        };
        if !due {
            continue;
        }
        reading.loading = true;
        reading.asked_at = Some(model.now);
        reading.state = state;
        // The interface reads the selected agent's screen for its preview:
        // that answer serves the tile too.
        if !(model.preview.agent == Some(tile.agent) && model.preview.loading) {
            effects.push(Effect::LoadPreview(tile.agent));
        }
        asked = true;
    }
    asked
}

#[cfg(test)]
mod tests;
