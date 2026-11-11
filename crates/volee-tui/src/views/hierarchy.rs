//! The tree view: who launched whom, as a tree of cards.
//!
//! It shows the tree of the selected agent: its root (a lead, a mission
//! started by the person) and everything under it, children side by side
//! under their parent. Agents that nobody launched and that launched nobody,
//! such as sessions started on their own, show side by side instead, in rows
//! ([`Show`]). Subagents hang under their parent's card as satellites
//! instead of taking a level of the tree. The tree is placed by
//! [`tree_layout`](super::tree_layout) at one of three levels of detail
//! (cards, compact cards, dots), chosen from the available room unless the
//! person picks one.

use std::collections::BTreeSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use volee_core::role::Role;
use volee_core::{Agent, AgentId, Registry};

use super::tree_layout::{self, Forest, Node, Placement, Spacing};
use crate::app::Model;
use crate::theme::StyleRole;
use crate::widgets::card::{self, CardContext};
use crate::widgets::{self, canvas::Canvas};

/// How much each card shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Zoom {
    /// Full cards: state, place, activity, delivery, sparkline.
    #[default]
    Cards,
    /// One line per card: the state.
    Compact,
    /// A glyph and a name.
    Dots,
}

impl Zoom {
    /// From the most to the least detailed.
    pub(crate) const ALL: [Self; 3] = [Self::Cards, Self::Compact, Self::Dots];

    /// The name shown in the view's title.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Cards => "cards",
            Self::Compact => "compact",
            Self::Dots => "dots",
        }
    }

    /// One level more detailed, if any.
    pub(crate) const fn more_detail(self) -> Self {
        match self {
            Self::Cards | Self::Compact => Self::Cards,
            Self::Dots => Self::Compact,
        }
    }

    /// One level less detailed, if any.
    pub(crate) const fn less_detail(self) -> Self {
        match self {
            Self::Cards => Self::Compact,
            Self::Compact | Self::Dots => Self::Dots,
        }
    }

    /// Room between cards at this level.
    pub(crate) const fn spacing(self) -> Spacing {
        match self {
            Self::Cards => Spacing {
                gap: 2,
                level: 2,
                indent: 3,
                forest_gap: 4,
            },
            Self::Compact => Spacing {
                gap: 2,
                level: 2,
                indent: 3,
                forest_gap: 3,
            },
            Self::Dots => Spacing {
                gap: 3,
                level: 2,
                indent: 3,
                forest_gap: 4,
            },
        }
    }
}

/// The detail level the person asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum ZoomChoice {
    /// The most detailed level that fits.
    #[default]
    Auto,
    /// This level, scrolling if needed.
    Fixed(Zoom),
}

/// Where an agent is drawn: its own card, or a satellite line of a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    Card(usize),
    Satellite(usize, usize),
}

/// A direction on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    /// Toward the top.
    Up,
    /// Toward the bottom.
    Down,
    /// Toward the left.
    Left,
    /// Toward the right.
    Right,
}

/// A card of the scene and what hangs from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SceneNode {
    /// The agent.
    pub(crate) agent: AgentId,
    /// The card and its satellites.
    pub(crate) place: Placement,
    /// The height of the card alone.
    pub(crate) card_height: u16,
    /// Subagents, shown under the card.
    pub(crate) satellites: Vec<AgentId>,
    /// Children in the tree, as indexes of the scene's nodes.
    pub(crate) children: Vec<usize>,
    /// Whether the children are stacked in a column.
    pub(crate) stacked: bool,
}

/// The placed hierarchy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Scene {
    /// The detail level in use.
    pub(crate) zoom: Zoom,
    /// Whether it was chosen from the room.
    pub(crate) automatic: bool,
    /// The cards.
    pub(crate) nodes: Vec<SceneNode>,
    /// Width in cells.
    pub(crate) width: u16,
    /// Height in cells.
    pub(crate) height: u16,
}

