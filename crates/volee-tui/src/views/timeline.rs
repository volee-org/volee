//! The timeline view: what happened when.
//!
//! One lane per agent, in the hierarchy's order so lanes never jump, over
//! the last half hour by default. A lane draws the agent's state as a bar,
//! heavy while it works or waits for you, thin while it is idle or done,
//! dotted once it stopped, and marks where something happened: it needed
//! you, finished or failed, asked a question, sent or received a message,
//! delivered (commits, push, pull request, merge, report) or got a verdict
//! from its mission's supervisor. A cursor scrubs through time, and the line
//! under the lanes tells what happened under it.
//!
//! Columns are buckets of whole multiples of the scale's step, so marks
//! stay in their column and the picture slides one column at a time.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use volee_core::message::{MessageKind, Party};
use volee_core::mission::Verdict;
use volee_core::state::StateCategory;
use volee_core::time::Timestamp;
use volee_core::{Agent, AgentId, Registry};

use super::hierarchy;
use crate::app::Model;
use crate::history::{Mark, MarkKind};
use crate::text;
use crate::theme::StyleRole;
use crate::widgets::{placeholder, put_spans, state_glyph, truncated};

/// The steps of the time scale, in milliseconds per column.
pub(crate) const STEPS: [i64; 13] = [
    1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000,
    1_800_000, 3_600_000,
];

/// What the automatic scale covers at least: the last half hour.
const DEFAULT_SPAN_MILLIS: i64 = 30 * 60_000;

/// The intervals between two labels of the time axis.
const TICKS: [i64; 15] = [
    10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000, 1_800_000, 3_600_000,
    7_200_000, 10_800_000, 21_600_000, 43_200_000, 86_400_000,
];

/// The time scale the person asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Scale {
    /// The finest step that shows the last half hour.
    #[default]
    Auto,
    /// This entry of [`STEPS`].
    Fixed(usize),
}

/// Where things go in the view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Geometry {
    /// The time axis, above the lanes.
    pub(crate) axis: Option<Rect>,
    /// The lanes: labels and cells.
    pub(crate) lanes: Rect,
    /// The width of the labels at the left of the lanes.
    pub(crate) label: u16,
    /// The line under the lanes that reads the cursor.
    pub(crate) readout: Option<Rect>,
}

impl Geometry {
    /// The geometry of a view area.
    pub(crate) fn of(area: Rect) -> Self {
        let roomy = area.height >= 5;
        let label = (area.width / 4).clamp(16.min(area.width), 24);
        let top = u16::from(roomy);
        let lanes = Rect::new(
            area.x,
            area.y + top,
            area.width,
            area.height.saturating_sub(2 * top),
        );
        Self {
            axis: roomy.then(|| Rect::new(area.x, area.y, area.width, 1)),
            lanes,
            label,
            readout: roomy.then(|| Rect::new(area.x, area.bottom() - 1, area.width, 1)),
        }
    }

    /// How many columns of time fit.
    pub(crate) fn columns(&self) -> u16 {
        self.lanes.width.saturating_sub(self.label + 1)
    }

    /// Where the columns of time start.
    pub(crate) fn cells_x(&self) -> u16 {
        self.lanes.x + self.label + 1
    }
}

/// The timeline's state.
#[derive(Debug, Default)]
pub(crate) struct TimelineState {
    /// The time scale the person asked for.
    pub(crate) scale: Scale,
    /// The moment under the cursor (the start of its column); `None` follows
    /// the present.
    pub(crate) cursor: Option<Timestamp>,
    /// The agents with a lane, top to bottom.
    pub(crate) lanes: Vec<AgentId>,
    /// How deep each lane's agent sits in the hierarchy.
    pub(crate) depths: Vec<u16>,
    /// The first lane shown.
    pub(crate) offset: usize,
    /// Where things go, for the area of the last layout.
    pub(crate) geometry: Geometry,
}

/// The time window on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Window {
    /// Milliseconds per column.
    pub(crate) step: i64,
    /// The bucket of the leftmost column.
    pub(crate) left: i64,
    /// How many columns.
    pub(crate) columns: u16,
    /// The bucket of the present.
    pub(crate) now: i64,
}

