//! The network view: who talks to whom.
//!
//! Every agent is a node, whatever its harness or its mission, and so are
//! you and volee itself (the mission supervisor) once they take part in a
//! conversation. A line joins two parties that exchanged messages, thicker
//! when they talked a lot lately; a faint dashed line joins an agent to the
//! agent that launched it. A question that waits for its answer is drawn in
//! the question color with a `?` next to the one who must answer, and every
//! new message sends a pulse along its line, from the sender to each
//! recipient.
//!
//! Positions come from a [force-directed layout](super::force) computed
//! only when the graph changes, then placed on the grid without overlaps at
//! one of three levels of detail: compact cards, one-line labels, or dots
//! that show the state and the agent's jump letter.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::iter;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use volee_core::harness::HarnessKind;
use volee_core::message::{MessageKind, Party};
use volee_core::time::Timestamp;
use volee_core::{AgentId, MessageId, Registry};

use super::force::{self, Point};
use super::hierarchy::{Direction, Zoom, ZoomChoice};
use super::spatial;
use crate::app::{Model, Motion};
use crate::text;
use crate::theme::StyleRole;
use crate::widgets::braille::{Dot, Pen, Raster};
use crate::widgets::canvas::Canvas;
use crate::widgets::card::{self, CardContext, NAME_MAX};
use crate::widgets::{centered_placeholder, put_spans, state_glyph, truncated};

/// A node of the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum NodeId {
    /// An agent.
    Agent(AgentId),
    /// The person using volee.
    Human,
    /// volee itself, the mission supervisor.
    System,
}

impl NodeId {
    /// The node of a party of a conversation.
    pub(crate) const fn of(party: Party) -> Self {
        match party {
            Party::Agent(agent) => Self::Agent(agent),
            Party::Human => Self::Human,
            Party::System => Self::System,
        }
    }

    /// The agent, for agent nodes.
    pub(crate) const fn agent(self) -> Option<AgentId> {
        match self {
            Self::Agent(agent) => Some(agent),
            _ => None,
        }
    }

    /// A number that names the node across layouts.
    fn key(self) -> u64 {
        match self {
            Self::Human => 1,
            Self::System => 2,
            Self::Agent(agent) => {
                let value = agent.ulid().0;
                let low = u64::try_from(value & u128::from(u64::MAX)).unwrap_or(0);
                let high = u64::try_from(value >> 64).unwrap_or(0);
                low ^ high.rotate_left(29)
            }
        }
    }
}

/// How many message times a link remembers, the most recent ones; older
/// messages weigh almost nothing anyway.
const RECENT: usize = 64;

/// Messages lose half their weight after this long, so a line shows recent
/// conversation more than old history.
const HALF_LIFE_MILLIS: f64 = 300_000.0;

/// The weight from which a line is drawn thick.
const THICK: f64 = 2.5;

/// A line between two nodes, by index in [`Graph::nodes`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Link {
    /// One end, the lower index.
    pub(crate) a: usize,
    /// The other end.
    pub(crate) b: usize,
    /// One end launched the other.
    pub(crate) lineage: bool,
    /// When they wrote to each other, in either direction, oldest first.
    pub(crate) times: Vec<Timestamp>,
    /// A question that waits for its answer: who asked, who must answer.
    pub(crate) waiting: Option<(usize, usize)>,
}

impl Link {
    /// Whether the two ends exchanged messages.
    pub(crate) fn talked(&self) -> bool {
        !self.times.is_empty()
    }

    /// How much they talked lately: each message counts 1 when new, half
    /// after [`HALF_LIFE_MILLIS`], and less and less afterwards.
    pub(crate) fn weight(&self, now: Timestamp) -> f64 {
        self.times
            .iter()
            .map(|at| {
                let age = f64::from(u32::try_from(at.millis_until(now)).unwrap_or(u32::MAX));
                HALF_LIFE_MILLIS / (HALF_LIFE_MILLIS + age)
            })
            .sum()
    }

    /// The other end.
    pub(crate) const fn other(&self, end: usize) -> usize {
        if end == self.a { self.b } else { self.a }
    }
}

/// Who talks to whom, from the registry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Graph {
    /// Agents, oldest first, then you and volee when they take part.
    pub(crate) nodes: Vec<NodeId>,
    /// Lines, by their two ends.
    pub(crate) links: Vec<Link>,
}

/// The shape of a graph, which decides the layout: nodes and which pairs
/// are linked, by conversation and by lineage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Topology {
    nodes: Vec<NodeId>,
    pairs: Vec<(usize, usize, bool, bool)>,
}

