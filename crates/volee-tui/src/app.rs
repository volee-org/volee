//! The state of the interface and how it changes (ADR 0009).
//!
//! The interface follows the Elm architecture: a [`Model`], messages
//! ([`Msg`]) and a pure [`update`] that changes the model and returns the
//! [`Effect`]s the run loop must carry out. Drawing (in [`crate::view`]) only
//! reads the model. Everything the mouse can hit is computed here, by the
//! layout pass that ends every update.

use ratatui::crossterm::event::{
    KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use volee_core::event::{Event, EventKind};
use volee_core::role::Role;
use volee_core::time::Timestamp;
use volee_core::workspace::Location;
use volee_core::{Agent, AgentId, Registry};

use crate::activity::ActivityLog;
use crate::history::History;
use crate::keymap::{Action, Context, KeyCombo, Keymap};
use crate::layout::{Mode, ScreenLayout};
use crate::letters::Letters;
use crate::preview::PreviewScreen;
use crate::pulses::Pulses;
use crate::theme::Theme;
use crate::views::hierarchy::{self, Direction, Scene, SceneKey, ZoomChoice};
use crate::views::network::{self, NetworkState};
use crate::views::repos::ReposState;
use crate::views::timeline::TimelineState;

/// The views, in tab order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    /// Who launched whom.
    #[default]
    Hierarchy,
    /// Who talks to whom.
    Network,
    /// Repositories and worktrees.
    Repos,
    /// What happened when.
    Timeline,
}

impl Tab {
    /// Every tab, in order.
    pub(crate) const ALL: [Self; 4] = [Self::Hierarchy, Self::Network, Self::Repos, Self::Timeline];

    /// The tab's name.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Hierarchy => "Hierarchy",
            Self::Network => "Network",
            Self::Repos => "Repos",
            Self::Timeline => "Timeline",
        }
    }
}

/// The panels that can have the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Pane {
    /// The list of agents.
    #[default]
    Rail,
    /// The current view.
    View,
    /// The detail and the preview.
    Detail,
}

/// A window over the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overlay {
    /// The keys, scrolled by some lines.
    Help {
        /// Lines scrolled.
        scroll: u16,
    },
}

/// The filter prompt and its text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Filter {
    /// What was typed.
    pub(crate) text: String,
    /// Whether the prompt is open.
    pub(crate) editing: bool,
}

impl Filter {
    /// The text in lower case, when it filters anything.
    fn needle(&self) -> Option<String> {
        let text = self.text.trim();
        (!text.is_empty()).then(|| text.to_lowercase())
    }
}

/// A short message at the bottom of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Notice {
    /// The message.
    pub(crate) text: String,
    /// Whether it reports a failure.
    pub(crate) error: bool,
    /// When it disappears.
    pub(crate) until: Timestamp,
}

/// How long a notice stays.
const NOTICE_MILLIS: i64 = 4_000;

/// How often the preview of a busy agent is refreshed.
const PREVIEW_REFRESH_MILLIS: u64 = 2_000;

/// The preview of the selected agent.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PreviewState {
    /// Whose screen.
    pub(crate) agent: Option<AgentId>,
    /// The screen, once loaded.
    pub(crate) screen: Option<PreviewScreen>,
    /// Why it could not be loaded.
    pub(crate) error: Option<String>,
    /// A request is under way.
    pub(crate) loading: bool,
    /// When the last request left.
    pub(crate) requested_at: Option<Timestamp>,
    /// Rows scrolled up from the bottom of the screen.
    pub(crate) scroll: u16,
}

/// One row of the agents list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RailEntry {
    /// The agent.
    pub(crate) agent: AgentId,
    /// Tree guides in front of the name, in tree order (`├─ `).
    pub(crate) guides: String,
}

/// How the agents list is sorted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RailOrder {
    /// Who needs you first (the domain's urgency order).
    #[default]
    Urgency,
    /// As a tree, when there is no room for the hierarchy view.
    Tree,
}

/// Something the mouse can hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hit {
    /// An agent, in a panel.
    Agent(AgentId, Pane),
    /// A tab of the top bar.
    Tab(Tab),
    /// A panel.
    Pane(Pane),
    /// The cells of an agent's lane in the timeline: a click also moves the
    /// cursor to the column.
    Lane(AgentId),
}

/// What changes the model.
#[derive(Debug)]
pub(crate) enum Msg {
    /// A key was pressed.
    Key(KeyEvent),
    /// The mouse did something.
    Mouse(MouseEvent),
    /// Text was pasted.
    Paste(String),
    /// The terminal changed size.
    Resize {
        /// Columns.
        width: u16,
        /// Rows.
        height: u16,
    },
    /// Time passed.
    Tick(Timestamp),
    /// New events from the source, in order.
    Events(Vec<Event>),
    /// The source stopped sending events.
    SourceClosed,
    /// A preview arrived.
    PreviewLoaded {
        /// Whose.
        agent: AgentId,
        /// The raw screen, or why it failed.
        result: Result<String, String>,
    },
    /// The source cannot take over this agent from here.
    TakeOverUnavailable(AgentId),
    /// The interface is back after a take over.
    Resumed {
        /// The agent that was taken over.
        agent: AgentId,
        /// How it ended.
        result: Result<(), String>,
    },
}