/// What the view shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Show {
    /// Every tree.
    #[default]
    Everything,
    /// One tree: an agent and everything under it.
    Tree(AgentId),
    /// The agents that nobody launched and that launched nobody, such as
    /// sessions started on their own, side by side in rows.
    Loners,
}

impl Show {
    /// What to show: the subtree the person entered, else the tree of the
    /// selected agent, else every tree.
    pub(crate) fn of(
        registry: &Registry,
        subtree: Option<AgentId>,
        selected: Option<AgentId>,
    ) -> Self {
        if let Some(root) = subtree.filter(|id| registry.agent(*id).is_some()) {
            return Self::Tree(root);
        }
        let Some(root) = selected.and_then(|id| root_of(registry, id)) else {
            return Self::Everything;
        };
        if launched_others(registry, root) {
            Self::Tree(root)
        } else {
            Self::Loners
        }
    }
}

/// The agent at the top of `agent`'s tree.
fn root_of(registry: &Registry, agent: AgentId) -> Option<AgentId> {
    let mut current = registry.agent(agent)?;
    let mut seen = BTreeSet::from([current.id]);
    while let Some(parent) = current.parent.and_then(|id| registry.agent(id)) {
        if !seen.insert(parent.id) {
            break;
        }
        current = parent;
    }
    Some(current.id)
}

/// Whether `agent` launched agents that take a level of the tree (its
/// subagents hang under its card and do not).
fn launched_others(registry: &Registry, agent: AgentId) -> bool {
    registry
        .children(agent)
        .iter()
        .any(|child| child.role != Role::Subagent)
}

/// What a scene is built from; the scene is rebuilt only when it changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SceneKey {
    agents: Vec<(AgentId, Option<AgentId>, Role, String)>,
    show: Show,
    zoom: ZoomChoice,
    area: (u16, u16),
}

/// The inputs of a scene.
pub(crate) fn scene_key(
    registry: &Registry,
    show: Show,
    zoom: ZoomChoice,
    area: (u16, u16),
) -> SceneKey {
    SceneKey {
        agents: registry
            .agents()
            .map(|agent| (agent.id, agent.parent, agent.role, agent.name.clone()))
            .collect(),
        show,
        zoom,
        area,
    }
}

/// The order of roots: leads first, independent sessions last.
const fn role_rank(role: Role) -> u8 {
    match role {
        Role::Lead => 0,
        Role::Coordinator => 1,
        Role::Worker => 2,
        Role::Subagent => 3,
        Role::Independent => 4,
    }
}

/// The roots to show, in display order.
fn roots(registry: &Registry, show: Show) -> Vec<&Agent> {
    let mut roots = match show {
        Show::Tree(root) => return registry.agent(root).into_iter().collect(),
        Show::Everything => registry.roots(),
        Show::Loners => registry
            .roots()
            .into_iter()
            .filter(|agent| !launched_others(registry, agent.id))
            .collect(),
    };
    roots.sort_by_key(|agent| role_rank(agent.role));
    roots
}

/// Agents as a tree with satellites, before placement.
#[derive(Debug, Default)]
struct AgentTree {
    agents: Vec<AgentId>,
    names: Vec<String>,
    roles: Vec<Role>,
    satellites: Vec<Vec<AgentId>>,
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
}

impl AgentTree {
    fn new(registry: &Registry, show: Show) -> Self {
        let mut tree = Self::default();
        let mut seen = BTreeSet::new();
        for root in roots(registry, show) {
            if let Some(index) = tree.add(registry, root, &mut seen) {
                tree.roots.push(index);
            }
        }
        tree
    }

    fn add(
        &mut self,
        registry: &Registry,
        agent: &Agent,
        seen: &mut BTreeSet<AgentId>,
    ) -> Option<usize> {
        if !seen.insert(agent.id) {
            return None;
        }
        let index = self.agents.len();
        self.agents.push(agent.id);
        self.names.push(agent.name.clone());
        self.roles.push(agent.role);
        self.satellites.push(Vec::new());
        self.children.push(Vec::new());
        let mut satellites = Vec::new();
        for child in registry.children(agent.id) {
            if child.role == Role::Subagent {
                collect_satellites(registry, child, &mut satellites, seen);
            } else if let Some(child) = self.add(registry, child, seen) {
                self.children[index].push(child);
            }
        }
        self.satellites[index] = satellites;
        Some(index)
    }