impl Graph {
    /// The graph of every agent and message of the registry.
    pub(crate) fn build(registry: &Registry) -> Self {
        let mut nodes: Vec<NodeId> = registry
            .agents()
            .map(|agent| NodeId::Agent(agent.id))
            .collect();
        let parties: BTreeSet<NodeId> = registry
            .messages()
            .flat_map(|message| iter::once(message.from).chain(message.recipients()))
            .filter(|party| !matches!(party, Party::Agent(_)))
            .map(NodeId::of)
            .collect();
        nodes.extend(parties);
        let index: HashMap<NodeId, usize> = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (*node, index))
            .collect();
        let mut links: BTreeMap<(usize, usize), Link> = BTreeMap::new();
        for agent in registry.agents() {
            if let Some(parent) = agent.parent
                && let (Some(&child), Some(&parent)) = (
                    index.get(&NodeId::Agent(agent.id)),
                    index.get(&NodeId::Agent(parent)),
                )
            {
                link_between(&mut links, child, parent).lineage = true;
            }
        }
        for message in registry.messages() {
            let Some(&from) = index.get(&NodeId::of(message.from)) else {
                continue;
            };
            for to in message.recipients() {
                if let Some(&to) = index.get(&NodeId::of(to))
                    && to != from
                {
                    link_between(&mut links, from, to).times.push(message.at);
                }
            }
        }
        for (asker, respondent) in waiting_questions(registry, &index) {
            link_between(&mut links, asker, respondent).waiting = Some((asker, respondent));
        }
        let links = links
            .into_values()
            .map(|mut link| {
                let extra = link.times.len().saturating_sub(RECENT);
                link.times.drain(..extra);
                link
            })
            .collect();
        Self { nodes, links }
    }

    fn topology(&self) -> Topology {
        Topology {
            nodes: self.nodes.clone(),
            pairs: self
                .links
                .iter()
                .map(|link| (link.a, link.b, link.lineage, link.talked()))
                .collect(),
        }
    }

    /// The index of a node.
    pub(crate) fn index_of(&self, node: NodeId) -> Option<usize> {
        self.nodes.iter().position(|candidate| *candidate == node)
    }

    /// How many lines touch each node.
    fn degrees(&self) -> Vec<usize> {
        let mut degrees = vec![0; self.nodes.len()];
        for link in &self.links {
            degrees[link.a] += 1;
            degrees[link.b] += 1;
        }
        degrees
    }
}

/// The link between two nodes, created on first use.
fn link_between(links: &mut BTreeMap<(usize, usize), Link>, one: usize, other: usize) -> &mut Link {
    let (a, b) = (one.min(other), one.max(other));
    links.entry((a, b)).or_insert_with(|| Link {
        a,
        b,
        ..Link::default()
    })
}

/// Questions without an answer yet, as (asker, asked) node indexes. A
/// question is answered by a message that replies to it, or by an answer
/// from the one asked to the asker written after it.
fn waiting_questions(registry: &Registry, index: &HashMap<NodeId, usize>) -> Vec<(usize, usize)> {
    let replied: HashSet<MessageId> = registry
        .messages()
        .filter_map(|message| message.in_reply_to)
        .collect();
    let mut waiting = Vec::new();
    for question in registry
        .messages()
        .filter(|message| message.kind == MessageKind::Question && !replied.contains(&message.id))
    {
        for asked in question.recipients() {
            let answered = registry.messages().any(|message| {
                message.kind == MessageKind::Answer
                    && message.from == asked
                    && message.is_for(question.from)
                    && message.at >= question.at
            });
            if answered {
                continue;
            }
            if let (Some(&from), Some(&to)) = (
                index.get(&NodeId::of(question.from)),
                index.get(&NodeId::of(asked)),
            ) {
                waiting.push((from, to));
            }
        }
    }
    waiting
}

/// Where a node is drawn, relative to the graph's area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Spot {
    /// The cells the node covers.
    pub(crate) rect: Rect,
    /// The cell where its lines attach.
    pub(crate) anchor: (u16, u16),
}

/// The network, placed in an area.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Scene {
    /// The detail level in use.
    pub(crate) zoom: Zoom,
    /// Whether it was chosen from the room.
    pub(crate) automatic: bool,
    /// One spot per node of the graph, in the same order.
    pub(crate) spots: Vec<Spot>,
}

/// What a scene is placed from; it is placed again only when this changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SceneKey {
    topology: Topology,
    widths: Vec<u16>,
    zoom: ZoomChoice,
    area: (u16, u16),
}

/// What a graph is built from: the agents and their parents, and how many
/// messages were posted.
type GraphKey = (Vec<(AgentId, Option<AgentId>)>, u64);

/// The network view's state: the graph, its layout and how it is placed.
#[derive(Debug, Default)]
pub(crate) struct NetworkState {
    /// The detail level the person asked for.
    pub(crate) zoom: ZoomChoice,
    /// Who talks to whom.
    pub(crate) graph: Graph,
    /// The graph, placed.
    pub(crate) scene: Scene,
    /// What the graph was built from.
    graph_key: Option<GraphKey>,
    /// What the layout was computed for.
    topology: Option<Topology>,
    /// The layout, stretched to `[0, 1]` on both axes, one point per node.
    points: Vec<Point>,
    /// The layout's own positions by node key, the start of the next one.
    memory: BTreeMap<u64, Point>,
    scene_key: Option<SceneKey>,
}

