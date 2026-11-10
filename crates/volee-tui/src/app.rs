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
use volee_core::message::{MessageKind, Party};
use volee_core::role::Role;
use volee_core::time::Timestamp;
use volee_core::workspace::Location;
use volee_core::{Agent, AgentId, MessageId, Registry, ThreadId};

use crate::activity::ActivityLog;
use crate::editor::TextField;
use crate::history::History;
use crate::inbox::InboxView;
use crate::keymap::{Action, Context, KeyCombo, Keymap, Resolved};
use crate::launch::LaunchDialog;
use crate::layout::{Mode, ScreenLayout};
use crate::letters::Letters;
use crate::live::{Live, LiveInput};
use crate::preview::{Fit, PreviewScreen};
use crate::pulses::Pulses;
use crate::source::{
    Changes, MissionRequest, NewMission, Outgoing, PaneFocus, SourceError, SpawnRequest, Summary,
    TakeOver,
};
use crate::text;
use crate::theme::Theme;
use crate::tools::{Tool, ToolMenu};
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Overlay {
    /// The keys of the current screen, in a popup at the bottom right.
    Keys,
    /// Every key, scrolled by some lines.
    Help {
        /// Lines scrolled.
        scroll: u16,
    },
    /// The command palette.
    Commands(crate::commands::Palette),
    /// The theme picker: the theme shown, and the one to come back to.
    Themes {
        /// The theme shown, an index into [`Model::themes`].
        selected: usize,
        /// The theme there was when the picker opened.
        original: usize,
    },
    /// The message composer, editing [`Model::draft`].
    Composer,
    /// The changes of an agent.
    Diff(DiffView),
    /// The person's messages.
    Inbox(InboxView),
    /// The programs to open an agent's worktree with.
    Tools(ToolMenu),
    /// A dialog that launches an agent or a mission.
    Launch(Box<LaunchDialog>),
    /// The selected agent's screen, full size.
    Screen,
}

/// What the interface knows from the configuration and the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// Programs to open a worktree with.
    pub(crate) tools: Vec<Tool>,
    /// Repositories agents and missions can be launched in.
    pub(crate) repos: Vec<String>,
    /// Whether the source can use Claude Code: false when the daemon did
    /// not find it, or found a version too old for volee.
    pub(crate) claude_code_available: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            repos: Vec::new(),
            claude_code_available: true,
        }
    }
}

impl Settings {
    /// What a view without agents says when volee cannot use Claude Code on
    /// this machine, instead of waiting for agents that cannot come.
    pub(crate) fn without_claude_code(&self) -> Option<&'static str> {
        (!self.claude_code_available)
            .then_some("volee cannot use Claude Code here: run `volee daemon status` to see why.")
    }
}

/// A step that brings a pane forward failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FocusFailure {
    /// How many steps ran fine before it: from one on, the pane is in front
    /// in its own session.
    pub(crate) done: usize,
    /// The program that failed.
    pub(crate) program: String,
    /// What it said, or how it ended.
    pub(crate) reason: String,
}

/// A question asked on the bottom line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Prompt {
    /// `'` was pressed and the letter of an agent is expected.
    Jump,
    /// Quitting would lose the draft: quit anyway?
    Quit,
    /// Forget this agent: stop listing it?
    Forget(AgentId),
    /// This agent runs as a background session that volee only shows:
    /// adopt it into a host and take it over?
    AdoptAndTakeOver(AgentId),
}

/// The kinds of message the person can send, in the composer's order.
pub(crate) const MESSAGE_KINDS: [MessageKind; 5] = [
    MessageKind::Note,
    MessageKind::Question,
    MessageKind::Answer,
    MessageKind::Contract,
    MessageKind::Alert,
];

/// A message being written to an agent. It survives closing the composer,
/// so quitting asks first when it would be lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Draft {
    /// The recipient.
    pub(crate) to: AgentId,
    /// What the message is for.
    pub(crate) kind: MessageKind,
    /// The text and the cursor.
    pub(crate) field: TextField,
    /// The agent's question to the person that an answer replies to.
    pub(crate) question: Option<MessageId>,
    /// The thread it goes in, for a reply; the source picks one otherwise.
    pub(crate) thread: Option<ThreadId>,
    /// The source is sending it.
    pub(crate) sending: bool,
    /// The window to come back to when the composer closes.
    pub(crate) back: Option<Box<Overlay>>,
}

/// The changes window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffView {
    /// Whose changes.
    pub(crate) agent: AgentId,
    /// The full patch rather than the summary.
    pub(crate) full: bool,
    /// The changes, once loaded.
    pub(crate) changes: Option<Changes>,
    /// Why they could not be loaded.
    pub(crate) error: Option<String>,
    /// A request is under way.
    pub(crate) loading: bool,
    /// Lines scrolled.
    pub(crate) scroll: u16,
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
    /// The source cannot show this agent's screen: no refresh on its own.
    pub(crate) unsupported: bool,
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
    /// The source stopped sending events, and why when it said.
    SourceClosed(Option<String>),
    /// A preview arrived.
    PreviewLoaded {
        /// Whose.
        agent: AgentId,
        /// The raw screen, or why it failed.
        result: Result<String, SourceError>,
    },
    /// Changes arrived.
    DiffLoaded {
        /// Whose.
        agent: AgentId,
        /// The full patch rather than the summary.
        full: bool,
        /// The changes, or why they could not be read.
        result: Result<Changes, SourceError>,
    },
    /// The source said how to take over an agent, or why it cannot.
    TakeOverReady {
        /// The agent.
        agent: AgentId,
        /// The command, or why there is none.
        result: Result<TakeOver, SourceError>,
    },
    /// The source sent a message, or could not.
    MessageSent {
        /// The recipient.
        to: AgentId,
        /// How it went.
        result: Result<(), SourceError>,
    },
    /// The interface is back after a take over.
    Resumed {
        /// The agent that was taken over.
        agent: AgentId,
        /// How it ended.
        result: Result<(), String>,
    },
    /// The pane an agent runs in was brought forward, or could not be.
    PaneFocused {
        /// The agent.
        agent: AgentId,
        /// The pane.
        focus: PaneFocus,
        /// How it went.
        result: Result<(), FocusFailure>,
    },
    /// The source marked a message read, or could not.
    MarkedRead(Result<(), SourceError>),
    /// A thread's summary arrived, or why there is none.
    Summarized {
        /// The thread.
        thread: ThreadId,
        /// The summary, or why there is none.
        result: Result<Summary, SourceError>,
    },
    /// The source launched an agent, or refused.
    Launched(Result<AgentId, SourceError>),
    /// The source created a mission, or refused.
    MissionCreated(Result<NewMission, SourceError>),
    /// A tool ran in this terminal, or started in its own window.
    ToolFinished {
        /// The tool's name.
        label: String,
        /// How it went.
        result: Result<(), String>,
    },
    /// The source adopted an agent into a host, or refused.
    Adopted {
        /// The agent.
        agent: AgentId,
        /// The agent's name.
        name: String,
        /// How it went.
        result: Result<(), SourceError>,
    },
    /// The source forgot an agent, or refused.
    Forgot {
        /// The agent's name, kept from the request: once forgotten, the
        /// registry no longer has it.
        name: String,
        /// How it went.
        result: Result<(), SourceError>,
    },
    /// A volee host welcomed a live screen's connection.
    LiveOpened {
        /// Which connection.
        link: u64,
        /// The host's welcome.
        welcome: Box<volee_host::Welcome>,
    },
    /// A volee host sent a live screen an update.
    LiveUpdate {
        /// Which connection.
        link: u64,
        /// The update.
        update: volee_host::Update,
    },
    /// A live screen's connection ended, or could not open.
    LiveClosed {
        /// Which connection.
        link: u64,
        /// Why.
        reason: String,
    },
}