/// What the run loop must do after an update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Leave volee.
    Quit,
    /// Fetch the screen of an agent.
    LoadPreview(AgentId),
    /// Suspend the interface and take over an agent in this terminal.
    TakeOver(AgentId),
}

/// How much the interface moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Motion {
    /// Spinners turn and messages travel along the network's lines.
    #[default]
    Full,
    /// Nothing travels: message pulses become short highlights and
    /// spinners stop.
    Reduced,
}

/// The spinner of working agents.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// How long a spinner frame lasts.
const SPINNER_MILLIS: i64 = 80;

/// The state of the interface.
#[derive(Debug)]
pub(crate) struct Model {
    /// Colors.
    pub(crate) theme: Theme,
    /// Key bindings.
    pub(crate) keymap: Keymap,
    /// A word about the data, such as "demo".
    pub(crate) banner: Option<String>,

    /// The replica of the daemon's registry.
    pub(crate) registry: Registry,
    /// Jump letters.
    pub(crate) letters: Letters,
    /// Activity counts for sparklines.
    pub(crate) activity: ActivityLog,
    /// What happened when, for the timeline.
    pub(crate) history: History,
    /// Messages travelling along the network's lines.
    pub(crate) pulses: Pulses,
    /// How much the interface moves.
    pub(crate) motion: Motion,
    /// The current time.
    pub(crate) now: Timestamp,

    /// The tab on screen.
    pub(crate) tab: Tab,
    /// The panel with the keyboard.
    pub(crate) focus: Pane,
    /// The selected agent.
    pub(crate) selected: Option<AgentId>,
    /// The agent selected before, for `-`.
    pub(crate) previous: Option<AgentId>,
    /// Subtrees entered with Enter, innermost last.
    pub(crate) subtree: Vec<AgentId>,
    /// The detail level of the hierarchy.
    pub(crate) zoom: ZoomChoice,
    /// The filter.
    pub(crate) filter: Filter,
    /// `'` was pressed and a letter is expected.
    pub(crate) jump_pending: bool,
    /// The window on top, if any.
    pub(crate) overlay: Option<Overlay>,
    /// The message at the bottom, if any.
    pub(crate) notice: Option<Notice>,
    /// The preview of the selected agent.
    pub(crate) preview: PreviewState,

    /// The terminal size.
    pub(crate) size: (u16, u16),
    /// Where each panel is.
    pub(crate) layout: ScreenLayout,
    /// The rows of the agents list.
    pub(crate) rail: Vec<RailEntry>,
    /// How the list is sorted.
    pub(crate) rail_order: RailOrder,
    /// The first visible row of the list.
    pub(crate) rail_offset: usize,
    /// The hierarchy, placed.
    pub(crate) scene: Scene,
    /// What the scene was built from.
    scene_key: Option<SceneKey>,
    /// The top left of the hierarchy's visible window, in scene cells.
    pub(crate) scene_offset: (i32, i32),
    /// Bring the selection into view at the next layout pass.
    reveal_selection: bool,
    /// The network view.
    pub(crate) network: NetworkState,
    /// The repositories view.
    pub(crate) repos: ReposState,
    /// The timeline view.
    pub(crate) timeline: TimelineState,
    /// Everything the mouse can hit, topmost last.
    pub(crate) hits: Vec<(Rect, Hit)>,

    /// Something changed since the last frame.
    pub(crate) dirty: bool,
}

impl Model {
    /// A model for a terminal of the given size.
    pub(crate) fn new(
        theme: Theme,
        keymap: Keymap,
        banner: Option<String>,
        size: (u16, u16),
        now: Timestamp,
    ) -> Self {
        let mut model = Self {
            theme,
            keymap,
            banner,
            registry: Registry::new(),
            letters: Letters::default(),
            activity: ActivityLog::default(),
            history: History::default(),
            pulses: Pulses::default(),
            motion: Motion::Full,
            now,
            tab: Tab::default(),
            focus: Pane::default(),
            selected: None,
            previous: None,
            subtree: Vec::new(),
            zoom: ZoomChoice::default(),
            filter: Filter::default(),
            jump_pending: false,
            overlay: None,
            notice: None,
            preview: PreviewState::default(),
            size,
            layout: ScreenLayout::default(),
            rail: Vec::new(),
            rail_order: RailOrder::default(),
            rail_offset: 0,
            scene: Scene::default(),
            scene_key: None,
            scene_offset: (0, 0),
            reveal_selection: true,
            network: NetworkState::default(),
            repos: ReposState::default(),
            timeline: TimelineState::default(),
            hits: Vec::new(),
            dirty: true,
        };
        model.relayout();
        model
    }

    /// The selected agent's record.
    pub(crate) fn selected_agent(&self) -> Option<&Agent> {
        self.selected.and_then(|id| self.registry.agent(id))
    }

    /// The spinner frame of the current time.
    pub(crate) fn spinner(&self) -> char {
        let frame = self.now.as_millis().div_euclid(SPINNER_MILLIS);
        SPINNER[usize::try_from(frame.rem_euclid(10)).unwrap_or(0)]
    }

    /// Whether something on screen moves, so the run loop ticks fast.
    pub(crate) fn is_animating(&self) -> bool {
        if self.motion == Motion::Reduced {
            return false;
        }
        if self.tab == Tab::Network && self.layout.view.is_some() && self.pulses.is_moving(self.now)
        {
            return true;
        }
        self.rail.iter().any(|entry| {
            self.registry.agent(entry.agent).is_some_and(|agent| {
                matches!(
                    agent.state,
                    volee_core::AgentState::Working | volee_core::AgentState::Launching
                )
            })
        })
    }