impl NetworkState {
    /// Brings the graph, its layout and its placement up to date for an
    /// area of `area` cells. Cheap when nothing changed, as on most frames.
    pub(crate) fn relayout(&mut self, registry: &Registry, messages: u64, area: (u16, u16)) {
        let key = (
            registry
                .agents()
                .map(|agent| (agent.id, agent.parent))
                .collect::<Vec<_>>(),
            messages,
        );
        if self.graph_key.as_ref() != Some(&key) {
            self.graph = Graph::build(registry);
            self.graph_key = Some(key);
        }
        let topology = self.graph.topology();
        if self.topology.as_ref() != Some(&topology) {
            let keys: Vec<u64> = self.graph.nodes.iter().map(|node| node.key()).collect();
            let springs: Vec<force::Link> = self
                .graph
                .links
                .iter()
                .map(|link| force::Link {
                    a: link.a,
                    b: link.b,
                    strength: if link.talked() { 1.0 } else { 0.5 },
                })
                .collect();
            let positions = force::relax(&keys, &springs, &self.memory);
            self.points = force::normalize(&positions);
            self.memory = keys.into_iter().zip(positions).collect();
            self.topology = Some(topology.clone());
        }
        let widths: Vec<u16> = self
            .graph
            .nodes
            .iter()
            .map(|node| label_width(*node, registry))
            .collect();
        let scene_key = SceneKey {
            topology,
            widths,
            zoom: self.zoom,
            area,
        };
        if self.scene_key.as_ref() != Some(&scene_key) {
            self.scene = place_scene(&self.graph, &self.points, &scene_key);
            self.scene_key = Some(scene_key);
        }
    }

    /// Every agent node and the cells it covers, relative to the graph's
    /// area: what the mouse and the arrows can reach.
    pub(crate) fn targets(&self) -> Vec<(AgentId, Rect)> {
        self.graph
            .nodes
            .iter()
            .zip(&self.scene.spots)
            .filter_map(|(node, spot)| node.agent().map(|agent| (agent, spot.rect)))
            .collect()
    }

    /// The agent to move to from `from`, among those `allowed`.
    pub(crate) fn neighbour(
        &self,
        from: AgentId,
        direction: Direction,
        allowed: impl Fn(AgentId) -> bool,
    ) -> Option<AgentId> {
        spatial::nearest(&self.targets(), from, direction, allowed)
    }

    /// The agent at the top left, or at the bottom right when `last`.
    pub(crate) fn corner(&self, last: bool, allowed: impl Fn(AgentId) -> bool) -> Option<AgentId> {
        let targets: Vec<(AgentId, Rect)> = self
            .targets()
            .into_iter()
            .filter(|(agent, _)| allowed(*agent))
            .collect();
        spatial::corner(&targets, last)
    }

    /// How many questions wait for their answer.
    pub(crate) fn waiting(&self) -> usize {
        self.graph
            .links
            .iter()
            .filter(|link| link.waiting.is_some())
            .count()
    }
}

/// The width of a node's one-line label: its state glyph and its name.
fn label_width(node: NodeId, registry: &Registry) -> u16 {
    match node {
        NodeId::Human | NodeId::System => party_label(node).1,
        NodeId::Agent(agent) => {
            let name = registry
                .agent(agent)
                .map_or("", |agent| agent.name.as_str());
            2 + u16::try_from(text::width(name).min(NAME_MAX)).unwrap_or(0)
        }
    }
}

/// The label of a node that is not an agent, and its width.
fn party_label(node: NodeId) -> (&'static str, u16) {
    match node {
        NodeId::System => ("◇ volee", 7),
        _ => ("◉ you", 5),
    }
}

/// The cells a node covers at a detail level, and where its lines attach:
/// the middle of the card or label, so lines stop at its edges.
fn footprint(node: NodeId, label: u16, zoom: Zoom) -> (u16, u16, (u16, u16)) {
    match (node, zoom) {
        (NodeId::Agent(_), Zoom::Cards) => (card::COMPACT_WIDTH, 3, (card::COMPACT_WIDTH / 2, 1)),
        (NodeId::Agent(_), Zoom::Dots) => (2, 1, (0, 0)),
        _ => (label, 1, (label / 2, 0)),
    }
}

/// Places the graph at the most detailed level that fits, or at the level
/// asked for.
fn place_scene(graph: &Graph, points: &[Point], key: &SceneKey) -> Scene {
    let candidates: &[Zoom] = match &key.zoom {
        ZoomChoice::Auto => &Zoom::ALL,
        ZoomChoice::Fixed(zoom) => std::slice::from_ref(zoom),
    };
    let degrees = graph.degrees();
    let mut order: Vec<usize> = (0..graph.nodes.len()).collect();
    order.sort_by_key(|index| (std::cmp::Reverse(degrees[*index]), *index));
    let mut chosen = Scene::default();
    for zoom in candidates {
        let sizes: Vec<(u16, u16, (u16, u16))> = graph
            .nodes
            .iter()
            .zip(&key.widths)
            .map(|(node, width)| footprint(*node, *width, *zoom))
            .collect();
        let gap = if *zoom == Zoom::Cards { 1 } else { 2 };
        let (spots, fits) = place(&sizes, points, &order, key.area, gap);
        chosen = Scene {
            zoom: *zoom,
            automatic: key.zoom == ZoomChoice::Auto,
            spots,
        };
        if fits {
            break;
        }
    }
    chosen
}