    fn forest(&self, registry: &Registry, zoom: Zoom) -> Forest {
        let nodes = (0..self.agents.len())
            .map(|index| {
                let (card_width, card_height) =
                    card::size(zoom, self.roles[index], &self.names[index]);
                let satellites = &self.satellites[index];
                let satellite_width = satellites
                    .iter()
                    .filter_map(|id| registry.agent(*id))
                    .map(|agent| card::dot_width(&agent.name, 5))
                    .max()
                    .unwrap_or(0);
                // Dots are as wide as their labels and attach at their glyph;
                // cards attach at their middle.
                let (width, anchor) = if zoom == Zoom::Dots {
                    (card_width.max(satellite_width), 0)
                } else {
                    (card_width, card_width / 2)
                };
                Node {
                    width,
                    height: card_height + u16::try_from(satellites.len()).unwrap_or(u16::MAX),
                    anchor,
                    children: self.children[index].clone(),
                }
            })
            .collect();
        Forest {
            nodes,
            roots: self.roots.clone(),
        }
    }
}

/// A subagent and its own subagents, flattened.
fn collect_satellites(
    registry: &Registry,
    agent: &Agent,
    out: &mut Vec<AgentId>,
    seen: &mut BTreeSet<AgentId>,
) {
    if !seen.insert(agent.id) {
        return;
    }
    out.push(agent.id);
    for child in registry.children(agent.id) {
        collect_satellites(registry, child, out, seen);
    }
}

impl Scene {
    /// Places what `show` asks for in an area of `area` cells.
    pub(crate) fn build(
        registry: &Registry,
        show: Show,
        choice: ZoomChoice,
        area: (u16, u16),
    ) -> Self {
        let tree = AgentTree::new(registry, show);
        let candidates: &[Zoom] = match &choice {
            ZoomChoice::Auto => &Zoom::ALL,
            ZoomChoice::Fixed(zoom) => std::slice::from_ref(zoom),
        };
        let mut chosen = None;
        for zoom in candidates {
            let forest = tree.forest(registry, *zoom);
            let layout = if show == Show::Loners {
                tree_layout::grid(&forest, zoom.spacing(), area.0)
            } else {
                tree_layout::fit(&forest, zoom.spacing(), area.0)
            };
            let fits = layout.width <= area.0 && layout.height <= area.1;
            chosen = Some((*zoom, layout));
            if fits {
                break;
            }
        }
        let Some((zoom, layout)) = chosen else {
            return Self::default();
        };
        let nodes = (0..tree.agents.len())
            .map(|index| SceneNode {
                agent: tree.agents[index],
                place: layout.nodes[index],
                card_height: card::size(zoom, tree.roles[index], &tree.names[index]).1,
                satellites: tree.satellites[index].clone(),
                children: tree.children[index].clone(),
                stacked: layout.stacking.parents.contains(&index),
            })
            .collect();
        Self {
            zoom,
            automatic: choice == ZoomChoice::Auto,
            nodes,
            width: layout.width,
            height: layout.height,
        }
    }

    /// Every selectable rectangle: cards and satellite lines.
    pub(crate) fn targets(&self) -> Vec<(AgentId, Rect)> {
        let mut targets = Vec::new();
        for node in &self.nodes {
            let place = node.place;
            targets.push((
                node.agent,
                Rect::new(place.x, place.y, place.width, node.card_height),
            ));
            for (row, satellite) in (0..).zip(&node.satellites) {
                targets.push((
                    *satellite,
                    Rect::new(
                        place.x + 2,
                        place.y + node.card_height + row,
                        place.width.saturating_sub(2),
                        1,
                    ),
                ));
            }
        }
        targets
    }