/// What the run loop must do after an update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Leave volee.
    Quit,
    /// Fetch the screen of an agent.
    LoadPreview(AgentId),
    /// Fetch the changes of an agent.
    LoadDiff {
        /// Whose.
        agent: AgentId,
        /// The full patch rather than the summary.
        full: bool,
    },
    /// Ask the source how to take over an agent.
    PrepareTakeOver(AgentId),
    /// Suspend the interface and run the take over in this terminal.
    TakeOver {
        /// The agent.
        agent: AgentId,
        /// What to run.
        take_over: TakeOver,
    },
    /// Bring forward the multiplexer pane an agent runs in, keeping the
    /// terminal.
    FocusPane {
        /// The agent.
        agent: AgentId,
        /// The pane, and the steps that bring it forward.
        focus: PaneFocus,
    },
    /// Send a message through the source.
    Send(Outgoing),
    /// Mark a message to the person read.
    MarkRead(MessageId),
    /// Ask for a thread's summary.
    Summarize(ThreadId),
    /// Launch an agent.
    Spawn(SpawnRequest),
    /// Create a mission.
    CreateMission(MissionRequest),
    /// Forget an agent: the source stops listing it.
    Forget {
        /// The agent.
        agent: AgentId,
        /// Its name, for the notice that follows.
        name: String,
    },
    /// Adopt an agent into a volee host.
    Adopt {
        /// The agent.
        agent: AgentId,
        /// Its name, for the notice that follows.
        name: String,
    },
    /// Keep the theme chosen in the picker.
    SaveTheme(String),
    /// Connect to an agent's volee host for its live screen: as a watcher,
    /// or as the input owner of edit mode, of `size` (ADR 0026).
    LiveOpen {
        /// The connection's number.
        link: u64,
        /// The host's socket.
        socket: std::path::PathBuf,
        /// Edit mode: take the input and give the agent `size`.
        edit: bool,
        /// The panel's size, for edit mode.
        size: volee_host::Size,
    },
    /// Send keys, a paste or a size to a live screen's host.
    LiveSend {
        /// The connection.
        link: u64,
        /// What to send.
        input: LiveInput,
    },
    /// Close a live screen's connection.
    LiveClose {
        /// The connection.
        link: u64,
    },
    /// Run a tool on a worktree: in this terminal, or detached in a window
    /// of its own.
    RunTool {
        /// Its name, for notices.
        label: String,
        /// Whether it opens a window of its own.
        detached: bool,
        /// What to run.
        take_over: TakeOver,
    },
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
    /// The keys of a sequence typed so far, such as the leader.
    pub(crate) pending_keys: Vec<KeyCombo>,
    /// The themes the picker offers.
    pub(crate) themes: crate::themes::ThemeMenu,
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
    /// The question on the bottom line, if any.
    pub(crate) prompt: Option<Prompt>,
    /// The window on top, if any.
    pub(crate) overlay: Option<Overlay>,
    /// The message at the bottom, if any.
    pub(crate) notice: Option<Notice>,
    /// The preview of the selected agent.
    pub(crate) preview: PreviewState,
    /// The message being written, if any.
    pub(crate) draft: Option<Draft>,
    /// The agent whose take over the source is preparing.
    pub(crate) pending_take_over: Option<AgentId>,
    /// The agent to take over as soon as its adoption is done.
    pub(crate) take_over_after_adopt: Option<AgentId>,
    /// The agent to edit as soon as the interface sees it in a volee host.
    pub(crate) edit_when_hosted: Option<AgentId>,
    /// The live screen of the full-screen view, for an agent in a volee
    /// host (ADR 0026).
    pub(crate) live: Option<Live>,
    /// The number of the last live screen connection.
    pub(crate) next_link: u64,
    /// Where volee hosts live, when the source runs agents in them.
    pub(crate) hosts: Option<volee_host::HostDir>,
    /// Why the source stopped, once it has: the screen no longer updates.
    pub(crate) offline: Option<String>,
    /// What the configuration and the machine offer.
    pub(crate) settings: Settings,
    /// An agent just launched, to select once it appears.
    pub(crate) awaited: Option<AgentId>,

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
            pending_keys: Vec::new(),
            themes: crate::themes::ThemeMenu::default(),
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
            prompt: None,
            overlay: None,
            notice: None,
            preview: PreviewState::default(),
            draft: None,
            pending_take_over: None,
            take_over_after_adopt: None,
            edit_when_hosted: None,
            live: None,
            next_link: 0,
            hosts: None,
            offline: None,
            settings: Settings::default(),
            awaited: None,
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
        if matches!(self.prompt, Some(Prompt::Quit | Prompt::Forget(_))) {
            return vec![Context::Confirm];
        }
        match self.overlay {
            Some(Overlay::Keys) => return vec![Context::Keys],
            Some(Overlay::Help { .. }) => return vec![Context::Help],
            Some(Overlay::Commands(_)) => return vec![Context::Commands],
            Some(Overlay::Themes { .. }) => return vec![Context::Themes],
            Some(Overlay::Composer) => return vec![Context::Composer],
            Some(Overlay::Diff(_)) => return vec![Context::Diff],
            Some(Overlay::Inbox(_)) => return vec![Context::Inbox],
            Some(Overlay::Tools(_)) => return vec![Context::Tools],
            Some(Overlay::Launch(_)) => return vec![Context::Dialog],
            Some(Overlay::Screen) if crate::edit::editing(self) => {
                return vec![Context::Attached];
            }
            Some(Overlay::Screen) => return vec![Context::Screen],
            None => {}
        }
        if self.filter.editing {
            return vec![Context::Filter];
        }
        self.screen_contexts()
    }

    /// The key contexts of the screen under any popup or window: the
    /// focused panel, then everywhere.
    pub(crate) fn screen_contexts(&self) -> Vec<Context> {
        let pane = match self.focus {
            Pane::Rail => Context::Agents,
            Pane::View => view_context(self.tab),
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
        self.prompt.is_some()
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

    /// Selects an agent now, or as soon as it appears.
    pub(crate) fn follow(&mut self, agent: AgentId) {
        if self.registry.agent(agent).is_some() {
            let mut effects = Vec::new();
            self.select(agent, &mut effects);
            // The end of the update asks for the new agent's screen.
            self.preview = PreviewState::default();
            self.awaited = None;
        } else {
            self.awaited = Some(agent);
        }
    }

    pub(crate) fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some(Notice {
            text: text.into(),
            error,
            until: self.now.plus_millis(NOTICE_MILLIS),
        });
    }

    pub(crate) fn name_of(&self, agent: AgentId) -> String {
        self.registry
            .agent(agent)
            .map_or_else(|| agent.short(), |agent| agent.name.clone())
    }

    /// Selects an agent, remembering the previous one.
    pub(crate) fn select(&mut self, agent: AgentId, effects: &mut Vec<Effect>) {
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

    pub(crate) fn request_preview(&mut self, agent: AgentId, effects: &mut Vec<Effect>) {
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
                    if self.awaited == Some(*agent) {
                        self.follow(*agent);
                    }
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
            } else if crate::edit::editing(model) {
                crate::edit::on_key(model, &key, &mut effects);
            } else {
                on_key(model, KeyCombo::from_event(&key), &mut effects);
            }
        }
        // In edit mode the mouse does nothing: the keys are the agent's.
        Msg::Mouse(_) if crate::edit::editing(model) => dirty = false,
        Msg::Mouse(mouse) => dirty = on_mouse(model, mouse, &mut effects),
        Msg::Paste(text) if crate::edit::editing(model) => {
            crate::edit::on_paste(model, &text, &mut effects);
        }
        Msg::Paste(text) => dirty = on_paste(model, &text),
        Msg::Resize { width, height } => {
            model.size = (width, height);
            crate::edit::on_resize(model, &mut effects);
        }
        live @ (Msg::LiveOpened { .. } | Msg::LiveUpdate { .. } | Msg::LiveClosed { .. }) => {
            dirty = crate::edit::on_msg(model, live, &mut effects);
        }
        Msg::Tick(now) => dirty = on_tick(model, now, &mut effects),
        Msg::Events(events) => model.apply(events),
        Msg::SourceClosed(reason) => {
            let reason = reason.unwrap_or_else(|| "the source stopped".to_owned());
            model.notify(
                format!(
                    "{}: what you see is no longer updated.",
                    text::sentence(&reason)
                ),
                true,
            );
            model.offline = Some(reason);
        }
        answer @ (Msg::PreviewLoaded { .. }
        | Msg::DiffLoaded { .. }
        | Msg::TakeOverReady { .. }
        | Msg::MessageSent { .. }
        | Msg::MarkedRead(_)
        | Msg::Summarized { .. }
        | Msg::Launched(_)
        | Msg::MissionCreated(_)
        | Msg::ToolFinished { .. }
        | Msg::Forgot { .. }
        | Msg::Adopted { .. }) => dirty = on_answer(model, answer, &mut effects),
        Msg::Resumed { agent, result } => resumed(model, agent, result, &mut effects),
        Msg::PaneFocused {
            agent,
            focus,
            result,
        } => {
            let name = model.name_of(agent);
            // Names that came from the multiplexer: shown without controls.
            let printable =
                |text: &str| -> String { text.chars().filter(|c| !c.is_control()).collect() };
            let (multiplexer, session) = (printable(&focus.multiplexer), printable(&focus.session));
            match result {
                Ok(()) => model.notify(
                    format!("{name} is in front: {multiplexer} pane, session {session}."),
                    false,
                ),
                // Focused in its session, which no window of the multiplexer
                // shows (none is attached, or another one would have to
                // switch): say where it is and how to see it.
                Err(failure) if failure.done > 0 && !focus.attach.is_empty() => {
                    let attach = printable(&text::command_line(&focus.attach));
                    model.notify(
                        format!(
                            "{name} is in front in {multiplexer} session {session}: run `{attach}` \
                             to see it."
                        ),
                        false,
                    );
                }
                Err(failure) => {
                    let reason = text::clause(&failure.reason);
                    let program = printable(&failure.program);
                    model.notify(
                        format!(
                            "Could not bring {name} forward in {multiplexer}: {program} says: \
                             {reason}."
                        ),
                        true,
                    );
                }
            }
        }
    }
    crate::edit::settle(model, &mut effects);
    model.relayout();
    // The layout pass may have moved the selection (the agent left, or the
    // filter hides it): the preview follows, and shows nothing once nothing
    // is selected, rather than the screen of an agent that left.
    match model.selected {
        Some(selected) if model.preview.agent != Some(selected) => {
            model.request_preview(selected, &mut effects);
        }
        None if model.preview.agent.is_some() => model.preview = PreviewState::default(),
        _ => {}
    }
    model.dirty |= dirty;
    effects
}