/// Puts each node at the spot its layout point asks for, or at the nearest
/// free one, in `order`. A point at 0 hugs the left or top border, a point
/// at 1 the right or bottom one. Returns the spots, and whether every node
/// found room near where it belongs.
fn place(
    sizes: &[(u16, u16, (u16, u16))],
    points: &[Point],
    order: &[usize],
    (width, height): (u16, u16),
    gap: u16,
) -> (Vec<Spot>, bool) {
    let mut placed: Vec<Option<Rect>> = vec![None; sizes.len()];
    let offsets = offsets(width, height);
    // How far, in columns (a row counts two), a node may move away from
    // where it belongs before the level is judged too crowded.
    let limit = i32::from((width / 8).max(5));
    let mut fits = true;
    for &index in order {
        let (w, h, _) = sizes[index];
        let (w, h) = (w.min(width), h.min(height));
        let point = points.get(index).copied().unwrap_or((0.5, 0.5));
        let ideal = (
            to_cell(point.0, width.saturating_sub(w)),
            to_cell(point.1, height.saturating_sub(h)),
        );
        let free = |x: i32, y: i32| {
            if x < 0
                || y < 0
                || x + i32::from(w) > i32::from(width)
                || y + i32::from(h) > i32::from(height)
            {
                return false;
            }
            placed.iter().flatten().all(|other| {
                let (ox, oy) = (i32::from(other.x), i32::from(other.y));
                let g = i32::from(gap);
                x >= ox + i32::from(other.width) + g
                    || ox >= x + i32::from(w) + g
                    || y >= oy + i32::from(other.height)
                    || oy >= y + i32::from(h)
            })
        };
        let found = offsets
            .iter()
            .map(|(dx, dy)| (ideal.0 + dx, ideal.1 + dy, dx.abs() + 2 * dy.abs()))
            .find(|(x, y, _)| free(*x, *y));
        let (x, y) = if let Some((x, y, moved)) = found {
            fits &= moved <= limit;
            (x, y)
        } else {
            fits = false;
            ideal
        };
        placed[index] = Some(Rect::new(
            u16::try_from(x).unwrap_or(0),
            u16::try_from(y).unwrap_or(0),
            w,
            h,
        ));
    }
    let spots = placed
        .into_iter()
        .zip(sizes)
        .map(|(rect, (_, _, (ax, ay)))| {
            let rect = rect.unwrap_or_default();
            Spot {
                rect,
                anchor: (
                    (rect.x + ax).min(width.saturating_sub(1)),
                    (rect.y + ay).min(height.saturating_sub(1)),
                ),
            }
        })
        .collect();
    (spots, fits)
}

/// Every move within the area, the shortest first (a row counts as two
/// columns, as cells are about twice as tall as wide).
fn offsets(width: u16, height: u16) -> Vec<(i32, i32)> {
    let (w, h) = (i32::from(width), i32::from(height));
    let mut offsets: Vec<(i32, i32)> = (-h..=h)
        .flat_map(|dy| (-w..=w).map(move |dx| (dx, dy)))
        .collect();
    offsets.sort_by_key(|(dx, dy)| (dx.abs() + 2 * dy.abs(), dy.abs(), *dy, *dx));
    offsets
}

/// The cell of a fraction of `span` cells.
#[allow(clippy::cast_possible_truncation)] // clamped to 0..=span, which fits in i32
fn to_cell(fraction: f64, span: u16) -> i32 {
    (fraction.clamp(0.0, 1.0) * f64::from(span))
        .round()
        .clamp(0.0, f64::from(span)) as i32
}

/// The graph's part of the view, and the summary line under it when there
/// is room for one.
pub(crate) fn split(area: Rect) -> (Rect, Option<Rect>) {
    if area.height < 8 {
        return (area, None);
    }
    let graph = Rect::new(area.x, area.y, area.width, area.height - 1);
    let footer = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    (graph, Some(footer))
}

/// The view's title after its name: the detail level and the questions
/// that wait.
pub(crate) fn title(model: &Model) -> Vec<Span<'static>> {
    let theme = &model.theme;
    let state = &model.network;
    let zoom = if state.scene.automatic {
        state.scene.zoom.label().to_owned()
    } else {
        format!("{} (fixed)", state.scene.zoom.label())
    };
    let mut spans = vec![Span::styled(
        format!(" · {zoom}"),
        theme.style(StyleRole::TextDim),
    )];
    let waiting = state.waiting();
    if waiting > 0 {
        let noun = if waiting == 1 {
            "1 question waits".to_owned()
        } else {
            format!("{waiting} questions wait")
        };
        spans.push(Span::styled(" · ", theme.style(StyleRole::TextDim)));
        spans.push(Span::styled(
            format!("? {noun}"),
            theme.style(StyleRole::EdgeQuestion),
        ));
    }
    spans
}