    /// The rectangle of an agent's card or satellite line.
    pub(crate) fn rect_of(&self, agent: AgentId) -> Option<Rect> {
        self.targets()
            .into_iter()
            .find(|(id, _)| *id == agent)
            .map(|(_, rect)| rect)
    }

    /// The top-left card.
    pub(crate) fn first(&self) -> Option<AgentId> {
        self.targets()
            .into_iter()
            .min_by_key(|(_, rect)| (rect.y, rect.x))
            .map(|(id, _)| id)
    }

    /// The bottom-right card.
    pub(crate) fn last(&self) -> Option<AgentId> {
        self.targets()
            .into_iter()
            .max_by_key(|(_, rect)| (rect.y, rect.x))
            .map(|(id, _)| id)
    }

    /// The card to move to from `from`, among those `allowed`.
    ///
    /// Up and down follow the tree as drawn: down goes to the first
    /// satellite, then to the child under the card, or to the next card of a
    /// stack; up goes back the same way, to the parent. Left and right, and
    /// moves the tree cannot answer, pick the nearest card in that direction.
    pub(crate) fn neighbour(
        &self,
        from: AgentId,
        direction: Direction,
        allowed: impl Fn(AgentId) -> bool,
    ) -> Option<AgentId> {
        let structural = match (direction, self.spot(from)?) {
            (Direction::Down, Spot::Card(node)) => self.below_card(node, 0),
            (Direction::Down, Spot::Satellite(node, index)) => self.below_card(node, index + 1),
            (Direction::Up, Spot::Satellite(node, 0)) => Some(self.nodes[node].agent),
            (Direction::Up, Spot::Satellite(node, index)) => {
                self.nodes[node].satellites.get(index - 1).copied()
            }
            (Direction::Up, Spot::Card(node)) => self.above_card(node),
            _ => None,
        };
        match structural {
            Some(agent) if allowed(agent) => Some(agent),
            _ => self.nearest(from, direction, allowed),
        }
    }

    /// Where an agent is drawn.
    fn spot(&self, agent: AgentId) -> Option<Spot> {
        self.nodes.iter().enumerate().find_map(|(index, node)| {
            if node.agent == agent {
                Some(Spot::Card(index))
            } else {
                node.satellites
                    .iter()
                    .position(|satellite| *satellite == agent)
                    .map(|satellite| Spot::Satellite(index, satellite))
            }
        })
    }

    fn parent(&self, node: usize) -> Option<usize> {
        self.nodes
            .iter()
            .position(|parent| parent.children.contains(&node))
    }

    /// Below a card, after its satellites from `satellite` on.
    fn below_card(&self, node: usize, satellite: usize) -> Option<AgentId> {
        let card = &self.nodes[node];
        if let Some(next) = card.satellites.get(satellite) {
            return Some(*next);
        }
        if card.stacked {
            return card.children.first().map(|child| self.nodes[*child].agent);
        }
        if let Some(child) = card.children.iter().min_by_key(|child| {
            (i32::from(self.nodes[**child].place.center()) - i32::from(card.place.center())).abs()
        }) {
            return Some(self.nodes[*child].agent);
        }
        // The next card of the same stack.
        let parent = self.parent(node)?;
        let siblings = &self.nodes[parent].children;
        let position = siblings.iter().position(|sibling| *sibling == node)?;
        self.nodes[parent]
            .stacked
            .then(|| siblings.get(position + 1))
            .flatten()
            .map(|next| self.nodes[*next].agent)
    }

    /// Above a card: the previous card of its stack, or its parent.
    fn above_card(&self, node: usize) -> Option<AgentId> {
        let parent = self.parent(node)?;
        let siblings = &self.nodes[parent].children;
        let position = siblings.iter().position(|sibling| *sibling == node)?;
        if self.nodes[parent].stacked && position > 0 {
            let previous = &self.nodes[siblings[position - 1]];
            return Some(
                previous
                    .satellites
                    .last()
                    .copied()
                    .unwrap_or(previous.agent),
            );
        }
        Some(self.nodes[parent].agent)
    }