impl Window {
    /// The start of a column's bucket.
    pub(crate) fn start_of(&self, column: u16) -> Timestamp {
        Timestamp::from_millis((self.left + i64::from(column)) * self.step)
    }

    /// The column of a moment, if it is on screen.
    pub(crate) fn column_of(&self, at: Timestamp) -> Option<u16> {
        let column = at.as_millis().div_euclid(self.step) - self.left;
        u16::try_from(column)
            .ok()
            .filter(|column| *column < self.columns)
    }
}

impl TimelineState {
    /// Brings the lanes up to date for the view area `area`, keeping only
    /// the agents that pass `matches`, and scrolls to keep `selected` in
    /// view.
    pub(crate) fn relayout(
        &mut self,
        registry: &Registry,
        matches: &dyn Fn(&Agent) -> bool,
        area: Rect,
        selected: Option<AgentId>,
    ) {
        self.geometry = Geometry::of(area);
        let (lanes, depths): (Vec<AgentId>, Vec<u16>) = hierarchy::outline(registry, None)
            .into_iter()
            .filter(|(agent, _)| registry.agent(*agent).is_some_and(matches))
            .map(|(agent, guides)| {
                let depth = u16::try_from(guides.chars().count() / 3).unwrap_or(0);
                (agent, depth)
            })
            .unzip();
        self.lanes = lanes;
        self.depths = depths;
        let visible = usize::from(self.geometry.lanes.height);
        let mut offset = self.offset.min(self.lanes.len().saturating_sub(visible));
        if let Some(index) = selected.and_then(|agent| self.lanes.iter().position(|a| *a == agent))
        {
            if index < offset {
                offset = index;
            } else if visible > 0 && index >= offset + visible {
                offset = index + 1 - visible;
            }
        }
        self.offset = offset;
    }

    /// The index in [`STEPS`] of the scale in use.
    pub(crate) fn step_index(&self) -> usize {
        match self.scale {
            Scale::Fixed(index) => index.min(STEPS.len() - 1),
            Scale::Auto => {
                let columns = i64::from(self.geometry.columns().max(1));
                STEPS
                    .iter()
                    .position(|step| step * columns >= DEFAULT_SPAN_MILLIS)
                    .unwrap_or(STEPS.len() - 1)
            }
        }
    }

    /// The time window at `now`: it ends with the present, unless the cursor
    /// went further back than the window reaches.
    pub(crate) fn window(&self, now: Timestamp) -> Window {
        let step = STEPS[self.step_index()];
        let columns = self.geometry.columns().max(1);
        let span = i64::from(columns) - 1;
        let now_bucket = now.as_millis().div_euclid(step);
        let right = match self.cursor {
            Some(cursor) if cursor.as_millis().div_euclid(step) < now_bucket - span => {
                cursor.as_millis().div_euclid(step) + span
            }
            _ => now_bucket,
        };
        Window {
            step,
            left: right - span,
            columns,
            now: now_bucket,
        }
    }

    /// The bucket under the cursor at `now`.
    fn cursor_bucket(&self, now: Timestamp) -> i64 {
        let step = STEPS[self.step_index()];
        self.cursor
            .unwrap_or(now)
            .as_millis()
            .div_euclid(step)
            .min(now.as_millis().div_euclid(step))
    }

    /// Moves the cursor one column back or forward; past the present, it
    /// follows the present again.
    pub(crate) fn scrub(&mut self, now: Timestamp, forward: bool) {
        let step = STEPS[self.step_index()];
        let bucket = self.cursor_bucket(now) + if forward { 1 } else { -1 };
        self.set_cursor(bucket, step, now);
    }

    /// Moves the cursor to the bucket of `at`.
    pub(crate) fn jump_to(&mut self, at: Timestamp, now: Timestamp) {
        let step = STEPS[self.step_index()];
        self.set_cursor(at.as_millis().div_euclid(step), step, now);
    }