/// How a node stands out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Emphasis {
    /// The selected agent.
    Selected,
    /// Linked to the selected agent.
    Neighbour,
    /// Anyone else.
    Plain,
    /// Filtered out.
    Dimmed,
}

/// Draws the network in `area`, the inside of the view panel.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    if area.intersection(buf.area) != area || area.is_empty() {
        return;
    }
    let state = &model.network;
    let graph = &state.graph;
    if graph.nodes.is_empty() || state.scene.spots.len() != graph.nodes.len() {
        let message = model.settings.without_claude_code().unwrap_or(
            "No agents yet. Start Claude Code anywhere, or launch one with volee spawn.",
        );
        centered_placeholder(model, message, area, buf);
        return;
    }
    let (graph_area, footer) = split(area);
    let spots = &state.scene.spots;
    let selected = model
        .selected
        .and_then(|agent| graph.index_of(NodeId::Agent(agent)));
    let emphasis: Vec<Emphasis> = (0..graph.nodes.len())
        .map(|index| emphasis_of(model, graph, selected, index))
        .collect();

    // Lines first, with pulses and question marks over them, then nodes.
    draw_lines(model, selected, &emphasis, graph_area, buf);
    let context = CardContext::new(model);
    let mut canvas = Canvas::new(buf, graph_area, (0, 0));
    for (index, node) in graph.nodes.iter().enumerate() {
        draw_node(
            &mut canvas,
            model,
            &context,
            *node,
            spots[index],
            emphasis[index],
        );
    }
    if state.scene.zoom == Zoom::Dots
        && let Some(index) = selected
    {
        name_tag(&mut canvas, model, graph, spots, index, graph_area);
    }
    if let Some(footer) = footer {
        summary(model, graph, selected, footer, buf);
    }
}

/// The lines between nodes, the pulses travelling along them and the `?`
/// of waiting questions, in the graph's `area`.
fn draw_lines(
    model: &Model,
    selected: Option<usize>,
    emphasis: &[Emphasis],
    area: Rect,
    buf: &mut Buffer,
) {
    let theme = &model.theme;
    let graph = &model.network.graph;
    let spots = &model.network.scene.spots;
    let mut raster = Raster::new(area);
    let center = |index: usize| Raster::center(spots[index].anchor.0, spots[index].anchor.1);
    for link in &graph.links {
        let pen = pen_for(model, link, selected, emphasis);
        raster.line(center(link.a), center(link.b), &pen);
    }
    let pulse_pen = Pen {
        style: theme.style(StyleRole::EdgePulse),
        priority: 9,
        width: 2,
        dashed: false,
    };
    let mut heads: Vec<Dot> = Vec::new();
    for (pulse, progress) in model.pulses.active(model.now) {
        let (Some(from), Some(to)) = (
            graph.index_of(NodeId::of(pulse.from)),
            graph.index_of(NodeId::of(pulse.to)),
        ) else {
            continue;
        };
        let (start, end) = (center(from), center(to));
        if model.motion == Motion::Reduced {
            raster.line(start, end, &pulse_pen);
        } else {
            // The head and a short trail behind it.
            let head = Raster::along(start, end, progress);
            raster.line(Raster::along(start, end, progress - 0.3), head, &pulse_pen);
            heads.push(head);
        }
    }
    raster.render(buf);
    let put = |buf: &mut Buffer, (column, row): (u16, u16), symbol: &str, role: StyleRole| {
        buf.set_string(area.x + column, area.y + row, symbol, theme.style(role));
    };
    for head in heads {
        if let Some(cell) = raster.cell_of(head)
            && !spots.iter().any(|spot| spot.rect.contains(cell.into()))
        {
            put(buf, cell, "●", StyleRole::EdgePulse);
        }
    }
    for link in &graph.links {
        if let Some((asker, respondent)) = link.waiting
            && let Some(cell) = question_mark(
                &raster,
                spots,
                center(asker),
                center(respondent),
                respondent,
            )
        {
            put(buf, cell, "?", StyleRole::EdgeQuestion);
        }
    }
}

/// How the node at `index` stands out.
fn emphasis_of(model: &Model, graph: &Graph, selected: Option<usize>, index: usize) -> Emphasis {
    if let NodeId::Agent(agent) = graph.nodes[index]
        && model
            .registry
            .agent(agent)
            .is_some_and(|agent| !model.matches(agent))
    {
        return Emphasis::Dimmed;
    }
    match selected {
        Some(selected) if selected == index => Emphasis::Selected,
        Some(selected)
            if graph.links.iter().any(|link| {
                (link.a == selected || link.b == selected) && link.other(selected) == index
            }) =>
        {
            Emphasis::Neighbour
        }
        _ => Emphasis::Plain,
    }
}