    /// The nearest card entirely beyond `from` in a direction, preferring
    /// cards aligned with it.
    fn nearest(
        &self,
        from: AgentId,
        direction: Direction,
        allowed: impl Fn(AgentId) -> bool,
    ) -> Option<AgentId> {
        super::spatial::nearest(&self.targets(), from, direction, allowed)
    }
}

/// The hierarchy as an indented list, for the agents panel when there is
/// no room for the view: each agent with its tree guides (`├─ `).
pub(crate) fn outline(registry: &Registry, focus: Option<AgentId>) -> Vec<(AgentId, String)> {
    fn walk(
        registry: &Registry,
        agent: &Agent,
        prefix: &str,
        guide: &str,
        out: &mut Vec<(AgentId, String)>,
        seen: &mut BTreeSet<AgentId>,
    ) {
        if !seen.insert(agent.id) {
            return;
        }
        out.push((agent.id, format!("{prefix}{guide}")));
        let children = registry.children(agent.id);
        let inner = match guide {
            "" => prefix.to_owned(),
            "└─ " => format!("{prefix}   "),
            _ => format!("{prefix}│  "),
        };
        for (index, child) in children.iter().enumerate() {
            let guide = if index + 1 == children.len() {
                "└─ "
            } else {
                "├─ "
            };
            walk(registry, child, &inner, guide, out, seen);
        }
    }
    let show = focus.map_or(Show::Everything, Show::Tree);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots(registry, show) {
        walk(registry, root, "", "", &mut out, &mut seen);
    }
    out
}

/// Box-drawing glyph for a cell joined up, down, left and right.
fn junction(joins: u8) -> &'static str {
    const CROSS: u8 = UP | DOWN | LEFT | RIGHT;
    const TEE_LEFT: u8 = UP | DOWN | LEFT;
    const TEE_RIGHT: u8 = UP | DOWN | RIGHT;
    const TEE_DOWN: u8 = DOWN | LEFT | RIGHT;
    const TEE_UP: u8 = UP | LEFT | RIGHT;
    const DOWN_LEFT: u8 = DOWN | LEFT;
    const DOWN_RIGHT: u8 = DOWN | RIGHT;
    const UP_LEFT: u8 = UP | LEFT;
    const UP_RIGHT: u8 = UP | RIGHT;
    match joins {
        CROSS => "┼",
        TEE_LEFT => "┤",
        TEE_RIGHT => "├",
        TEE_DOWN => "┬",
        TEE_UP => "┴",
        DOWN_LEFT => "┐",
        DOWN_RIGHT => "┌",
        UP_LEFT => "┘",
        UP_RIGHT => "└",
        _ if joins & (UP | DOWN) == 0 => "─",
        _ => "│",
    }
}

/// Directions a box-drawing cell joins, as bits for [`junction`].
const UP: u8 = 0b1000;
const DOWN: u8 = 0b0100;
const LEFT: u8 = 0b0010;
const RIGHT: u8 = 0b0001;

/// The bit of a direction, when `joined`.
const fn join(joined: bool, direction: u8) -> u8 {
    if joined { direction } else { 0 }
}