/// The interface is back after a take over in the terminal.
fn resumed(
    model: &mut Model,
    agent: AgentId,
    result: Result<(), String>,
    effects: &mut Vec<Effect>,
) {
    let name = model.name_of(agent);
    match result {
        Ok(()) => model.notify(format!("Back from {name}."), false),
        Err(error) => model.notify(format!("Could not take over {name}: {error}"), true),
    }
    if let Some(selected) = model.selected {
        model.request_preview(selected, effects);
    }
}

/// An answer of the source to a request; returns whether anything changed.
fn on_answer(model: &mut Model, answer: Msg, effects: &mut Vec<Effect>) -> bool {
    match answer {
        Msg::PreviewLoaded { agent, result } => {
            if model.preview.agent == Some(agent) {
                model.preview.loading = false;
                match result {
                    Ok(text) => {
                        model.preview.screen =
                            Some(PreviewScreen::parse(&text, model.theme.color_depth()));
                        model.preview.error = None;
                        model.preview.unsupported = false;
                    }
                    Err(error) => {
                        model.preview.unsupported = matches!(error, SourceError::Unsupported(_));
                        model.preview.error = Some(error.to_string());
                    }
                }
            } else {
                return false;
            }
        }
        Msg::DiffLoaded {
            agent,
            full,
            result,
        } => {
            if let Some(Overlay::Diff(diff)) = &mut model.overlay
                && diff.agent == agent
                && diff.full == full
            {
                diff.loading = false;
                match result {
                    Ok(changes) => {
                        diff.changes = Some(changes);
                        diff.error = None;
                    }
                    Err(error) => {
                        diff.changes = None;
                        diff.error = Some(error.to_string());
                    }
                }
            } else {
                return false;
            }
        }
        Msg::TakeOverReady { agent, result } => {
            model.pending_take_over = None;
            match result {
                Ok(TakeOver {
                    focus: Some(focus), ..
                }) => effects.push(Effect::FocusPane { agent, focus }),
                Ok(mut take_over) => {
                    // Said in the terminal as the agent's session starts.
                    let name = model.name_of(agent);
                    take_over.hint = Some(match take_over.hint {
                        Some(hint) => format!("Taking over {name}. {hint}"),
                        None => format!("Taking over {name}."),
                    });
                    effects.push(Effect::TakeOver { agent, take_over });
                }
                Err(error) => {
                    let name = model.name_of(agent);
                    let reason = error.to_string();
                    let reason = text::clause(&reason);
                    model.notify(format!("Cannot take over {name}: {reason}."), true);
                }
            }
        }
        Msg::MessageSent { to, result } => {
            let name = model.name_of(to);
            match result {
                Ok(()) => {
                    if model
                        .draft
                        .as_ref()
                        .is_some_and(|draft| draft.to == to && draft.sending)
                        && let Some(draft) = model.draft.take()
                        && model.overlay == Some(Overlay::Composer)
                    {
                        model.overlay = draft.back.map(|back| *back);
                    }
                    model.notify(format!("Message sent to {name}."), false);
                }
                Err(error) => {
                    if let Some(draft) = model.draft.as_mut().filter(|draft| draft.to == to) {
                        draft.sending = false;
                    }
                    let reason = error.to_string();
                    let reason = text::clause(&reason);
                    model.notify(format!("Could not send to {name}: {reason}."), true);
                }
            }
        }
        other => return on_window_answer(model, other, effects),
    }
    true
}