    fn set_cursor(&mut self, bucket: i64, step: i64, now: Timestamp) {
        self.cursor = (bucket < now.as_millis().div_euclid(step))
            .then(|| Timestamp::from_millis(bucket * step));
    }

    /// The moment of the selected agent's previous (or next) mark that
    /// shows on its lane, from the cursor.
    pub(crate) fn event_from(
        &self,
        marks: &[Mark],
        now: Timestamp,
        forward: bool,
    ) -> Option<Timestamp> {
        let step = STEPS[self.step_index()];
        let bucket = self.cursor_bucket(now);
        let mut shown = marks
            .iter()
            .filter(|mark| glyph(mark.kind).is_some())
            .map(|mark| mark.at);
        if forward {
            shown.find(|at| at.as_millis().div_euclid(step) > bucket)
        } else {
            shown.rfind(|at| at.as_millis().div_euclid(step) < bucket)
        }
    }

    /// More detail (a finer step) or less.
    pub(crate) fn zoom(&mut self, finer: bool) {
        let index = self.step_index();
        let next = if finer {
            index.saturating_sub(1)
        } else {
            (index + 1).min(STEPS.len() - 1)
        };
        self.scale = Scale::Fixed(next);
    }

    /// Back to the present and the automatic scale.
    pub(crate) fn reset(&mut self) {
        self.scale = Scale::Auto;
        self.cursor = None;
    }

    /// The agent `forward` or back from `from` among the lanes.
    pub(crate) fn step_lane(&self, from: Option<AgentId>, forward: bool) -> Option<AgentId> {
        let index = from.and_then(|agent| self.lanes.iter().position(|a| *a == agent));
        match (index, forward) {
            (None, _) => self.lanes.first().copied(),
            (Some(index), true) => self.lanes.get(index + 1).copied(),
            (Some(index), false) => index
                .checked_sub(1)
                .and_then(|i| self.lanes.get(i).copied()),
        }
    }
}

/// The state of each column: the most urgent state the agent was in during
/// the column's time; `None` before the agent existed.
pub(crate) fn column_states(
    states: &[(Timestamp, StateCategory)],
    window: &Window,
) -> Vec<Option<StateCategory>> {
    let mut index = 0;
    (0..window.columns)
        .map(|column| {
            let start = window.start_of(column);
            let end = start.plus_millis(window.step);
            while index + 1 < states.len() && states[index + 1].0 <= start {
                index += 1;
            }
            let mut state = states
                .get(index)
                .filter(|(since, _)| *since <= start)
                .map(|(_, state)| *state);
            for (since, next) in states.iter().skip(index) {
                if *since >= end {
                    break;
                }
                if *since > start || state.is_none() {
                    state = Some(state.map_or(*next, |current| current.min(*next)));
                }
            }
            state
        })
        .collect()
}

/// The mark of each column that a person most needs to see.
pub(crate) fn column_marks<'a>(marks: &'a [Mark], window: &Window) -> Vec<Option<&'a Mark>> {
    let mut columns: Vec<Option<&Mark>> = vec![None; usize::from(window.columns)];
    let first = window.start_of(0);
    let start = marks.partition_point(|mark| mark.at < first);
    for mark in &marks[start..] {
        if glyph(mark.kind).is_none() {
            continue;
        }
        let Some(column) = window.column_of(mark.at) else {
            if mark.at > first {
                break;
            }
            continue;
        };
        let slot = &mut columns[usize::from(column)];
        if slot.is_none_or(|current| mark.kind.priority() >= current.kind.priority()) {
            *slot = Some(mark);
        }
    }
    columns
}