/// Draws the hierarchy in `area`, the inside of the view panel.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let scene = &model.scene;
    if scene.nodes.is_empty() {
        let message = if model.registry.agents().next().is_none() {
            model.settings.without_claude_code().unwrap_or(
                "No agents yet. Start Claude Code anywhere, or launch one with volee spawn.",
            )
        } else {
            "Nothing to show here."
        };
        widgets::centered_placeholder(model, message, area, buf);
        return;
    }
    let edges = Edges {
        normal: theme.style(StyleRole::Edge),
        hot: Style {
            fg: theme.style(StyleRole::CardSelected).fg,
            ..Style::default()
        },
        lineage: lineage(model),
    };
    let mut canvas = Canvas::new(buf, area, model.scene_offset);
    for node in &scene.nodes {
        if node.stacked {
            draw_spine(&mut canvas, scene, node, &edges);
        } else {
            draw_bus(&mut canvas, scene, node, &edges);
        }
    }

    let context = CardContext::new(model);
    for node in &scene.nodes {
        card::draw(&mut canvas, scene.zoom, node, &context);
    }

    // Joints where edges meet card borders, drawn over the borders.
    if scene.zoom == Zoom::Dots {
        return;
    }
    for node in scene.nodes.iter().filter(|node| !node.children.is_empty()) {
        let children = || node.children.iter().map(|child| &scene.nodes[*child]);
        let place = node.place;
        let bottom = i32::from(place.y + node.card_height) - 1;
        let hot = edges.style(children().any(|child| edges.is_hot(child.agent)));
        if node.stacked {
            joint(
                &mut canvas,
                i32::from(place.x) + 1,
                bottom,
                Joint::Below,
                hot,
            );
            for child in children() {
                let style = edges.style(edges.is_hot(child.agent));
                let row = i32::from(child.place.y) + 1;
                joint(
                    &mut canvas,
                    i32::from(child.place.x),
                    row,
                    Joint::Left,
                    style,
                );
            }
        } else {
            if node.satellites.is_empty() {
                joint(
                    &mut canvas,
                    i32::from(place.center()),
                    bottom,
                    Joint::Below,
                    hot,
                );
            }
            for child in children() {
                let style = edges.style(edges.is_hot(child.agent));
                let (x, y) = (i32::from(child.place.center()), i32::from(child.place.y));
                joint(&mut canvas, x, y, Joint::Above, style);
            }
        }
    }
}

/// Edge styles: the path from the selected agent up to its root is
/// highlighted.
struct Edges {
    normal: Style,
    hot: Style,
    lineage: BTreeSet<AgentId>,
}

impl Edges {
    fn is_hot(&self, child: AgentId) -> bool {
        self.lineage.contains(&child)
    }

    fn style(&self, hot: bool) -> Style {
        if hot { self.hot } else { self.normal }
    }
}

/// The selected agent and its ancestors.
fn lineage(model: &Model) -> BTreeSet<AgentId> {
    let mut lineage = BTreeSet::new();
    let mut current = model.selected;
    while let Some(id) = current {
        if !lineage.insert(id) {
            break;
        }
        current = model.registry.agent(id).and_then(|agent| agent.parent);
    }
    lineage
}

#[derive(Clone, Copy)]
enum Joint {
    /// An edge leaves a bottom border.
    Below,
    /// An edge arrives on a top border.
    Above,
    /// An edge arrives on a left border.
    Left,
}

fn joint(canvas: &mut Canvas<'_>, x: i32, y: i32, joint: Joint, style: Style) {
    let glyph = match (canvas.symbol(x, y), joint) {
        (Some("─"), Joint::Below) => "┬",
        (Some("━"), Joint::Below) => "┯",
        (Some("─"), Joint::Above) => "┴",
        (Some("━"), Joint::Above) => "┷",
        (Some("│"), Joint::Left) => "┤",
        (Some("┃"), Joint::Left) => "┨",
        _ => return,
    };
    canvas.put(x, y, glyph, style);
}

/// Edges from a parent to children side by side: down to a bus, along the
/// bus, and down into each child.
fn draw_bus(canvas: &mut Canvas<'_>, scene: &Scene, node: &SceneNode, edges: &Edges) {
    let children: Vec<&SceneNode> = node.children.iter().map(|c| &scene.nodes[*c]).collect();
    let Some(first) = children.first() else {
        return;
    };
    let top = i32::from(node.place.bottom());
    let bottom = i32::from(first.place.y) - 1;
    let bus = if bottom > top { bottom - 1 } else { top };
    let parent_x = i32::from(node.place.center());
    let anchors: Vec<(i32, bool)> = children
        .iter()
        .map(|child| (i32::from(child.place.center()), edges.is_hot(child.agent)))
        .collect();
    let any_hot = anchors.iter().any(|(_, hot)| *hot);
    for row in top..bus {
        canvas.put(parent_x, row, "│", edges.style(any_hot));
    }
    let left = anchors.iter().map(|(x, _)| *x).fold(parent_x, i32::min);
    let right = anchors.iter().map(|(x, _)| *x).fold(parent_x, i32::max);
    for x in left..=right {
        let down = anchors.iter().any(|(ax, _)| *ax == x);
        let glyph = junction(
            join(x == parent_x, UP)
                | join(down, DOWN)
                | join(x > left, LEFT)
                | join(x < right, RIGHT),
        );
        // Between the parent and a highlighted child, the bus is highlighted.
        let hot = anchors
            .iter()
            .any(|(ax, hot)| *hot && x >= parent_x.min(*ax) && x <= parent_x.max(*ax));
        canvas.put(x, bus, glyph, edges.style(hot));
    }
    for (x, hot) in &anchors {
        for row in bus + 1..=bottom {
            canvas.put(*x, row, "│", edges.style(*hot));
        }
    }
}