/// The answers to the requests of the messages, the tools and the launch
/// dialogs; returns whether anything changed.
fn on_window_answer(model: &mut Model, answer: Msg, effects: &mut Vec<Effect>) -> bool {
    match answer {
        Msg::MarkedRead(result) => {
            if let Err(error) = result {
                let reason = error.to_string();
                let reason = text::clause(&reason);
                model.notify(format!("Could not mark the message read: {reason}."), true);
            }
        }
        Msg::Summarized { thread, result } => {
            return crate::inbox::summarized(model, thread, result);
        }
        Msg::Launched(result) => return crate::launch::launched(model, result),
        Msg::MissionCreated(result) => return crate::launch::mission_created(model, result),
        Msg::ToolFinished { label, result } => match result {
            Ok(()) => model.notify(format!("Back from {label}."), false),
            Err(error) => {
                let reason = text::clause(&error);
                model.notify(format!("Could not open {label}: {reason}."), true);
            }
        },
        Msg::Adopted {
            agent,
            name,
            result,
        } => {
            let take_over = model.take_over_after_adopt.take() == Some(agent);
            match result {
                Ok(()) if take_over && model.hosts.is_some() => {
                    model.notify(format!("Adopted {name}: opening it in edit mode..."), false);
                    model.edit_when_hosted = Some(agent);
                }
                Ok(()) if take_over && model.pending_take_over.is_none() => {
                    model.notify(format!("Adopted {name}: taking it over..."), false);
                    model.pending_take_over = Some(agent);
                    effects.push(Effect::PrepareTakeOver(agent));
                }
                Ok(()) => model.notify(
                    format!(
                        "Adopted {name}: it continues in a volee host. Press i to type into it."
                    ),
                    false,
                ),
                Err(error) => {
                    let reason = text::clause(&error.to_string()).to_owned();
                    model.notify(format!("Could not adopt {name}: {reason}."), true);
                }
            }
        }
        Msg::Forgot { name, result } => match result {
            Ok(()) => model.notify(
                format!(
                    "Forgot {name}: volee no longer lists it. Its sessions, worktree and branch \
                     stay as they are."
                ),
                false,
            ),
            // The daemon's own words: it refuses an agent volee launched
            // that still runs, and an agent of an open mission.
            Err(error) => {
                let reason = text::clause(&error.to_string()).to_owned();
                model.notify(format!("Could not forget {name}: {reason}."), true);
            }
        },
        // The other messages are not answers.
        _ => return false,
    }
    true
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
    let live = model
        .selected
        .is_some_and(|agent| crate::edit::live_for(model, agent));
    let wanted =
        !model.preview.loading && !model.preview.unsupported && model.offline.is_none() && !live;
    if busy
        && wanted
        && due
        && let Some(selected) = model.selected
    {
        model.request_preview(selected, effects);
    }
    changed
}