    /// Whether an agent passes the filter.
    pub(crate) fn matches(&self, agent: &Agent) -> bool {
        passes(self.filter.needle().as_deref(), &self.registry, agent)
    }

    /// The key contexts that apply now, the most specific first.
    pub(crate) fn contexts(&self) -> Vec<Context> {
        if matches!(self.overlay, Some(Overlay::Help { .. })) {
            return vec![Context::Help];
        }
        if self.filter.editing {
            return vec![Context::Filter];
        }
        let pane = match self.focus {
            Pane::Rail => Context::Agents,
            Pane::View => match self.tab {
                Tab::Hierarchy => Context::Hierarchy,
                Tab::Network => Context::Network,
                Tab::Repos => Context::Repos,
                Tab::Timeline => Context::Timeline,
            },
            Pane::Detail => Context::Detail,
        };
        vec![pane, Context::Global]
    }

    /// Whether the timeline's cursor left the present.
    fn scrubbing(&self) -> bool {
        self.tab == Tab::Timeline && self.timeline.cursor.is_some()
    }

    /// Whether Escape has something to close or leave.
    pub(crate) fn can_go_back(&self) -> bool {
        self.jump_pending
            || self.overlay.is_some()
            || self.filter.editing
            || self.scrubbing()
            || !self.subtree.is_empty()
            || self.filter.needle().is_some()
            || self.focus != Pane::Rail
    }