/// The glyph and the style role of a mark, for the marks a lane shows. A
/// lane's bar already shows when an agent starts working or goes idle.
fn glyph(kind: MarkKind) -> Option<(&'static str, StyleRole)> {
    Some(match kind {
        MarkKind::State(StateCategory::NeedsInput) => ("!", StyleRole::StateNeedsInput),
        MarkKind::State(StateCategory::Failed) => ("✗", StyleRole::StateFailed),
        MarkKind::State(StateCategory::Lost) => ("∅", StyleRole::StateLost),
        MarkKind::State(StateCategory::Done) => ("✓", StyleRole::StateDone),
        MarkKind::State(_) => return None,
        MarkKind::Sent {
            kind: MessageKind::Question,
            ..
        } => ("?", StyleRole::EdgeQuestion),
        MarkKind::Sent { .. } => ("◆", StyleRole::Text),
        MarkKind::Received { .. } => ("◇", StyleRole::TextDim),
        MarkKind::Delivery(_) => ("●", StyleRole::BadgePr),
        MarkKind::Verdict(verdict) => (
            "◉",
            match verdict {
                Verdict::Delivered => StyleRole::StateDone,
                Verdict::NeedsHuman => StyleRole::StateNeedsInput,
                Verdict::NotDelivered => StyleRole::ChecksFailing,
                Verdict::Failed => StyleRole::StateFailed,
                Verdict::Lost => StyleRole::StateLost,
            },
        ),
    })
}

/// The bar of a state, and its style role.
const fn bar(state: StateCategory) -> (&'static str, StyleRole) {
    match state {
        StateCategory::Working => ("━", StyleRole::StateWorking),
        StateCategory::NeedsInput => ("━", StyleRole::StateNeedsInput),
        StateCategory::Failed => ("━", StyleRole::StateFailed),
        StateCategory::Launching => ("╍", StyleRole::StateLaunching),
        StateCategory::Done => ("─", StyleRole::StateDone),
        StateCategory::Idle => ("─", StyleRole::StateIdle),
        StateCategory::Stopped => ("┄", StyleRole::StateStopped),
        StateCategory::Lost => ("┄", StyleRole::StateLost),
    }
}

/// The label of a state category.
const fn state_label(state: StateCategory) -> &'static str {
    match state {
        StateCategory::NeedsInput => "needs you",
        StateCategory::Failed => "failed",
        StateCategory::Working => "working",
        StateCategory::Launching => "launching",
        StateCategory::Done => "done",
        StateCategory::Idle => "idle",
        StateCategory::Stopped => "stopped",
        StateCategory::Lost => "lost",
    }
}

/// A span of time for people: `45 s`, `5 min`, `2 h`, `90 min`.
pub(crate) fn span_label(millis: i64) -> String {
    let seconds = millis / 1_000;
    if seconds > 0 && seconds % 3_600 == 0 {
        format!("{} h", seconds / 3_600)
    } else if seconds >= 60 && seconds % 60 == 0 {
        format!("{} min", seconds / 60)
    } else {
        format!("{seconds} s")
    }
}

/// About how long, rounded to the minute past one: `45 s`, `33 min`,
/// `2 h 05 min`.
fn rough(millis: i64) -> String {
    let seconds = millis.max(0) / 1_000;
    let minutes = (seconds + 30) / 60;
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3_600 => format!("{minutes} min"),
        _ if minutes % 60 == 0 => format!("{} h", minutes / 60),
        _ => format!("{} h {:02} min", minutes / 60, minutes % 60),
    }
}

/// How long ago, to the second: `4 min 10 s ago`, `1 h 05 min ago`.
fn ago(millis: i64) -> String {
    let seconds = millis.max(0) / 1_000;
    match seconds {
        0..60 => format!("{seconds} s ago"),
        60..3_600 => format!("{} min {:02} s ago", seconds / 60, seconds % 60),
        _ => format!("{} h {:02} min ago", seconds / 3_600, seconds % 3_600 / 60),
    }
}

/// The view's title after its name: the span and the step.
pub(crate) fn title(model: &Model) -> Vec<Span<'static>> {
    let state = &model.timeline;
    let window = state.window(model.now);
    let span = rough(window.step * i64::from(window.columns));
    let fixed = if state.scale == Scale::Auto {
        ""
    } else {
        " (fixed)"
    };
    vec![Span::styled(
        format!(" · {span} · {} per column{fixed}", span_label(window.step)),
        model.theme.style(StyleRole::TextDim),
    )]
}