/// Edges from a parent to stacked children: a spine under its left edge,
/// and a branch into each child.
fn draw_spine(canvas: &mut Canvas<'_>, scene: &Scene, node: &SceneNode, edges: &Edges) {
    let children: Vec<&SceneNode> = node.children.iter().map(|c| &scene.nodes[*c]).collect();
    let dots = scene.zoom == Zoom::Dots;
    // Cards connect on their first inner row, dots on their only row.
    let connection = |child: &SceneNode| i32::from(child.place.y) + i32::from(!dots);
    let Some(last) = children.last() else {
        return;
    };
    // Under a card's left border, or under a dot's glyph.
    let spine = i32::from(node.place.x) + i32::from(!dots);
    let top = i32::from(node.place.bottom());
    let end = connection(last);
    for row in top..=end {
        // The spine is highlighted down to a highlighted child.
        let hot = children
            .iter()
            .any(|child| edges.is_hot(child.agent) && connection(child) >= row);
        let joins = children.iter().any(|child| connection(child) == row);
        let glyph = match (joins, row == end) {
            (_, true) => "└",
            (true, false) => "├",
            (false, false) => "│",
        };
        canvas.put(spine, row, glyph, edges.style(hot));
    }
    for child in &children {
        let row = connection(child);
        let style = edges.style(edges.is_hot(child.agent));
        // Dots keep a blank cell before their label.
        let stop = i32::from(child.place.x) - i32::from(dots);
        for x in spine + 1..stop {
            canvas.put(x, row, "─", style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::demo::{
        DemoSource, EXPLORE_HANDLERS, EXPLORE_TRACES, INFRA, INVOICE_PAGE, INVOICE_SCHEMA,
        INVOICE_TOOLS, INVOICING, LATENCY_AUDIT, TOWER, TRACE_GATEWAY, TRACE_SEARCH, agent_id,
    };
    use std::sync::Arc;
    use volee_core::time::Timestamp;

    fn start() -> Timestamp {
        Timestamp::from_millis(1_793_986_617_000)
    }

    fn registry() -> Registry {
        let demo = DemoSource::new(Arc::new(ManualClock::new(start())));
        Registry::from_events(&demo.events_until(start())).unwrap()
    }

    fn id(member: usize) -> AgentId {
        agent_id(start(), member)
    }

    #[test]
    fn subagents_are_satellites_and_independents_come_last() {
        let scene = Scene::build(
            &registry(),
            Show::Everything,
            ZoomChoice::Fixed(Zoom::Cards),
            (500, 100),
        );
        let gateway = scene
            .nodes
            .iter()
            .find(|n| n.agent == id(TRACE_GATEWAY))
            .unwrap();
        assert_eq!(
            gateway.satellites,
            [id(EXPLORE_HANDLERS), id(EXPLORE_TRACES)]
        );
        assert_eq!(gateway.place.height, 9 + 2);
        assert!(!scene.nodes.iter().any(|n| n.agent == id(EXPLORE_TRACES)));
        assert_eq!(scene.nodes[0].agent, id(TOWER));
        let infra = scene.nodes.iter().find(|n| n.agent == id(INFRA)).unwrap();
        assert!(infra.place.x > scene.nodes[0].place.x);
    }

    #[test]
    fn the_automatic_zoom_picks_the_most_detail_that_fits() {
        let registry = registry();
        let roomy = Scene::build(&registry, Show::Everything, ZoomChoice::Auto, (162, 32));
        assert_eq!(roomy.zoom, Zoom::Cards);
        assert!(roomy.automatic);
        assert!(roomy.width <= 162 && roomy.height <= 32);
        let medium = Scene::build(&registry, Show::Everything, ZoomChoice::Auto, (88, 24));
        assert_eq!(medium.zoom, Zoom::Compact);
        let small = Scene::build(&registry, Show::Everything, ZoomChoice::Auto, (40, 12));
        assert_eq!(small.zoom, Zoom::Dots);
        let fixed = Scene::build(
            &registry,
            Show::Everything,
            ZoomChoice::Fixed(Zoom::Cards),
            (40, 12),
        );
        assert_eq!(fixed.zoom, Zoom::Cards);
        assert!(!fixed.automatic);
    }

    #[test]
    fn a_focused_subtree_has_one_root() {
        let scene = Scene::build(
            &registry(),
            Show::Tree(id(INVOICING)),
            ZoomChoice::Auto,
            (162, 32),
        );
        let agents: Vec<AgentId> = scene.nodes.iter().map(|n| n.agent).collect();
        assert_eq!(
            agents,
            [
                id(INVOICING),
                id(INVOICE_SCHEMA),
                id(INVOICE_TOOLS),
                id(INVOICE_PAGE)
            ]
        );
    }

    #[test]
    fn spatial_navigation_follows_the_picture() {
        let scene = Scene::build(
            &registry(),
            Show::Everything,
            ZoomChoice::Fixed(Zoom::Cards),
            (500, 100),
        );
        let all = |_| true;
        let go = |from, direction| scene.neighbour(id(from), direction, all);
        assert_eq!(go(TOWER, Direction::Up), None);
        assert!(
            matches!(go(TOWER, Direction::Down), Some(a) if a == id(INVOICING) || a == id(LATENCY_AUDIT))
        );
        assert_eq!(go(INVOICE_SCHEMA, Direction::Up), Some(id(INVOICING)));
        assert_eq!(
            go(INVOICE_SCHEMA, Direction::Right),
            Some(id(INVOICE_TOOLS))
        );
        assert_eq!(go(INVOICE_TOOLS, Direction::Left), Some(id(INVOICE_SCHEMA)));
        assert_eq!(
            go(TRACE_GATEWAY, Direction::Down),
            Some(id(EXPLORE_HANDLERS))
        );
        assert_eq!(
            go(EXPLORE_HANDLERS, Direction::Down),
            Some(id(EXPLORE_TRACES))
        );
        assert_eq!(go(TRACE_SEARCH, Direction::Up), Some(id(LATENCY_AUDIT)));
        let none = |_| false;
        assert_eq!(scene.neighbour(id(TOWER), Direction::Down, none), None);
    }

    #[test]
    fn the_outline_draws_tree_guides() {
        let outline = outline(&registry(), Some(id(LATENCY_AUDIT)));
        let lines: Vec<&str> = outline.iter().map(|(_, guides)| guides.as_str()).collect();
        assert_eq!(lines, ["", "├─ ", "│  ├─ ", "│  └─ ", "└─ "]);
        assert_eq!(outline[4].0, id(TRACE_SEARCH));
    }

    #[test]
    fn junctions_join_the_right_directions() {
        assert_eq!(junction(UP | DOWN | LEFT | RIGHT), "┼");
        assert_eq!(junction(DOWN | RIGHT), "┌");
        assert_eq!(junction(DOWN | LEFT), "┐");
        assert_eq!(junction(UP | LEFT | RIGHT), "┴");
        assert_eq!(junction(DOWN | LEFT | RIGHT), "┬");
        assert_eq!(junction(UP | DOWN | RIGHT), "├");
        assert_eq!(junction(LEFT | RIGHT), "─");
        assert_eq!(junction(UP | DOWN), "│");
    }
}