/// A key while a question waits on the bottom line; returns whether a
/// question took it.
fn on_prompt(model: &mut Model, key: KeyCombo, effects: &mut Vec<Effect>) -> bool {
    match model.prompt {
        Some(Prompt::Jump) => {
            model.prompt = None;
            if let Some(letter) = key.typed_char().filter(char::is_ascii_alphabetic) {
                match model.letters.agent(letter) {
                    Some(agent) => model.select(agent, effects),
                    None => model.notify(format!("No agent has the letter {letter}."), false),
                }
            }
            true
        }
        Some(Prompt::Quit) => {
            match model.keymap.action(&[Context::Confirm], key) {
                Some(Action::Confirm | Action::Quit) => effects.push(Effect::Quit),
                Some(Action::Back) => model.prompt = None,
                _ => {}
            }
            true
        }
        Some(Prompt::AdoptAndTakeOver(agent)) => {
            match model.keymap.action(&[Context::Confirm], key) {
                Some(Action::Confirm) => {
                    model.prompt = None;
                    model.take_over_after_adopt = Some(agent);
                    let name = model.name_of(agent);
                    model.notify(format!("Adopting {name}..."), false);
                    effects.push(Effect::Adopt { agent, name });
                }
                Some(Action::Back) => model.prompt = None,
                Some(Action::Quit) => {
                    model.prompt = None;
                    request_quit(model, effects);
                }
                _ => {}
            }
            true
        }
        Some(Prompt::Forget(agent)) => {
            match model.keymap.action(&[Context::Confirm], key) {
                Some(Action::Confirm) => {
                    model.prompt = None;
                    let name = model.name_of(agent);
                    effects.push(Effect::Forget { agent, name });
                }
                Some(Action::Back) => model.prompt = None,
                Some(Action::Quit) => {
                    model.prompt = None;
                    request_quit(model, effects);
                }
                _ => {}
            }
            true
        }
        None => false,
    }
}

fn on_key(model: &mut Model, key: KeyCombo, effects: &mut Vec<Effect>) {
    if on_prompt(model, key, effects) {
        return;
    }
    if model.overlay == Some(Overlay::Composer) {
        match model.keymap.action(&[Context::Composer], key) {
            Some(action) => compose(model, action, effects),
            None => edit_draft(model, key),
        }
        return;
    }
    if matches!(model.overlay, Some(Overlay::Launch(_))) {
        crate::launch::on_key(model, key, effects);
        return;
    }
    if model.filter.editing {
        match model.keymap.action(&[Context::Filter], key) {
            Some(Action::Apply) => model.filter.editing = false,
            Some(Action::Back) => model.filter = Filter::default(),
            Some(Action::Quit) => request_quit(model, effects),
            _ => edit_filter(model, key),
        }
        return;
    }
    if matches!(model.overlay, Some(Overlay::Commands(_))) {
        crate::commands::on_key(model, key, effects);
        return;
    }
    if model.overlay == Some(Overlay::Keys) {
        match model.keymap.action(&[Context::Keys], key) {
            Some(Action::AllKeys) => {
                model.overlay = Some(Overlay::Help { scroll: 0 });
                return;
            }
            Some(Action::Back) => {
                model.overlay = None;
                return;
            }
            // Any other key closes the popup and does what it does.
            _ => model.overlay = None,
        }
    }
    let contexts = model.contexts();
    let pending = std::mem::take(&mut model.pending_keys);
    match model.keymap.resolve(&contexts, &pending, key) {
        Resolved::Action(context, action) => perform(model, context, action, effects),
        // The leader, `g`...: wait for the next key, the which-key popup
        // shows what it can be.
        Resolved::Prefix => {
            model.pending_keys = pending;
            model.pending_keys.push(key);
        }
        // Escape, or a key that continues nothing: the sequence is dropped.
        Resolved::Unbound => {}
    }
}

fn edit_filter(model: &mut Model, key: KeyCombo) {
    use ratatui::crossterm::event::KeyCode;
    match model.keymap.action(&[Context::Editing], key) {
        Some(Action::DeleteWord | Action::DeleteToLineStart) => model.filter.text.clear(),
        _ => match key.code() {
            KeyCode::Backspace => {
                model.filter.text.pop();
            }
            _ => {
                if let Some(c) = key.typed_char() {
                    model.filter.text.push(c);
                }
            }
        },
    }
}