/// Draws the timeline in `area`, the inside of the view panel.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    if area.intersection(buf.area) != area || area.is_empty() {
        return;
    }
    let theme = &model.theme;
    let state = &model.timeline;
    let geometry = state.geometry;
    if state.lanes.is_empty() {
        let message = if model.registry.agents().next().is_none() {
            model
                .settings
                .without_claude_code()
                .unwrap_or("No agents yet: their states and messages will show here as they come.")
        } else {
            "No agent matches the filter."
        };
        placeholder(model, message, area, buf);
        return;
    }
    let window = state.window(model.now);
    let cursor = u16::try_from(state.cursor_bucket(model.now) - window.left)
        .ok()
        .filter(|column| *column < window.columns);
    if let Some(axis) = geometry.axis {
        draw_axis(model, &window, cursor, axis, buf);
    }
    let lanes = geometry.lanes;
    for (row, (index, agent)) in (0u16..).zip(state.lanes.iter().enumerate().skip(state.offset)) {
        if row >= lanes.height {
            break;
        }
        let depth = state.depths.get(index).copied().unwrap_or(0);
        draw_lane(model, *agent, depth, &window, lanes.y + row, buf);
    }
    let shown = u16::try_from(state.lanes.len().saturating_sub(state.offset))
        .unwrap_or(u16::MAX)
        .min(lanes.height);
    if let Some(column) = cursor {
        // A light column through the axis and the lanes.
        let x = geometry.cells_x() + column;
        let style = Style {
            bg: theme.style(StyleRole::Selection).bg,
            ..Style::default()
        };
        let top = geometry.axis.map_or(lanes.y, |axis| axis.y);
        buf.set_style(Rect::new(x, top, 1, shown + (lanes.y - top)), style);
    }
    // Arrows on the panel's border tell that the lanes go on.
    let arrow = theme.style(StyleRole::TextDim);
    let column = area.right().saturating_sub(2);
    if state.offset > 0
        && let Some(row) = area.y.checked_sub(1)
    {
        buf.set_string(column, row, "▲", arrow);
    }
    if state.offset + usize::from(lanes.height) < state.lanes.len()
        && area.bottom() < buf.area.bottom()
    {
        buf.set_string(column, area.bottom(), "▼", arrow);
    }
    if let Some(readout) = geometry.readout {
        // The legend takes a spare line above the readout.
        if shown + 2 <= lanes.height {
            draw_legend(model, Rect::new(area.x, readout.y - 1, area.width, 1), buf);
        }
        draw_readout(model, &window, readout, buf);
    }
}

/// What the bars and marks mean.
fn draw_legend(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let mut spans = Vec::new();
    let items = [
        ("━", StyleRole::StateWorking, "working"),
        ("━", StyleRole::StateNeedsInput, "needs you"),
        ("─", StyleRole::StateIdle, "idle"),
        ("─", StyleRole::StateDone, "done"),
        ("┄", StyleRole::StateStopped, "stopped"),
        ("◆", StyleRole::Text, "sent"),
        ("◇", StyleRole::TextDim, "received"),
        ("?", StyleRole::EdgeQuestion, "question"),
        ("●", StyleRole::BadgePr, "delivery"),
        ("◉", StyleRole::StateDone, "verdict"),
    ];
    for (index, (symbol, role, words)) in items.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(symbol, theme.style(role)));
        spans.push(Span::styled(format!(" {words}"), dim));
    }
    let room = area.width.saturating_sub(2);
    put_spans(buf, area.x + 1, area.y, &truncated(&spans, room), room);
}