/// How a line is drawn: conversations two dots wide, and a block wide when
/// busy; lineage one dot, dashed and faint; the selected agent's lines and
/// waiting questions highlighted; lines to filtered-out agents faint.
fn pen_for(model: &Model, link: &Link, selected: Option<usize>, emphasis: &[Emphasis]) -> Pen {
    let theme = &model.theme;
    let hot = selected.is_some_and(|selected| link.a == selected || link.b == selected);
    let dimmed = emphasis[link.a] == Emphasis::Dimmed || emphasis[link.b] == Emphasis::Dimmed;
    let hot_style = Style {
        fg: theme.style(StyleRole::CardSelected).fg,
        ..Style::default()
    };
    if !link.talked() {
        return Pen {
            style: if hot && !dimmed {
                hot_style
            } else {
                theme.style(StyleRole::Edge)
            },
            priority: if hot { 4 } else { 1 },
            width: 1,
            dashed: true,
        };
    }
    let width = if dimmed {
        1
    } else if link.weight(model.now) >= THICK {
        3
    } else {
        2
    };
    let (style, priority) = if dimmed {
        (theme.style(StyleRole::Edge), 1)
    } else if link.waiting.is_some() {
        (theme.style(StyleRole::EdgeQuestion), 6)
    } else if hot {
        (hot_style, 5)
    } else {
        (theme.style(StyleRole::EdgeMessage), 2)
    };
    Pen {
        style,
        priority,
        width,
        dashed: false,
    }
}

/// Where the `?` of a waiting question goes: on its line, just outside the
/// one who must answer, on a cell no node covers.
fn question_mark(
    raster: &Raster,
    spots: &[Spot],
    from: Dot,
    to: Dot,
    respondent: usize,
) -> Option<(u16, u16)> {
    let target = spots[respondent].rect;
    (0..=40).rev().find_map(|step| {
        let point = Raster::along(from, to, f64::from(step) / 40.0);
        let (column, row) = raster.cell_of(point)?;
        let free = !target.contains((column, row).into())
            && !spots
                .iter()
                .any(|spot| spot.rect.contains((column, row).into()));
        free.then_some((column, row))
    })
}