    /// The panels that can take the keyboard, in Tab order.
    fn panes(&self) -> Vec<Pane> {
        let mut panes = vec![Pane::Rail];
        if self.layout.view.is_some() {
            panes.push(Pane::View);
        }
        panes.push(Pane::Detail);
        panes
    }

    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some(Notice {
            text: text.into(),
            error,
            until: self.now.plus_millis(NOTICE_MILLIS),
        });
    }

    fn name_of(&self, agent: AgentId) -> String {
        self.registry
            .agent(agent)
            .map_or_else(|| agent.short(), |agent| agent.name.clone())
    }

    /// Selects an agent, remembering the previous one.
    fn select(&mut self, agent: AgentId, effects: &mut Vec<Effect>) {
        if self.selected == Some(agent) {
            return;
        }
        if let Some(record) = self.registry.agent(agent)
            && !self.matches(record)
        {
            self.filter = Filter::default();
        }
        if !self.subtree.is_empty() && !self.in_subtree(agent) {
            self.subtree.clear();
        }
        self.previous = self.selected;
        self.selected = Some(agent);
        self.reveal_selection = true;
        self.request_preview(agent, effects);
    }

    fn request_preview(&mut self, agent: AgentId, effects: &mut Vec<Effect>) {
        if self.preview.agent != Some(agent) {
            self.preview = PreviewState {
                agent: Some(agent),
                ..PreviewState::default()
            };
        }
        self.preview.loading = true;
        self.preview.requested_at = Some(self.now);
        effects.push(Effect::LoadPreview(agent));
    }

    /// Whether `agent` is inside the innermost subtree entered with Enter.
    fn in_subtree(&self, agent: AgentId) -> bool {
        let Some(root) = self.subtree.last() else {
            return true;
        };
        let mut current = Some(agent);
        while let Some(id) = current {
            if id == *root {
                return true;
            }
            current = self.registry.agent(id).and_then(|agent| agent.parent);
        }
        false
    }

    /// Applies events from the source.
    fn apply(&mut self, events: Vec<Event>) {
        for event in events {
            if let Err(error) = self.registry.apply(&event) {
                tracing::warn!(%error, "event ignored");
                continue;
            }
            self.history.record(&event);
            if let EventKind::MessagePosted { message } = &event.kind {
                self.pulses.record(message, self.now);
            }
            match &event.kind {
                EventKind::AgentRegistered { agent, name, .. } => {
                    self.letters.assign(*agent, name);
                }
                EventKind::ActivityRecorded { agent, .. } => {
                    self.activity.record(*agent, event.at);
                }
                EventKind::AgentForgotten { agent } => {
                    self.letters.release(*agent);
                    self.activity.forget(*agent);
                    self.subtree.retain(|root| root != agent);
                    if self.previous == Some(*agent) {
                        self.previous = None;
                    }
                }
                _ => {}
            }
        }
    }

    /// The layout pass: panels, list rows, the hierarchy and the hit areas.
    /// It runs at the end of every update; drawing reads its results.
    fn relayout(&mut self) {
        let area = Rect::new(0, 0, self.size.0, self.size.1);
        self.layout = ScreenLayout::compute(area, self.tab == Tab::Hierarchy);
        self.rail_order = if self.layout.mode == Mode::Narrow && self.tab == Tab::Hierarchy {
            RailOrder::Tree
        } else {
            RailOrder::Urgency
        };
        if self.layout.view.is_none() && self.focus == Pane::View {
            self.focus = Pane::Rail;
        }
        self.subtree
            .retain(|root| self.registry.agent(*root).is_some());
        self.rail = self.rail_entries();
        let selected_visible = self
            .selected
            .is_some_and(|id| self.rail.iter().any(|entry| entry.agent == id));
        if !selected_visible {
            let first = self.rail.first().map(|entry| entry.agent);
            if first.is_some() && first != self.selected {
                self.selected = first;
                self.reveal_selection = true;
            } else if first.is_none() {
                self.selected = None;
            }
        }
        if let Some(view) = self.layout.view
            && self.tab == Tab::Hierarchy
        {
            let inner = inner(view);
            let key = hierarchy::scene_key(
                &self.registry,
                self.subtree.last().copied(),
                self.zoom,
                (inner.width, inner.height),
            );
            if self.scene_key.as_ref() != Some(&key) {
                self.scene = Scene::build(
                    &self.registry,
                    self.subtree.last().copied(),
                    self.zoom,
                    (inner.width, inner.height),
                );
                self.scene_key = Some(key);
                self.reveal_selection = true;
            }
            self.scene_offset = self.fit_offset(inner);
        }
        if let Some(view) = self.layout.view {
            self.relayout_view(inner(view));
        }
        self.reveal_selection = false;
        self.rail_offset = self.fit_rail_offset();
        self.hits = self.hit_areas();
    }

    /// The layout pass of the network, repositories and timeline views.
    fn relayout_view(&mut self, area: Rect) {
        let needle = self.filter.needle();
        let registry = &self.registry;
        let matches = |agent: &Agent| passes(needle.as_deref(), registry, agent);
        match self.tab {
            Tab::Hierarchy => {}
            Tab::Network => {
                let (graph, _) = network::split(area);
                self.network.relayout(
                    registry,
                    self.history.messages(),
                    (graph.width, graph.height),
                );
            }
            Tab::Repos => self
                .repos
                .relayout(registry, &matches, area.height, self.selected),
            Tab::Timeline => self
                .timeline
                .relayout(registry, &matches, area, self.selected),
        }
    }

    /// Rows of the agents list, filtered.
    fn rail_entries(&self) -> Vec<RailEntry> {
        match self.rail_order {
            RailOrder::Urgency => self
                .registry
                .agents_by_urgency()
                .into_iter()
                .filter(|agent| self.matches(agent))
                .map(|agent| RailEntry {
                    agent: agent.id,
                    guides: String::new(),
                })
                .collect(),
            RailOrder::Tree => hierarchy::outline(&self.registry, self.subtree.last().copied())
                .into_iter()
                .filter(|(id, _)| self.registry.agent(*id).is_some_and(|a| self.matches(a)))
                .map(|(agent, guides)| RailEntry { agent, guides })
                .collect(),
        }
    }

    /// Rows per entry of the list.
    pub(crate) fn rail_row_height(&self) -> u16 {
        let rows = inner(self.layout.rail).height;
        let entries = u16::try_from(self.rail.len()).unwrap_or(u16::MAX);
        if self.rail_order == RailOrder::Urgency && entries.saturating_mul(2) <= rows {
            2
        } else {
            1
        }
    }

    fn fit_rail_offset(&self) -> usize {
        let visible = usize::from(inner(self.layout.rail).height / self.rail_row_height().max(1));
        let Some(index) = self
            .selected
            .and_then(|id| self.rail.iter().position(|entry| entry.agent == id))
        else {
            return 0;
        };
        let mut offset = self
            .rail_offset
            .min(self.rail.len().saturating_sub(visible.max(1)));
        if index < offset {
            offset = index;
        } else if visible > 0 && index >= offset + visible {
            offset = index + 1 - visible;
        }
        offset
    }

    /// The hierarchy's window: centered when the scene fits, otherwise
    /// moved just enough to show the selection.
    fn fit_offset(&self, area: Rect) -> (i32, i32) {
        let axis = |scene: u16, view: u16, offset: i32, target: Option<(u16, u16)>| -> i32 {
            let (scene, view) = (i32::from(scene), i32::from(view));
            if scene <= view {
                return -((view - scene) / 2);
            }
            let mut offset = offset.clamp(0, scene - view);
            if let Some((start, length)) = target {
                let (start, end) = (i32::from(start), i32::from(start) + i32::from(length));
                if start < offset {
                    offset = (start - 1).max(0);
                } else if end > offset + view {
                    offset = (end - view + 1).min(scene - view);
                }
            }
            offset
        };
        let target = if self.reveal_selection {
            self.selected.and_then(|id| self.scene.rect_of(id))
        } else {
            None
        };
        (
            axis(
                self.scene.width,
                area.width,
                self.scene_offset.0,
                target.map(|r| (r.x, r.width)),
            ),
            axis(
                self.scene.height,
                area.height,
                self.scene_offset.1,
                target.map(|r| (r.y, r.height)),
            ),
        )
    }

    fn hit_areas(&self) -> Vec<(Rect, Hit)> {
        let mut hits = Vec::new();
        let layout = &self.layout;
        hits.push((layout.rail, Hit::Pane(Pane::Rail)));
        hits.push((layout.detail, Hit::Pane(Pane::Detail)));
        if let Some(preview) = layout.preview {
            hits.push((preview, Hit::Pane(Pane::Detail)));
        }
        if let Some(view) = layout.view {
            hits.push((view, Hit::Pane(Pane::View)));
        }
        for (tab, area) in crate::widgets::top_bar::tab_areas(self) {
            hits.push((area, Hit::Tab(tab)));
        }
        let rail = inner(layout.rail);
        let height = self.rail_row_height();
        for (row, entry) in self.rail.iter().skip(self.rail_offset).enumerate() {
            let y = rail.y
                + u16::try_from(row)
                    .unwrap_or(u16::MAX)
                    .saturating_mul(height);
            if y >= rail.bottom() {
                break;
            }
            let rows = height.min(rail.bottom() - y);
            hits.push((
                Rect::new(rail.x, y, rail.width, rows),
                Hit::Agent(entry.agent, Pane::Rail),
            ));
        }
        if let Some(view) = layout.view {
            let area = inner(view);
            match self.tab {
                Tab::Hierarchy => {
                    for (agent, rect) in self.scene.targets() {
                        let x = i32::from(area.x) + i32::from(rect.x) - self.scene_offset.0;
                        let y = i32::from(area.y) + i32::from(rect.y) - self.scene_offset.1;
                        if let Some(visible) = clip(x, y, rect.width, rect.height, area) {
                            hits.push((visible, Hit::Agent(agent, Pane::View)));
                        }
                    }
                }
                Tab::Network => {
                    let (graph, _) = network::split(area);
                    for (agent, rect) in self.network.targets() {
                        let (x, y) = (i32::from(graph.x + rect.x), i32::from(graph.y + rect.y));
                        if let Some(visible) = clip(x, y, rect.width, rect.height, graph) {
                            hits.push((visible, Hit::Agent(agent, Pane::View)));
                        }
                    }
                }
                Tab::Repos => {
                    let lines = self.repos.lines.iter().skip(self.repos.offset);
                    for (row, line) in (0..area.height).zip(lines) {
                        if let crate::views::repos::Line::Agent { agent, .. } = line {
                            let rect = Rect::new(area.x, area.y + row, area.width, 1);
                            hits.push((rect, Hit::Agent(*agent, Pane::View)));
                        }
                    }
                }
                Tab::Timeline => {
                    let geometry = self.timeline.geometry;
                    let lanes = self.timeline.lanes.iter().skip(self.timeline.offset);
                    for (row, agent) in (0..geometry.lanes.height).zip(lanes) {
                        let y = geometry.lanes.y + row;
                        let label = Rect::new(geometry.lanes.x, y, geometry.label, 1);
                        hits.push((label, Hit::Agent(*agent, Pane::View)));
                        let cells = Rect::new(geometry.cells_x(), y, geometry.columns(), 1);
                        hits.push((cells, Hit::Lane(*agent)));
                    }
                }
            }
        }
        hits
    }

    /// The topmost thing under a cell.
    fn hit(&self, column: u16, row: u16) -> Option<Hit> {
        let position = Position::new(column, row);
        self.hits
            .iter()
            .rev()
            .find(|(area, _)| area.contains(position))
            .map(|(_, hit)| *hit)
    }

    /// The pane under a cell.
    fn pane_at(&self, column: u16, row: u16) -> Option<Pane> {
        let position = Position::new(column, row);
        self.hits.iter().rev().find_map(|(area, hit)| match hit {
            Hit::Pane(pane) if area.contains(position) => Some(*pane),
            _ => None,
        })
    }
}