/// The time axis: labels of time ago, right-aligned on their column, and
/// the cursor's mark.
fn draw_axis(model: &Model, window: &Window, cursor: Option<u16>, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let geometry = model.timeline.geometry;
    let dim = theme.style(StyleRole::TextDim);
    let x0 = geometry.cells_x();
    let tick = TICKS
        .iter()
        .copied()
        .find(|tick| tick % window.step == 0 && tick / window.step >= 12)
        .unwrap_or(window.step * 12);
    let mut free_from = area.x + geometry.label + 1;
    for column in 0..window.columns {
        let ago = (window.now - (window.left + i64::from(column))) * window.step;
        if ago % tick != 0 {
            continue;
        }
        let label = if ago == 0 {
            "now".to_owned()
        } else {
            format!("-{}", span_label(ago))
        };
        let width = u16::try_from(text::width(&label)).unwrap_or(0);
        let end = x0 + column + 1;
        if end < width || end - width < free_from || end > area.right() {
            continue;
        }
        buf.set_string(end - width, area.y, &label, dim);
        free_from = end + 1;
    }
    // A scrubbed cursor is marked on the axis, where no label is.
    if model.timeline.cursor.is_some()
        && let Some(column) = cursor
        && buf
            .cell((x0 + column, area.y))
            .is_some_and(|cell| cell.symbol() == " ")
    {
        buf.set_string(
            x0 + column,
            area.y,
            "▼",
            theme.style(StyleRole::BorderFocused),
        );
    }
}

/// One lane: the label, then the state bar with its marks.
fn draw_lane(model: &Model, agent: AgentId, depth: u16, window: &Window, y: u16, buf: &mut Buffer) {
    let theme = &model.theme;
    let geometry = model.timeline.geometry;
    let Some(record) = model.registry.agent(agent) else {
        return;
    };
    let selected = model.selected == Some(agent);
    let label_area = Rect::new(geometry.lanes.x, y, geometry.label, 1);
    if selected {
        buf.set_style(label_area, theme.style(StyleRole::Selection));
        buf.set_string(label_area.x, y, "▌", theme.style(StyleRole::BorderFocused));
    }
    let letter = model
        .letters
        .of(agent)
        .map_or_else(|| " ".to_owned(), |letter| letter.to_string());
    let mut name_style = theme.role(record.role);
    if selected {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    // Indented by depth in the hierarchy, when the labels have room for it.
    let indent = if geometry.label >= 20 {
        depth.min(3)
    } else {
        0
    };
    let label = [
        Span::raw(" ".repeat(usize::from(indent))),
        Span::styled(letter, theme.style(StyleRole::Letter)),
        Span::raw(" "),
        Span::styled(state_glyph(&record.state), theme.state(&record.state)),
        Span::raw(" "),
        Span::styled(record.name.clone(), name_style),
    ];
    let room = geometry.label.saturating_sub(1);
    put_spans(buf, label_area.x + 1, y, &truncated(&label, room), room);

    let Some(lane) = model.history.lane(agent) else {
        return;
    };
    let states = column_states(&lane.states, window);
    let marks = column_marks(&lane.marks, window);
    let x0 = geometry.cells_x();
    for column in 0..window.columns {
        let index = usize::from(column);
        let (symbol, role) = match (
            marks[index].and_then(|mark| glyph(mark.kind)),
            states[index],
        ) {
            (Some(mark), _) => mark,
            (None, Some(state)) => bar(state),
            (None, None) => continue,
        };
        buf.set_string(x0 + column, y, symbol, theme.style(role));
    }
}

/// The line under the lanes: the cursor's moment, then the selected agent's
/// state then and what it did.
fn draw_readout(model: &Model, window: &Window, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let state = &model.timeline;
    let dim = theme.style(StyleRole::TextDim);
    let bucket = state.cursor_bucket(model.now);
    let start = Timestamp::from_millis(bucket * window.step);
    let mut spans = vec![Span::styled(
        if state.cursor.is_none() {
            "now".to_owned()
        } else {
            ago(start.millis_until(model.now).try_into().unwrap_or(i64::MAX))
        },
        theme.style(StyleRole::BorderFocused),
    )];
    if let Some(agent) = model
        .selected
        .filter(|agent| state.lanes.contains(agent))
        .and_then(|agent| model.registry.agent(agent))
    {
        spans.push(Span::styled(" · ", dim));
        spans.push(Span::styled(
            agent.name.clone(),
            theme.role(agent.role).add_modifier(Modifier::BOLD),
        ));
        let lane = model.history.lane(agent.id);
        let then = if state.cursor.is_none() {
            Some(agent.state.category())
        } else {
            lane.and_then(|lane| lane.state_at(start.plus_millis(window.step - 1)))
        };
        match then {
            Some(category) => spans.push(Span::styled(
                format!(" {}", state_label(category)),
                theme.style(bar(category).1),
            )),
            None => spans.push(Span::styled(" not there yet", dim)),
        }
        let end = start.plus_millis(window.step);
        let mut marks: Vec<&Mark> = lane
            .map(|lane| {
                lane.marks
                    .iter()
                    .filter(|mark| mark.at >= start && mark.at < end && glyph(mark.kind).is_some())
                    .collect()
            })
            .unwrap_or_default();
        marks.sort_by_key(|mark| std::cmp::Reverse(mark.kind.priority()));
        for mark in marks.into_iter().take(2) {
            spans.push(Span::styled(" · ", dim));
            let (symbol, role) = glyph(mark.kind).unwrap_or(("·", StyleRole::Text));
            spans.push(Span::styled(format!("{symbol} "), theme.style(role)));
            spans.push(Span::styled(
                describe(&model.registry, mark),
                theme.style(StyleRole::Text),
            ));
        }
    }
    let room = area.width.saturating_sub(2);
    put_spans(buf, area.x + 1, area.y, &truncated(&spans, room), room);
}

/// Entering a state, as an event.
const fn became(state: StateCategory) -> &'static str {
    match state {
        StateCategory::NeedsInput => "began to wait for you",
        StateCategory::Failed => "failed",
        StateCategory::Working => "started working",
        StateCategory::Launching => "was launched",
        StateCategory::Done => "finished",
        StateCategory::Idle => "went idle",
        StateCategory::Stopped => "stopped",
        StateCategory::Lost => "was lost",
    }
}