/// Draws one node at its spot.
fn draw_node(
    canvas: &mut Canvas<'_>,
    model: &Model,
    context: &CardContext<'_>,
    node: NodeId,
    spot: Spot,
    emphasis: Emphasis,
) {
    let theme = &model.theme;
    let (x, y) = (i32::from(spot.rect.x), i32::from(spot.rect.y));
    // Lines are drawn first: a node hides what passes under it.
    let blank = theme.style(StyleRole::Background);
    for row in 0..i32::from(spot.rect.height) {
        for column in 0..i32::from(spot.rect.width) {
            canvas.put(x + column, y + row, " ", blank);
        }
    }
    let NodeId::Agent(id) = node else {
        let (label, width) = party_label(node);
        let style = theme.style(if node == NodeId::Human {
            StyleRole::Title
        } else {
            StyleRole::TextDim
        });
        canvas.text(x, y, label, style, width);
        return;
    };
    let Some(agent) = model.registry.agent(id) else {
        return;
    };
    let zoom = model.network.scene.zoom;
    if zoom == Zoom::Cards {
        card::draw_compact(canvas, x, y, agent, context);
        return;
    }
    let glyph = Span::styled(state_glyph(&agent.state), theme.state(&agent.state));
    let mut name_style = theme.role(agent.role);
    if emphasis == Emphasis::Neighbour {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    let mut spans = if zoom == Zoom::Dots {
        let letter = model
            .letters
            .of(id)
            .or_else(|| agent.name.chars().next())
            .unwrap_or('?');
        vec![
            glyph,
            Span::styled(letter.to_string(), name_style.add_modifier(Modifier::BOLD)),
        ]
    } else {
        vec![
            glyph,
            Span::raw(" "),
            Span::styled(
                text::truncate(&agent.name, NAME_MAX).into_owned(),
                name_style,
            ),
        ]
    };
    match emphasis {
        Emphasis::Selected => {
            let selection = theme.style(StyleRole::CardSelected);
            for span in &mut spans {
                span.style = span.style.patch(selection);
            }
        }
        Emphasis::Dimmed => {
            let dim = theme.style(StyleRole::TextDim);
            for span in &mut spans {
                span.style = dim;
            }
        }
        Emphasis::Neighbour | Emphasis::Plain => {}
    }
    canvas.spans(x, y, &spans, spot.rect.width);
}

/// In dots, the selected agent's name next to its dot, so the picture
/// tells who it is.
fn name_tag(
    canvas: &mut Canvas<'_>,
    model: &Model,
    graph: &Graph,
    spots: &[Spot],
    index: usize,
    area: Rect,
) {
    let NodeId::Agent(agent) = graph.nodes[index] else {
        return;
    };
    let Some(agent) = model.registry.agent(agent) else {
        return;
    };
    let tag = format!(" {} ", text::truncate(&agent.name, NAME_MAX));
    let width = u16::try_from(text::width(&tag)).unwrap_or(0);
    let rect = spots[index].rect;
    let x = if rect.right() + width <= area.width {
        i32::from(rect.right())
    } else {
        i32::from(rect.x) - i32::from(width)
    };
    canvas.text(
        x.max(0),
        i32::from(rect.y),
        &tag,
        model.theme.style(StyleRole::CardSelected),
        width,
    );
}

/// The line under the graph: the selected agent, who it talks with, the
/// questions it waits for or owes, and a legend when there is room.
fn summary(model: &Model, graph: &Graph, selected: Option<usize>, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let spans = selected
        .map(|index| about(model, graph, index))
        .unwrap_or_default();
    let legend = [
        Span::styled("⠶⠶", theme.style(StyleRole::EdgeMessage)),
        Span::styled(" messages  ", dim),
        Span::styled("⠂ ⠂", theme.style(StyleRole::Edge)),
        Span::styled(" launched  ", dim),
        Span::styled("?", theme.style(StyleRole::EdgeQuestion)),
        Span::styled(" waits  ", dim),
        Span::styled("●", theme.style(StyleRole::EdgePulse)),
        Span::styled(" new", dim),
    ];
    let width_of = |spans: &[Span<'_>]| -> u16 {
        u16::try_from(
            spans
                .iter()
                .map(|span| text::width(&span.content))
                .sum::<usize>(),
        )
        .unwrap_or(u16::MAX)
    };
    let legend_width = width_of(&legend);
    let summary_width = width_of(&spans);
    let room = area.width.saturating_sub(1);
    put_spans(buf, area.x + 1, area.y, &truncated(&spans, room), room);
    if summary_width + legend_width + 4 <= room {
        put_spans(
            buf,
            area.right() - legend_width - 1,
            area.y,
            &legend,
            legend_width,
        );
    }
}

/// The selected node in a line: questions first, as they wait for
/// someone, then who it talks with, who launched it and whom it launched.
fn about(model: &Model, graph: &Graph, index: usize) -> Vec<Span<'static>> {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let plain = theme.style(StyleRole::Text);
    let mut spans: Vec<Span<'static>> = Vec::new();
    if let NodeId::Agent(id) = graph.nodes[index]
        && let Some(agent) = model.registry.agent(id)
    {
        spans.push(Span::styled(
            state_glyph(&agent.state),
            theme.state(&agent.state),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            agent.name.clone(),
            theme.role(agent.role).add_modifier(Modifier::BOLD),
        ));
        let about = if agent.harness == HarnessKind::ClaudeCode {
            format!(" · {}", agent.role)
        } else {
            format!(" · {} · {}", agent.role, agent.harness)
        };
        spans.push(Span::styled(about, dim));
        // Questions first: they wait for someone.
        for link in &graph.links {
            match link.waiting {
                Some((asker, respondent)) if asker == index => spans.push(Span::styled(
                    format!(
                        " · ? waits for {}",
                        node_name(model, graph.nodes[respondent])
                    ),
                    theme.style(StyleRole::EdgeQuestion),
                )),
                Some((asker, respondent)) if respondent == index => spans.push(Span::styled(
                    format!(
                        " · ? {} waits for an answer",
                        node_name(model, graph.nodes[asker])
                    ),
                    theme.style(StyleRole::EdgeQuestion),
                )),
                _ => {}
            }
        }
        let mut partners: Vec<(f64, String)> = graph
            .links
            .iter()
            .filter(|link| link.talked() && (link.a == index || link.b == index))
            .map(|link| {
                (
                    link.weight(model.now),
                    node_name(model, graph.nodes[link.other(index)]),
                )
            })
            .collect();
        partners.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        if partners.is_empty() {
            spans.push(Span::styled(" · no messages yet", dim));
        } else {
            spans.push(Span::styled(" · talks with ", dim));
            let names: Vec<String> = partners.into_iter().map(|(_, name)| name).collect();
            spans.push(Span::styled(names.join(", "), plain));
        }
        if let Some(parent) = agent.parent.and_then(|parent| model.registry.agent(parent)) {
            spans.push(Span::styled(" · launched by ", dim));
            spans.push(Span::styled(parent.name.clone(), plain));
        }
        let children: Vec<&str> = model
            .registry
            .children(agent.id)
            .into_iter()
            .map(|child| child.name.as_str())
            .collect();
        if !children.is_empty() {
            spans.push(Span::styled(" · launched ", dim));
            spans.push(Span::styled(children.join(", "), plain));
        }
    }
    spans
}