/// Runs an action of a context, as its key does.
pub(crate) fn perform(
    model: &mut Model,
    context: Context,
    action: Action,
    effects: &mut Vec<Effect>,
) {
    if matches!(action, Action::HalfPageDown | Action::HalfPageUp) {
        half_page(model, context, action, effects);
        return;
    }
    if perform_windows(model, context, action, effects) || perform_finding(model, action, effects) {
        return;
    }
    match action {
        Action::Back => {
            if model.overlay == Some(Overlay::Screen) {
                crate::edit::close(model, effects);
            }
            back(model);
        }
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
        Action::Jump => model.prompt = Some(Prompt::Jump),
        Action::JumpBack => {
            if let Some(previous) = model.previous
                && model.registry.agent(previous).is_some()
            {
                model.select(previous, effects);
            }
        }
        Action::Filter => model.filter.editing = true,
        Action::TakeOver => take_over_selected(model, effects),
        Action::Message => open_composer(model),
        Action::FullScreen => open_full_screen(model, effects),
        Action::Forget => ask_forget(model),
        Action::Adopt => adopt_selected(model, effects),
        Action::ShowWall => show_wall(model, effects),
        Action::Diff => {
            if let Some(agent) = model.selected {
                open_diff(model, agent, false, effects);
            }
        }
        Action::ToggleFull => {
            if let Some(Overlay::Diff(diff)) = &model.overlay {
                let (agent, full) = (diff.agent, !diff.full);
                open_diff(model, agent, full, effects);
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
            if let Some(Overlay::Diff(diff)) = &model.overlay {
                let (agent, full) = (diff.agent, diff.full);
                open_diff(model, agent, full, effects);
            } else if let Some(agent) = model.selected {
                model.request_preview(agent, effects);
                // The live screen connects again: a new snapshot.
                if model.overlay == Some(Overlay::Screen) {
                    crate::edit::watch(model, agent, effects);
                }
            }
        }
        // The other actions belong to prompts and the composer, handled
        // before the keymap's contexts.
        _ => {}
    }
}

/// The keys of the messages, the tools and the launch dialogs, and the
/// keys that open them; returns whether the key was one of them.
fn perform_windows(
    model: &mut Model,
    context: Context,
    action: Action,
    effects: &mut Vec<Effect>,
) -> bool {
    match (context, action) {
        (_, Action::Quit) => request_quit(model, effects),
        (Context::Themes, _) => crate::themes::perform(model, action, effects),
        (Context::Inbox, _) => crate::inbox::perform(model, action, effects),
        (Context::Tools, _) => crate::tools::perform(model, action, effects),
        (_, Action::Messages) => crate::inbox::open(model),
        (_, Action::OpenWith) => {
            let agent = match &model.overlay {
                Some(Overlay::Diff(diff)) => Some(diff.agent),
                _ => model.selected,
            };
            if let Some(agent) = agent {
                crate::tools::open_menu(model, agent);
            }
        }
        (_, Action::NewAgent) => crate::launch::open(model, crate::launch::DialogKind::Agent),
        (_, Action::NewMission) => {
            crate::launch::open(model, crate::launch::DialogKind::Mission);
        }
        _ => return false,
    }
    true
}

/// The keys of the view a tab shows.
fn view_context(tab: Tab) -> Context {
    match tab {
        Tab::Hierarchy => Context::Hierarchy,
        Tab::Network => Context::Network,
        Tab::Repos => Context::Repos,
        Tab::Timeline => Context::Timeline,
    }
}

/// The zoom keys of a view: more or less detail, or the level the size
/// allows; on the timeline, a finer or coarser time scale, or back to now.
/// From the agents list, they act on the view beside it.
fn zoom(model: &mut Model, context: Context, action: Action) {
    let context = if context == Context::Agents {
        view_context(model.tab)
    } else {
        context
    };
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

/// Takes over the selected agent. A background session cannot be typed
/// into: volee offers to move it into a host first, then takes it over
/// (ADR 0023).
fn take_over_selected(model: &mut Model, effects: &mut Vec<Effect>) {
    let Some(agent) = model.selected else {
        return;
    };
    if model.pending_take_over.is_some() {
        return;
    }
    let background = model
        .registry
        .agent(agent)
        .is_some_and(|record| record.location == Location::Background);
    if background {
        model.prompt = Some(Prompt::AdoptAndTakeOver(agent));
    } else if !crate::edit::edit(model, agent, effects) {
        model.pending_take_over = Some(agent);
        effects.push(Effect::PrepareTakeOver(agent));
    }
}

/// Asks before forgetting the selected agent.
fn ask_forget(model: &mut Model) {
    if let Some(agent) = model.selected {
        model.prompt = Some(Prompt::Forget(agent));
    }
}

/// Asks the source to adopt the selected agent into a volee host.
fn adopt_selected(model: &mut Model, effects: &mut Vec<Effect>) {
    if let Some(agent) = model.selected {
        let name = model
            .registry
            .agent(agent)
            .map_or_else(|| agent.short(), |record| record.name.clone());
        model.notify(format!("Adopting {name}..."), false);
        effects.push(Effect::Adopt { agent, name });
    }
}

/// Opens the wall of screens (ADR 0021): the agents of the current list that
/// run in a volee host, as tiles of their live screens, in this terminal
/// until the person comes back with `q`.
fn show_wall(model: &mut Model, effects: &mut Vec<Effect>) {
    let hosts: Vec<String> = model
        .rail
        .iter()
        .filter_map(|entry| model.registry.agent(entry.agent))
        .filter(|agent| agent.state.is_live())
        .filter_map(|agent| agent.location.host().map(str::to_owned))
        .collect();
    if hosts.is_empty() {
        model.notify(
            "No agent of the list runs in a volee host: the wall shows agents volee launched \
             or adopted.",
            false,
        );
        return;
    }
    let Ok(program) = std::env::current_exe() else {
        model.notify("Cannot find the volee program to open the wall.", true);
        return;
    };
    effects.push(Effect::RunTool {
        label: "the wall".to_owned(),
        detached: false,
        take_over: TakeOver {
            program: program.display().to_string(),
            args: vec![
                "host".to_owned(),
                "wall".to_owned(),
                "--ids".to_owned(),
                hosts.join(","),
            ],
            cwd: None,
            env_remove: Vec::new(),
            hint: None,
            focus: None,
        },
    });
}

/// Shows the selected agent's screen full size, refreshed.
fn open_full_screen(model: &mut Model, effects: &mut Vec<Effect>) {
    if let Some(agent) = model.selected {
        model.overlay = Some(Overlay::Screen);
        model.preview.scroll = 0;
        model.request_preview(agent, effects);
        crate::edit::watch(model, agent, effects);
    }
}

/// Quits, unless a draft would be lost: then asks first.
pub(crate) fn request_quit(model: &mut Model, effects: &mut Vec<Effect>) {
    if model
        .draft
        .as_ref()
        .is_some_and(|draft| !draft.field.is_blank())
    {
        model.prompt = Some(Prompt::Quit);
    } else {
        effects.push(Effect::Quit);
    }
}

/// Opens the changes window on an agent and asks for its changes.
fn open_diff(model: &mut Model, agent: AgentId, full: bool, effects: &mut Vec<Effect>) {
    model.overlay = Some(Overlay::Diff(DiffView {
        agent,
        full,
        changes: None,
        error: None,
        loading: true,
        scroll: 0,
    }));
    effects.push(Effect::LoadDiff { agent, full });
}

/// Opens the composer on the selected agent, keeping the draft. A draft
/// written to another agent moves to this one. When the agent waits for an
/// answer from the person, the message starts as that answer.
fn open_composer(model: &mut Model) {
    let Some(agent) = model.selected else {
        model.notify("Select an agent to write to first.", false);
        return;
    };
    let question = model
        .registry
        .unanswered_questions()
        .into_iter()
        .rfind(|message| message.from == Party::Agent(agent))
        .map(|message| message.id);
    let kind = if question.is_some() {
        MessageKind::Answer
    } else {
        MessageKind::Note
    };
    match &mut model.draft {
        Some(draft) if draft.to == agent => {}
        Some(draft) => {
            draft.to = agent;
            draft.kind = kind;
            draft.question = question;
            draft.thread = None;
        }
        None => {
            model.draft = Some(Draft {
                to: agent,
                kind,
                field: TextField::default(),
                question,
                thread: None,
                sending: false,
                back: None,
            });
        }
    }
    model.overlay = Some(Overlay::Composer);
}

/// The composer's own keys.
fn compose(model: &mut Model, action: Action, effects: &mut Vec<Effect>) {
    match action {
        Action::Send => send_draft(model, effects),
        Action::NewLine => edit(model, |field| field.insert("\n")),
        Action::NextKind | Action::PreviousKind => {
            if let Some(draft) = model.draft.as_mut().filter(|draft| !draft.sending) {
                let index = MESSAGE_KINDS
                    .iter()
                    .position(|kind| *kind == draft.kind)
                    .unwrap_or(0);
                let count = MESSAGE_KINDS.len();
                let next = if action == Action::NextKind {
                    (index + 1) % count
                } else {
                    (index + count - 1) % count
                };
                draft.kind = MESSAGE_KINDS[next];
            }
        }
        Action::Back => {
            // Back to the window the composer was opened from, if any.
            model.overlay = model
                .draft
                .as_mut()
                .and_then(|draft| draft.back.take())
                .map(|back| *back);
            if model
                .draft
                .as_ref()
                .is_some_and(|draft| draft.field.is_blank() && !draft.sending)
            {
                model.draft = None;
            }
        }
        Action::Quit => request_quit(model, effects),
        _ => {}
    }
}

fn send_draft(model: &mut Model, effects: &mut Vec<Effect>) {
    let Some(draft) = model.draft.as_mut().filter(|draft| !draft.sending) else {
        return;
    };
    let body = draft.field.text().trim().to_owned();
    if body.is_empty() {
        model.notify("Write something first.", false);
        return;
    }
    draft.sending = true;
    effects.push(Effect::Send(Outgoing {
        to: draft.to,
        kind: draft.kind,
        body,
        in_reply_to: draft.question.filter(|_| draft.kind == MessageKind::Answer),
        thread: draft.thread,
    }));
}

/// Changes the draft's text, unless it is being sent.
fn edit(model: &mut Model, change: impl FnOnce(&mut TextField)) {
    if let Some(draft) = model.draft.as_mut().filter(|draft| !draft.sending) {
        change(&mut draft.field);
    }
}

/// Keys the composer does not bind edit the text.
fn edit_draft(model: &mut Model, key: KeyCombo) {
    let keymap = &model.keymap;
    if let Some(draft) = model.draft.as_mut().filter(|draft| !draft.sending) {
        crate::editor::apply_key(&mut draft.field, key, keymap);
    }
}

/// Pasted text goes to the composer or the filter prompt, whichever has
/// the keyboard; returns whether anything changed.
fn on_paste(model: &mut Model, text: &str) -> bool {
    if model.prompt.is_some() {
        return false;
    }
    if model.overlay == Some(Overlay::Composer) {
        let text = crate::editor::clean_paste(text);
        edit(model, |field| field.insert(&text));
        return true;
    }
    if let Some(Overlay::Launch(dialog)) = &mut model.overlay {
        crate::launch::paste(dialog, text);
        return true;
    }
    if model.filter.editing && model.overlay.is_none() {
        model
            .filter
            .text
            .extend(text.chars().filter(|c| !c.is_control()));
        return true;
    }
    false
}

/// Escape: closes or leaves the innermost thing, and never quits.
fn back(model: &mut Model) {
    if let Some(overlay) = model.overlay.take() {
        if matches!(overlay, Overlay::Screen) {
            // The preview counts rows, the full-screen view wrapped lines.
            model.preview.scroll = 0;
        }
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
            let most = crate::widgets::help::max_scroll(model);
            if let Some(Overlay::Help { scroll }) = &mut model.overlay {
                *scroll = scrolled(*scroll, action, most);
            }
        }
        Context::Diff => {
            let size = model.size;
            if let Some(Overlay::Diff(diff)) = &mut model.overlay {
                let most = crate::widgets::diff::max_scroll(diff, size);
                diff.scroll = scrolled(diff.scroll, action, most);
            }
        }
        Context::Detail => {
            let scroll = &mut model.preview.scroll;
            *scroll = match action {
                Action::Up => scroll.saturating_add(1),
                _ => scroll.saturating_sub(1),
            };
        }
        Context::Screen => scroll_screen(model, action),
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

/// A window's scroll after a key, between 0 and `most` lines.
/// The keys that help find things: the keys popup, every key, the palette,
/// the theme picker, the matches of the filter and the views; returns
/// whether the action was one of them.
fn perform_finding(model: &mut Model, action: Action, effects: &mut Vec<Effect>) -> bool {
    match action {
        Action::Help => {
            model.overlay = match model.overlay {
                Some(Overlay::Keys) => None,
                _ => Some(Overlay::Keys),
            };
        }
        Action::AllKeys => model.overlay = Some(Overlay::Help { scroll: 0 }),
        Action::Commands => crate::commands::open(model),
        Action::PickTheme => crate::themes::open(model),
        Action::NextMatch | Action::PreviousMatch => {
            next_match(model, action == Action::NextMatch, effects);
        }
        Action::PreviousView | Action::NextView => {
            let tabs = [Tab::Hierarchy, Tab::Network, Tab::Repos, Tab::Timeline];
            let index = tabs.iter().position(|tab| *tab == model.tab).unwrap_or(0);
            let count = tabs.len();
            model.tab = if action == Action::NextView {
                tabs[(index + 1) % count]
            } else {
                tabs[(index + count - 1) % count]
            };
        }
        _ => return false,
    }
    true
}

/// Half a page down or up: in the agents list, half its rows at once;
/// elsewhere, that many lines of scrolling.
fn half_page(model: &mut Model, context: Context, action: Action, effects: &mut Vec<Effect>) {
    let rows = usize::from(model.size.1.saturating_sub(4) / 2).max(1);
    let down = action == Action::HalfPageDown;
    if context == Context::Agents {
        let last = model.rail.len().saturating_sub(1);
        let index = model
            .selected
            .and_then(|id| model.rail.iter().position(|entry| entry.agent == id))
            .unwrap_or(0);
        let target = if down {
            (index + rows).min(last)
        } else {
            index.saturating_sub(rows)
        };
        if let Some(entry) = model.rail.get(target) {
            let agent = entry.agent;
            model.select(agent, effects);
        }
        return;
    }
    let step = if down { Action::Down } else { Action::Up };
    for _ in 0..rows {
        perform(model, context, step, effects);
    }
}

/// Scrolls the agent's screen in the full-screen view.
fn scroll_screen(model: &mut Model, action: Action) {
    // A live screen drawn cell for cell scrolls through its history.
    let area = crate::edit::screen_area(model.size);
    if let Some(live) = model
        .live
        .as_mut()
        .filter(|live| live.has_screen() && live.fits(area))
    {
        match action {
            Action::Up => {
                live.scroll_by(1);
            }
            Action::Down => {
                live.scroll_by(-1);
            }
            Action::First => {
                live.scroll_by(isize::MAX);
            }
            _ => live.scroll_to_bottom(),
        }
        return;
    }
    // The screen fills the terminal but its frame, its margin and
    // the hint line: it scrolls until its first line is in sight.
    let (columns, rows) = model.size;
    let live = model
        .live
        .as_ref()
        .and_then(|live| live.preview(&model.theme));
    let lines = live
        .as_ref()
        .or(model.preview.screen.as_ref())
        .map_or(0, |screen| {
            screen.line_count(columns.saturating_sub(3), Fit::Wrap)
        });
    let most = lines.saturating_sub(rows.saturating_sub(3));
    let scroll = &mut model.preview.scroll;
    *scroll = match action {
        Action::Up => scroll.saturating_add(1).min(most),
        Action::First => most,
        Action::Last => 0,
        _ => scroll.saturating_sub(1),
    };
}

/// `n` and `N`: the next or previous agent of the filtered list, round it.
fn next_match(model: &mut Model, forward: bool, effects: &mut Vec<Effect>) {
    if model.filter.needle().is_none() {
        model.notify(
            "n and N go through the agents that match the filter: press / and type first.",
            false,
        );
        return;
    }
    let count = model.rail.len();
    if count == 0 {
        model.notify("No agent matches the filter.", false);
        return;
    }
    let next = match model
        .selected
        .and_then(|id| model.rail.iter().position(|entry| entry.agent == id))
    {
        Some(index) if forward => (index + 1) % count,
        Some(index) => (index + count - 1) % count,
        None => 0,
    };
    let agent = model.rail[next].agent;
    model.select(agent, effects);
}

fn scrolled(scroll: u16, action: Action, most: u16) -> u16 {
    let scroll = match action {
        Action::Down => scroll.saturating_add(1),
        Action::Up => scroll.saturating_sub(1),
        Action::First => 0,
        Action::Last => most,
        _ => scroll,
    };
    scroll.min(most)
}

/// The wheel over the view: the hierarchy scrolls (`sideways` with Shift),
/// the repositories and the timeline move the selection, and Shift moves
/// the timeline's cursor. Returns whether anything changed.
fn scroll_view(model: &mut Model, down: bool, sideways: bool, effects: &mut Vec<Effect>) -> bool {
    match model.tab {
        Tab::Hierarchy => {
            let step = if down { 2 } else { -2 };
            if sideways {
                model.scene_offset.0 += step * 2;
            } else {
                model.scene_offset.1 += step;
            }
        }
        Tab::Timeline if sideways => model.timeline.scrub(model.now, down),
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
    }
    true
}

/// The mouse over a window: the wheel scrolls it, a click closes one that
/// holds nothing typed; returns whether anything changed.
fn on_overlay_mouse(model: &mut Model, mouse: MouseEvent, effects: &mut Vec<Effect>) -> bool {
    let action = match mouse.kind {
        MouseEventKind::ScrollDown => Action::Down,
        MouseEventKind::ScrollUp => Action::Up,
        // A click closes a window that holds nothing typed; the
        // composer, the messages and the dialogs close with keys.
        MouseEventKind::Down(_)
            if matches!(
                model.overlay,
                Some(
                    Overlay::Keys
                        | Overlay::Help { .. }
                        | Overlay::Diff(_)
                        | Overlay::Tools(_)
                        | Overlay::Commands(_)
                )
            ) =>
        {
            model.overlay = None;
            return true;
        }
        // The theme picker goes back to the theme there was.
        MouseEventKind::Down(_) if matches!(model.overlay, Some(Overlay::Themes { .. })) => {
            crate::themes::perform(model, Action::Back, effects);
            return true;
        }
        _ => return false,
    };
    let context = match model.overlay {
        Some(Overlay::Help { .. }) => Context::Help,
        Some(Overlay::Diff(_)) => Context::Diff,
        Some(Overlay::Inbox(_)) => {
            for _ in 0..2 {
                crate::inbox::perform(model, action, effects);
            }
            return true;
        }
        _ => return false,
    };
    for _ in 0..2 {
        navigate(model, context, action, effects);
    }
    true
}

/// Handles the mouse; returns whether anything changed.
fn on_mouse(model: &mut Model, mouse: MouseEvent, effects: &mut Vec<Effect>) -> bool {
    let (column, row) = (mouse.column, mouse.row);
    if model.prompt.is_some() {
        return false;
    }
    if model.overlay.is_some() {
        return on_overlay_mouse(model, mouse, effects);
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
                Some(Pane::View) => {
                    let sideways = mouse.modifiers.contains(KeyModifiers::SHIFT);
                    return scroll_view(model, down, sideways, effects);
                }
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