/// What a mark means, in a few words.
fn describe(registry: &Registry, mark: &Mark) -> String {
    let name = |party: Party| match party {
        Party::Agent(agent) => registry
            .agent(agent)
            .map_or_else(|| agent.short(), |agent| agent.name.clone()),
        Party::Human => "you".to_owned(),
        Party::System => "volee".to_owned(),
    };
    match mark.kind {
        MarkKind::State(state) => became(state).to_owned(),
        MarkKind::Sent { message, kind } => {
            let Some(message) = registry.message(message) else {
                return format!("sent a {}", kind.label());
            };
            let to: Vec<String> = message.recipients().map(name).collect();
            let to = to.join(", ");
            let verb = match kind {
                MessageKind::Question => format!("asked {to}"),
                MessageKind::Answer => format!("answered {to}"),
                MessageKind::Report => format!("reported to {to}"),
                MessageKind::Contract => format!("contract to {to}"),
                MessageKind::Alert => format!("alerted {to}"),
                MessageKind::Note => format!("wrote to {to}"),
            };
            format!("{verb}: {}", first_line(&message.body))
        }
        MarkKind::Received { message, kind } => {
            let Some(message) = registry.message(message) else {
                return format!("received a {}", kind.label());
            };
            format!(
                "{} from {}: {}",
                kind.label(),
                name(message.from),
                first_line(&message.body)
            )
        }
        MarkKind::Delivery(change) => change.describe(),
        MarkKind::Verdict(verdict) => format!("verdict: {}", verdict.label()),
    }
}