/// A node's name for people: the agent's name, `you` or `volee`.
fn node_name(model: &Model, node: NodeId) -> String {
    match node {
        NodeId::Agent(agent) => model.name_of(agent),
        NodeId::Human => "you".to_owned(),
        NodeId::System => "volee".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::clock::ManualClock;
    use crate::demo::{
        DemoSource, INFRA, INVOICE_PAGE, INVOICE_TOOLS, INVOICING, TOWER, TRACE_GATEWAY, agent_id,
    };

    fn start() -> Timestamp {
        Timestamp::from_millis(1_793_986_617_000)
    }

    fn registry() -> Registry {
        let demo = DemoSource::new(Arc::new(ManualClock::new(start())));
        Registry::from_events(&demo.events_until(start())).unwrap()
    }

    fn node(member: usize) -> NodeId {
        NodeId::Agent(agent_id(start(), member))
    }

    fn link(graph: &Graph, a: NodeId, b: NodeId) -> Option<&Link> {
        let (a, b) = (graph.index_of(a)?, graph.index_of(b)?);
        graph
            .links
            .iter()
            .find(|link| (link.a, link.b) == (a.min(b), a.max(b)))
    }

    #[test]
    fn the_graph_has_every_agent_and_the_parties_that_talk() {
        let registry = registry();
        let graph = Graph::build(&registry);
        assert_eq!(graph.nodes.len(), registry.agents().count() + 2);
        assert!(graph.nodes.contains(&NodeId::Human));
        assert!(graph.nodes.contains(&NodeId::System));
        let lineage = link(&graph, node(TOWER), node(INVOICING)).unwrap();
        assert!(lineage.lineage);
        let contract = link(&graph, node(INVOICING), node(INVOICE_PAGE)).unwrap();
        assert!(contract.lineage && contract.talked());
        // A session of another harness that volee did not launch talks to
        // you, along the routes volee allows, and to no mission's worker.
        let codex = link(&graph, node(INFRA), NodeId::Human).unwrap();
        assert!(codex.talked() && !codex.lineage);
        assert_eq!(link(&graph, node(INFRA), node(TRACE_GATEWAY)), None);
    }

    #[test]
    fn unanswered_questions_wait_on_their_line() {
        let registry = registry();
        let graph = Graph::build(&registry);
        let to_you = link(&graph, node(INVOICE_TOOLS), NodeId::Human).unwrap();
        let (asker, respondent) = to_you.waiting.unwrap();
        assert_eq!(graph.nodes[asker], node(INVOICE_TOOLS));
        assert_eq!(graph.nodes[respondent], NodeId::Human);
        // The latency audit asks you about the other mission.
        let relay = link(&graph, node(TRACE_GATEWAY), NodeId::Human).unwrap();
        assert_eq!(
            relay.waiting.map(|(asker, _)| graph.nodes[asker]),
            Some(node(TRACE_GATEWAY))
        );
        let answered = link(
            &graph,
            node(INVOICE_PAGE),
            node(crate::demo::INVOICE_SCHEMA),
        )
        .unwrap();
        assert_eq!(answered.waiting, None);
    }

    #[test]
    fn weights_fade_with_time() {
        let link = Link {
            times: vec![start(), start()],
            ..Link::default()
        };
        assert!((link.weight(start()) - 2.0).abs() < 1e-9);
        let later = start().plus_millis(300_000);
        assert!((link.weight(later) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn placement_keeps_nodes_apart_and_inside() {
        let sizes = vec![(10, 1, (0, 0)); 6];
        let points = vec![(0.5, 0.5); 6];
        let order: Vec<usize> = (0..6).collect();
        let (spots, _) = place(&sizes, &points, &order, (40, 10), 2);
        for (i, a) in spots.iter().enumerate() {
            assert!(a.rect.right() <= 40 && a.rect.bottom() <= 10, "{a:?}");
            for b in &spots[i + 1..] {
                let apart = a.rect.right() + 2 <= b.rect.x
                    || b.rect.right() + 2 <= a.rect.x
                    || a.rect.y != b.rect.y;
                assert!(apart, "{a:?} and {b:?}");
            }
        }
        // The same input places the same way.
        assert_eq!(place(&sizes, &points, &order, (40, 10), 2).0, spots);
    }

    #[test]
    fn the_automatic_level_follows_the_room() {
        let registry = registry();
        let mut state = NetworkState::default();
        state.relayout(&registry, 1, (158, 32));
        assert_eq!(state.scene.zoom, Zoom::Cards);
        state.relayout(&registry, 1, (84, 23));
        assert_ne!(state.scene.zoom, Zoom::Cards);
        state.relayout(&registry, 1, (30, 10));
        assert_eq!(state.scene.zoom, Zoom::Dots);
        state.zoom = ZoomChoice::Fixed(Zoom::Compact);
        state.relayout(&registry, 1, (30, 10));
        assert_eq!(state.scene.zoom, Zoom::Compact);
        assert!(!state.scene.automatic);
    }

    #[test]
    fn a_new_message_between_known_agents_keeps_the_layout() {
        let registry = registry();
        let mut state = NetworkState::default();
        state.relayout(&registry, 1, (84, 23));
        let before = state.scene.clone();
        // Same agents, same pairs: only the message count moved.
        state.relayout(&registry, 2, (84, 23));
        assert_eq!(state.scene, before);
    }
}