/// Whether an agent's name, repository, branch or mission contains
/// `needle`, written in lower case; everything passes without a needle.
fn passes(needle: Option<&str>, registry: &Registry, agent: &Agent) -> bool {
    let Some(needle) = needle else {
        return true;
    };
    let mission = agent
        .mission
        .and_then(|mission| registry.mission(mission))
        .map(|mission| mission.title.as_str());
    [
        Some(agent.name.as_str()),
        agent.workspace.repo.as_deref(),
        agent.workspace.branch.as_deref(),
        mission,
    ]
    .into_iter()
    .flatten()
    .any(|field| field.to_lowercase().contains(needle))
}

/// A panel's area inside its border.
pub(crate) fn inner(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

/// The part of a rectangle at (`x`, `y`) that lies inside `area`.
fn clip(x: i32, y: i32, width: u16, height: u16, area: Rect) -> Option<Rect> {
    let left = x.max(i32::from(area.x));
    let top = y.max(i32::from(area.y));
    let right = (x + i32::from(width)).min(i32::from(area.right()));
    let bottom = (y + i32::from(height)).min(i32::from(area.bottom()));
    if left >= right || top >= bottom {
        return None;
    }
    Some(Rect::new(
        u16::try_from(left).ok()?,
        u16::try_from(top).ok()?,
        u16::try_from(right - left).ok()?,
        u16::try_from(bottom - top).ok()?,
    ))
}

/// Changes the model and returns what the run loop must do.
pub(crate) fn update(model: &mut Model, msg: Msg) -> Vec<Effect> {
    let mut effects = Vec::new();
    let mut dirty = true;
    match msg {
        Msg::Key(key) => {
            if key.kind == KeyEventKind::Release {
                dirty = false;
            } else {
                on_key(model, KeyCombo::from_event(&key), &mut effects);
            }
        }
        Msg::Mouse(mouse) => dirty = on_mouse(model, mouse, &mut effects),
        Msg::Paste(text) => {
            if model.filter.editing {
                model
                    .filter
                    .text
                    .extend(text.chars().filter(|c| !c.is_control()));
            } else {
                dirty = false;
            }
        }
        Msg::Resize { width, height } => model.size = (width, height),
        Msg::Tick(now) => dirty = on_tick(model, now, &mut effects),
        Msg::Events(events) => model.apply(events),
        Msg::SourceClosed => {
            model.notify(
                "The source stopped: what you see is no longer updated.",
                true,
            );
        }
        Msg::PreviewLoaded { agent, result } => {
            if model.preview.agent == Some(agent) {
                model.preview.loading = false;
                match result {
                    Ok(text) => {
                        model.preview.screen =
                            Some(PreviewScreen::parse(&text, model.theme.color_depth()));
                        model.preview.error = None;
                    }
                    Err(error) => model.preview.error = Some(error),
                }
            } else {
                dirty = false;
            }
        }
        Msg::TakeOverUnavailable(agent) => {
            let name = model.name_of(agent);
            let record = model.registry.agent(agent);
            let reason = match record.map(|agent| (agent.role, &agent.location)) {
                Some((Role::Subagent, _)) => "it runs inside its parent's session",
                Some((_, Location::Terminal)) => "it runs in another terminal; switch to it there",
                Some((_, Location::Pane { .. })) => {
                    "it runs in a multiplexer pane; switch to it there"
                }
                _ => "its harness cannot attach it from here",
            };
            model.notify(format!("Cannot take over {name}: {reason}."), true);
        }
        Msg::Resumed { agent, result } => {
            let name = model.name_of(agent);
            match result {
                Ok(()) => model.notify(format!("Back from {name}."), false),
                Err(error) => model.notify(format!("Could not take over {name}: {error}"), true),
            }
            if let Some(selected) = model.selected {
                model.request_preview(selected, &mut effects);
            }
        }
    }
    model.relayout();
    // The layout pass may have moved the selection (the agent left, or the
    // filter hides it): the preview follows.
    if let Some(selected) = model.selected
        && model.preview.agent != Some(selected)
    {
        model.request_preview(selected, &mut effects);
    }
    model.dirty |= dirty;
    effects
}

fn on_tick(model: &mut Model, now: Timestamp, effects: &mut Vec<Effect>) -> bool {
    let frame_before = model.spinner();
    let second_before = model.now.as_millis().div_euclid(1_000);
    let pulses_before = model.tab == Tab::Network && model.pulses.is_moving(model.now);
    model.now = now;
    model.pulses.prune(now);
    let mut changed = model.spinner() != frame_before
        || now.as_millis().div_euclid(1_000) != second_before
        || pulses_before;
    if model
        .notice
        .as_ref()
        .is_some_and(|notice| notice.until <= now)
    {
        model.notice = None;
        changed = true;
    }
    let busy = model
        .selected_agent()
        .is_some_and(|agent| agent.state.is_busy());
    let due = model
        .preview
        .requested_at
        .is_none_or(|at| at.millis_until(now) >= PREVIEW_REFRESH_MILLIS);
    if busy
        && !model.preview.loading
        && due
        && let Some(selected) = model.selected
    {
        model.request_preview(selected, effects);
    }
    changed
}

fn on_key(model: &mut Model, key: KeyCombo, effects: &mut Vec<Effect>) {
    if model.jump_pending {
        model.jump_pending = false;
        if let Some(letter) = key.typed_char().filter(char::is_ascii_alphabetic) {
            match model.letters.agent(letter) {
                Some(agent) => model.select(agent, effects),
                None => model.notify(format!("No agent has the letter {letter}."), false),
            }
        }
        return;
    }
    if model.filter.editing {
        match model.keymap.action(&[Context::Filter], key) {
            Some(Action::Apply) => model.filter.editing = false,
            Some(Action::Back) => model.filter = Filter::default(),
            Some(Action::Quit) => effects.push(Effect::Quit),
            _ => edit_filter(model, key),
        }
        return;
    }
    let contexts = model.contexts();
    if let Some((context, action)) = model.keymap.lookup(&contexts, key) {
        perform(model, context, action, effects);
    }
}

fn edit_filter(model: &mut Model, key: KeyCombo) {
    use ratatui::crossterm::event::KeyCode;
    match key.code() {
        KeyCode::Backspace | KeyCode::Char('u' | 'w') if key.ctrl_held() => {
            model.filter.text.clear();
        }
        KeyCode::Backspace => {
            model.filter.text.pop();
        }
        _ => {
            if let Some(c) = key.typed_char() {
                model.filter.text.push(c);
            }
        }
    }
}

fn perform(model: &mut Model, context: Context, action: Action, effects: &mut Vec<Effect>) {
    match action {
        Action::Quit => effects.push(Effect::Quit),
        Action::Help => {
            model.overlay = match model.overlay {
                Some(Overlay::Help { .. }) => None,
                None => Some(Overlay::Help { scroll: 0 }),
            };
        }
        Action::Back => back(model),
        Action::NextPane | Action::PreviousPane => {
            let panes = model.panes();
            let index = panes
                .iter()
                .position(|pane| *pane == model.focus)
                .unwrap_or(0);
            let next = if action == Action::NextPane {
                (index + 1) % panes.len()
            } else {
                (index + panes.len() - 1) % panes.len()
            };
            model.focus = panes[next];
        }
        Action::ShowHierarchy => model.tab = Tab::Hierarchy,
        Action::ShowNetwork => model.tab = Tab::Network,
        Action::ShowRepos => model.tab = Tab::Repos,
        Action::ShowTimeline => model.tab = Tab::Timeline,
        Action::Jump => model.jump_pending = true,
        Action::JumpBack => {
            if let Some(previous) = model.previous
                && model.registry.agent(previous).is_some()
            {
                model.select(previous, effects);
            }
        }
        Action::Filter => model.filter.editing = true,
        Action::TakeOver => {
            if let Some(agent) = model.selected {
                effects.push(Effect::TakeOver(agent));
            }
        }
        Action::Down | Action::Up | Action::Left | Action::Right | Action::First | Action::Last => {
            navigate(model, context, action, effects);
        }
        Action::FocusSubtree => {
            if let Some(agent) = model.selected
                && model.subtree.last() != Some(&agent)
            {
                model.subtree.push(agent);
                if model.layout.view.is_some() {
                    model.tab = Tab::Hierarchy;
                    model.focus = Pane::View;
                }
            }
        }
        Action::ZoomIn | Action::ZoomOut | Action::ZoomAuto => zoom(model, context, action),
        Action::PreviousEvent | Action::NextEvent => {
            let forward = action == Action::NextEvent;
            let found = model
                .selected
                .and_then(|agent| model.history.lane(agent))
                .and_then(|lane| model.timeline.event_from(&lane.marks, model.now, forward));
            match found {
                Some(at) => model.timeline.jump_to(at, model.now),
                None if forward => model.timeline.cursor = None,
                None => model.notify("No earlier event on this lane.", false),
            }
        }
        Action::Refresh => {
            if let Some(agent) = model.selected {
                model.request_preview(agent, effects);
            }
        }
        // Apply only means something in the filter prompt.
        _ => {}
    }
}

/// The zoom keys of a view: more or less detail, or the level the size
/// allows; on the timeline, a finer or coarser time scale, or back to now.
fn zoom(model: &mut Model, context: Context, action: Action) {
    let step = |choice: &mut ZoomChoice, current: hierarchy::Zoom| {
        *choice = match action {
            Action::ZoomIn => ZoomChoice::Fixed(current.more_detail()),
            Action::ZoomOut => ZoomChoice::Fixed(current.less_detail()),
            _ => ZoomChoice::Auto,
        };
    };
    match context {
        Context::Network => step(&mut model.network.zoom, model.network.scene.zoom),
        Context::Repos => step(&mut model.repos.zoom, model.repos.density),
        Context::Timeline => match action {
            Action::ZoomIn => model.timeline.zoom(true),
            Action::ZoomOut => model.timeline.zoom(false),
            _ => model.timeline.reset(),
        },
        _ => step(&mut model.zoom, model.scene.zoom),
    }
}

/// Escape: closes or leaves the innermost thing, and never quits.
fn back(model: &mut Model) {
    if model.overlay.take().is_some() {
        return;
    }
    if model.scrubbing() {
        model.timeline.cursor = None;
        return;
    }
    if !model.subtree.is_empty() {
        model.subtree.pop();
        model.reveal_selection = true;
        return;
    }
    if model.filter.needle().is_some() {
        model.filter = Filter::default();
        return;
    }
    model.focus = Pane::Rail;
}

fn navigate(model: &mut Model, context: Context, action: Action, effects: &mut Vec<Effect>) {
    match context {
        Context::Help => {
            if let Some(Overlay::Help { scroll }) = &mut model.overlay {
                *scroll = match action {
                    Action::Down => scroll.saturating_add(1),
                    _ => scroll.saturating_sub(1),
                };
            }
        }
        Context::Detail => {
            let scroll = &mut model.preview.scroll;
            *scroll = match action {
                Action::Up => scroll.saturating_add(1),
                _ => scroll.saturating_sub(1),
            };
        }
        Context::Hierarchy => {
            let target = match action {
                Action::First => model.scene.first(),
                Action::Last => model.scene.last(),
                _ => model.selected.and_then(|from| {
                    let selectable: Vec<AgentId> =
                        model.rail.iter().map(|entry| entry.agent).collect();
                    model
                        .scene
                        .neighbour(from, direction(action), |id| selectable.contains(&id))
                }),
            };
            if let Some(agent) = target {
                model.select(agent, effects);
            }
        }
        Context::Network | Context::Repos | Context::Timeline => {
            if let Some(agent) = view_target(model, context, action) {
                model.select(agent, effects);
            }
        }
        _ => {
            let index = model
                .selected
                .and_then(|id| model.rail.iter().position(|entry| entry.agent == id));
            let target = match (action, index) {
                (Action::First, _) | (Action::Down | Action::Up, None) => {
                    model.rail.first().map(|entry| entry.agent)
                }
                (Action::Last, _) => model.rail.last().map(|entry| entry.agent),
                (Action::Down, Some(index)) => model.rail.get(index + 1).map(|entry| entry.agent),
                (Action::Up, Some(index)) => index
                    .checked_sub(1)
                    .and_then(|i| model.rail.get(i))
                    .map(|entry| entry.agent),
                (Action::Left, _) => model.selected_agent().and_then(|agent| agent.parent),
                (Action::Right, _) => model.selected.and_then(|id| {
                    let children = model.registry.children(id);
                    children
                        .iter()
                        .find(|child| child.role != Role::Subagent)
                        .or(children.first())
                        .map(|child| child.id)
                }),
                _ => None,
            };
            if let Some(agent) = target {
                model.select(agent, effects);
            }
        }
    }
}

/// Where a move key goes in the network, repositories or timeline view; on
/// the timeline, left and right move the cursor instead of the selection.
fn view_target(model: &mut Model, context: Context, action: Action) -> Option<AgentId> {
    let selected = model.selected;
    match context {
        Context::Network => {
            let selectable: Vec<AgentId> = model.rail.iter().map(|entry| entry.agent).collect();
            let allowed = |id| selectable.contains(&id);
            match action {
                Action::First => model.network.corner(false, allowed),
                Action::Last => model.network.corner(true, allowed),
                _ => selected
                    .and_then(|from| model.network.neighbour(from, direction(action), allowed)),
            }
        }
        Context::Repos => match action {
            Action::Down => model.repos.step(selected, true),
            Action::Up => model.repos.step(selected, false),
            Action::Left => model.repos.jump_repo(selected, false),
            Action::Right => model.repos.jump_repo(selected, true),
            Action::First => model.repos.order.first().copied(),
            _ => model.repos.order.last().copied(),
        },
        _ => match action {
            Action::Left | Action::Right => {
                model.timeline.scrub(model.now, action == Action::Right);
                None
            }
            Action::Down => model.timeline.step_lane(selected, true),
            Action::Up => model.timeline.step_lane(selected, false),
            Action::First => model.timeline.lanes.first().copied(),
            _ => model.timeline.lanes.last().copied(),
        },
    }
}

/// The direction of a move action.
fn direction(action: Action) -> Direction {
    match action {
        Action::Down => Direction::Down,
        Action::Up => Direction::Up,
        Action::Left => Direction::Left,
        _ => Direction::Right,
    }
}

/// Handles the mouse; returns whether anything changed.
fn on_mouse(model: &mut Model, mouse: MouseEvent, effects: &mut Vec<Effect>) -> bool {
    let (column, row) = (mouse.column, mouse.row);
    if model.overlay.is_some() {
        if let Some(Overlay::Help { scroll }) = &mut model.overlay {
            match mouse.kind {
                MouseEventKind::ScrollDown => *scroll = scroll.saturating_add(2),
                MouseEventKind::ScrollUp => *scroll = scroll.saturating_sub(2),
                MouseEventKind::Down(_) => model.overlay = None,
                _ => return false,
            }
        }
        return true;
    }
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => match model.hit(column, row) {
            Some(Hit::Agent(agent, pane)) => {
                model.focus = pane;
                model.select(agent, effects);
                true
            }
            Some(Hit::Tab(tab)) => {
                model.tab = tab;
                true
            }
            Some(Hit::Pane(pane)) => {
                model.focus = pane;
                true
            }
            Some(Hit::Lane(agent)) => {
                model.focus = Pane::View;
                model.select(agent, effects);
                let geometry = model.timeline.geometry;
                let window = model.timeline.window(model.now);
                let offset = column.saturating_sub(geometry.cells_x());
                model.timeline.jump_to(window.start_of(offset), model.now);
                true
            }
            None => false,
        },
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            let down = mouse.kind == MouseEventKind::ScrollDown;
            match model.pane_at(column, row) {
                Some(Pane::Rail) => {
                    let action = if down { Action::Down } else { Action::Up };
                    navigate(model, Context::Agents, action, effects);
                }
                Some(Pane::View) => match model.tab {
                    Tab::Hierarchy => {
                        let step = if down { 2 } else { -2 };
                        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                            model.scene_offset.0 += step * 2;
                        } else {
                            model.scene_offset.1 += step;
                        }
                    }
                    Tab::Timeline if mouse.modifiers.contains(KeyModifiers::SHIFT) => {
                        model.timeline.scrub(model.now, down);
                    }
                    Tab::Repos | Tab::Timeline => {
                        let context = if model.tab == Tab::Repos {
                            Context::Repos
                        } else {
                            Context::Timeline
                        };
                        let action = if down { Action::Down } else { Action::Up };
                        navigate(model, context, action, effects);
                    }
                    Tab::Network => return false,
                },
                Some(Pane::Detail) => {
                    let action = if down { Action::Down } else { Action::Up };
                    navigate(model, Context::Detail, action, effects);
                }
                None => return false,
            }
            true
        }
        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
            if model.pane_at(column, row) == Some(Pane::View) =>
        {
            let right = mouse.kind == MouseEventKind::ScrollRight;
            match model.tab {
                Tab::Hierarchy => model.scene_offset.0 += if right { 4 } else { -4 },
                Tab::Timeline => model.timeline.scrub(model.now, right),
                Tab::Network | Tab::Repos => return false,
            }
            true
        }
        _ => false,
    }
}

/// Counts for the top bar: agents that need you, that work, and in all.
pub(crate) fn counters(registry: &Registry) -> (usize, usize, usize) {
    use volee_core::state::StateCategory;
    registry
        .agents()
        .fold((0, 0, 0), |(needs, working, all), agent| {
            match agent.state.category() {
                StateCategory::NeedsInput => (needs + 1, working, all + 1),
                StateCategory::Working => (needs, working + 1, all + 1),
                _ => (needs, working, all + 1),
            }
        })
}

#[cfg(test)]
mod tests;