fn first_line(body: &str) -> &str {
    body.lines().next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_millis(1_800_000_000_000 + seconds * 1_000)
    }

    fn state_for(width: u16, height: u16) -> TimelineState {
        TimelineState {
            geometry: Geometry::of(Rect::new(0, 0, width, height)),
            ..TimelineState::default()
        }
    }

    fn window(left_seconds: i64, step: i64, columns: u16) -> Window {
        let left = at(left_seconds).as_millis().div_euclid(step);
        Window {
            step,
            left,
            columns,
            now: left + i64::from(columns) - 1,
        }
    }

    #[test]
    fn the_automatic_scale_shows_the_last_half_hour() {
        let mut state = state_for(84, 24);
        assert_eq!(state.geometry.columns(), 62);
        assert_eq!(STEPS[state.step_index()], 30_000);
        let window = state.window(at(0));
        assert!(window.step * i64::from(window.columns) >= DEFAULT_SPAN_MILLIS);
        state.geometry = Geometry::of(Rect::new(0, 0, 158, 32));
        assert_eq!(STEPS[state.step_index()], 15_000);
        state.zoom(true);
        assert_eq!(state.scale, Scale::Fixed(3));
        state.zoom(false);
        state.zoom(false);
        assert_eq!(STEPS[state.step_index()], 30_000);
        state.reset();
        assert_eq!(state.scale, Scale::Auto);
    }

    #[test]
    fn columns_are_whole_buckets_that_slide_with_time() {
        let state = state_for(84, 24);
        let now = at(0);
        let window = state.window(now);
        assert_eq!(window.column_of(now), Some(window.columns - 1));
        // A moment keeps its column until the present enters a new bucket.
        let later = state.window(now.plus_millis(1));
        let event = now.plus_millis(-95_000);
        let shift = |w: &Window| w.column_of(event);
        if later.now == window.now {
            assert_eq!(shift(&later), shift(&window));
        }
        let next = state.window(now.plus_millis(30_000));
        assert_eq!(shift(&next).map(|c| c + 1), shift(&window));
    }

    #[test]
    fn the_cursor_scrubs_and_comes_back_to_the_present() {
        let mut state = state_for(84, 24);
        let now = at(0);
        state.scrub(now, false);
        let one_back = state.cursor.unwrap();
        assert_eq!(one_back.millis_until(now), 30_000);
        state.scrub(now, false);
        state.scrub(now, true);
        assert_eq!(state.cursor, Some(one_back));
        state.scrub(now, true);
        assert_eq!(state.cursor, None, "back to the present");
        // Far back, the window follows the cursor.
        state.jump_to(now.plus_millis(-3_600_000), now);
        let window = state.window(now);
        assert_eq!(window.column_of(state.cursor.unwrap()), Some(0));
    }

    #[test]
    fn a_column_shows_the_most_urgent_state_of_its_time() {
        let states = [
            (at(0), StateCategory::Working),
            (at(12), StateCategory::NeedsInput),
            (at(14), StateCategory::Working),
            (at(25), StateCategory::Idle),
        ];
        let window = window(-10, 10_000, 5);
        let columns = column_states(&states, &window);
        assert_eq!(
            columns,
            [
                None,
                Some(StateCategory::Working),
                Some(StateCategory::NeedsInput),
                Some(StateCategory::Working),
                Some(StateCategory::Idle),
            ]
        );
    }

    #[test]
    fn the_most_important_mark_wins_its_column() {
        let marks = [
            Mark {
                at: at(1),
                kind: MarkKind::State(StateCategory::Done),
            },
            Mark {
                at: at(2),
                kind: MarkKind::State(StateCategory::NeedsInput),
            },
            Mark {
                at: at(3),
                kind: MarkKind::State(StateCategory::Working),
            },
            Mark {
                at: at(15),
                kind: MarkKind::State(StateCategory::Done),
            },
        ];
        let window = window(0, 10_000, 3);
        let columns = column_marks(&marks, &window);
        assert_eq!(columns[0].map(|mark| mark.at), Some(at(2)));
        assert_eq!(columns[1].map(|mark| mark.at), Some(at(15)));
        assert_eq!(columns[2], None);
    }

    #[test]
    fn spans_read_naturally() {
        assert_eq!(span_label(45_000), "45 s");
        assert_eq!(span_label(300_000), "5 min");
        assert_eq!(span_label(5_400_000), "90 min");
        assert_eq!(span_label(7_200_000), "2 h");
        assert_eq!(ago(250_000), "4 min 10 s ago");
        assert_eq!(ago(3_900_000), "1 h 05 min ago");
    }
}
