//! The links of the tree, and what travels along them.
//!
//! A link runs from under a parent's card to each child's card: down to a
//! bus and along it for children side by side, down a spine for children
//! stacked in a column. It is drawn light, dotted, in a tint of the child's
//! color, with a dot where links meet; the selected agent's lineage is drawn
//! over it in solid lines. Dots flow down the links of children at work, and
//! each message travels as a small comet from the sender's card to the
//! recipient's, up to their common ancestor and down again.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ratatui::style::{Color, Modifier, Style};
use volee_core::message::{MessageKind, Party};
use volee_core::{AgentId, AgentState};

use super::hierarchy::{Scene, SceneNode, Zoom};
use crate::app::{Model, Motion};
use crate::pulses::Pulse;
use crate::theme::{StyleRole, mix};
use crate::widgets::canvas::Canvas;
use crate::widgets::card::CardContext;

/// How much of the child's color tints a link.
const TINT: f64 = 0.35;

/// Cells between two dots flowing down a link.
const FLOW_SPACING: i64 = 6;

/// Cells a flowing dot moves per second.
const FLOW_SPEED: i64 = 12;

/// The link from a parent's card to a child's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Link {
    /// The child, whose color the link takes.
    pub(crate) child: AgentId,
    /// The parent.
    pub(crate) parent: AgentId,
    /// The cells, from under the parent's card to above the child's (or
    /// left of it, for stacked children).
    pub(crate) cells: Vec<(i32, i32)>,
    /// Whether the link ends on the child's left border.
    stacked: bool,
}

/// The links of a scene, one per child.
pub(crate) fn links(scene: &Scene) -> Vec<Link> {
    let mut out = Vec::new();
    for node in &scene.nodes {
        if node.stacked {
            spine(scene, node, &mut out);
        } else {
            bus(scene, node, &mut out);
        }
    }
    out
}

/// Links to children side by side: down from the parent to a bus, along it,
/// and down into each child.
fn bus(scene: &Scene, node: &SceneNode, out: &mut Vec<Link>) {
    let children: Vec<&SceneNode> = node.children.iter().map(|c| &scene.nodes[*c]).collect();
    let Some(first) = children.first() else {
        return;
    };
    let top = i32::from(node.place.bottom());
    let bottom = i32::from(first.place.y) - 1;
    let bus = if bottom > top { bottom - 1 } else { top };
    let parent_x = i32::from(node.place.center());
    for child in children {
        let child_x = i32::from(child.place.center());
        let mut cells: Vec<(i32, i32)> = (top..bus).map(|y| (parent_x, y)).collect();
        let step = if child_x >= parent_x { 1 } else { -1 };
        let mut x = parent_x;
        loop {
            cells.push((x, bus));
            if x == child_x {
                break;
            }
            x += step;
        }
        cells.extend((bus + 1..=bottom).map(|y| (child_x, y)));
        out.push(Link {
            child: child.agent,
            parent: node.agent,
            cells,
            stacked: false,
        });
    }
}

/// Links to children stacked in a column: down a spine under the parent's
/// left edge, and across into each child.
fn spine(scene: &Scene, node: &SceneNode, out: &mut Vec<Link>) {
    let dots = scene.zoom == Zoom::Dots;
    // Cards connect on their first inner row, dots on their only row.
    let spine = i32::from(node.place.x) + i32::from(!dots);
    let top = i32::from(node.place.bottom());
    for child in node.children.iter().map(|c| &scene.nodes[*c]) {
        let row = i32::from(child.place.y) + i32::from(!dots);
        // Dots keep a blank cell before their label.
        let stop = i32::from(child.place.x) - i32::from(dots);
        let mut cells: Vec<(i32, i32)> = (top..=row).map(|y| (spine, y)).collect();
        cells.extend((spine + 1..stop).map(|x| (x, row)));
        out.push(Link {
            child: child.agent,
            parent: node.agent,
            cells,
            stacked: true,
        });
    }
}

/// The directions a cell of the tree joins, for its glyph.
#[derive(Clone, Copy, Debug, Default)]
struct Mark {
    down: bool,
    across: bool,
    /// How many links pass here.
    owners: u8,
    /// The tint of the last link that passed here.
    tint: Option<Color>,
}

/// Draws the links of the scene, then what travels along them: the light
/// links, the selected lineage in solid lines, the dots that flow to
/// children at work and the messages on their way.
pub(crate) fn draw(canvas: &mut Canvas<'_>, links: &[Link], context: &CardContext<'_>) {
    let model = context.model();
    let theme = &model.theme;
    let edge = theme.style(StyleRole::Edge);
    let edge_color = edge.fg.unwrap_or(Color::Reset);
    let tint_of = |agent: AgentId| -> Option<Color> {
        let accent = model.registry.agent(agent).map(|a| context.accent(a))?;
        Some(mix(edge_color, accent.fg?, TINT))
    };

    // The light links: one glyph per cell, from every link through it.
    let mut marks: BTreeMap<(i32, i32), Mark> = BTreeMap::new();
    for link in links {
        let tint = tint_of(link.child);
        for (index, cell) in link.cells.iter().enumerate() {
            let mark = marks.entry(*cell).or_default();
            mark.owners = mark.owners.saturating_add(1);
            mark.tint = tint;
            let neighbours = [
                index.checked_sub(1).and_then(|i| link.cells.get(i)),
                link.cells.get(index + 1),
            ];
            for other in neighbours.into_iter().flatten() {
                if other.0 == cell.0 {
                    mark.down = true;
                } else {
                    mark.across = true;
                }
            }
            if link.cells.len() == 1 {
                mark.down = true;
            }
        }
    }
    for ((x, y), mark) in &marks {
        let glyph = match (mark.down, mark.across) {
            (true, true) => "•",
            (false, true) => "┈",
            _ => "┊",
        };
        let style = match mark.tint {
            Some(color) if mark.owners == 1 => Style::default().fg(color),
            _ => edge,
        };
        canvas.put(*x, *y, glyph, style);
    }

    // The lineage of the selected agent, solid, over the light links.
    let lineage = lineage(model);
    let hot = Style {
        fg: theme.style(StyleRole::CardSelected).fg,
        ..Style::default()
    };
    for link in links.iter().filter(|link| lineage.contains(&link.child)) {
        for (index, (x, y)) in link.cells.iter().enumerate() {
            let glyph = super::hierarchy::junction(joins(link, index));
            canvas.put(*x, *y, glyph, hot);
        }
    }

    if model.motion == Motion::Full {
        flow(canvas, links, &lineage, hot, context);
    }
    messages(canvas, links, context);
}

/// The selected agent and its ancestors.
pub(crate) fn lineage(model: &Model) -> BTreeSet<AgentId> {
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

/// The directions the cell `index` of `link` joins, for a solid glyph.
fn joins(link: &Link, index: usize) -> u8 {
    use super::hierarchy::{DOWN, LEFT, RIGHT, UP};
    let (x, y) = link.cells[index];
    let toward = |(ox, oy): (i32, i32)| -> u8 {
        match (ox.cmp(&x), oy.cmp(&y)) {
            (std::cmp::Ordering::Less, _) => LEFT,
            (std::cmp::Ordering::Greater, _) => RIGHT,
            (_, std::cmp::Ordering::Less) => UP,
            _ => DOWN,
        }
    };
    let mut joins = 0;
    if index == 0 {
        // Under the parent's card.
        joins |= UP;
    } else {
        joins |= toward(link.cells[index - 1]);
    }
    if let Some(next) = link.cells.get(index + 1) {
        joins |= toward(*next);
    } else if link.stacked {
        joins |= RIGHT;
    } else {
        joins |= DOWN;
    }
    joins
}

/// Dots flowing from parents down the links of children at work.
fn flow(
    canvas: &mut Canvas<'_>,
    links: &[Link],
    lineage: &BTreeSet<AgentId>,
    hot: Style,
    context: &CardContext<'_>,
) {
    let model = context.model();
    let step = model.now.as_millis() * FLOW_SPEED / 1_000;
    for link in links {
        let Some(child) = model.registry.agent(link.child) else {
            continue;
        };
        if !matches!(child.state, AgentState::Working | AgentState::Launching) {
            continue;
        }
        let style = if lineage.contains(&link.child) {
            hot
        } else {
            context.accent(child).remove_modifier(Modifier::BOLD)
        };
        for (index, (x, y)) in (0_i64..).zip(&link.cells) {
            if (index - step).rem_euclid(FLOW_SPACING) == 0 {
                canvas.put(*x, *y, "•", style);
            }
        }
    }
}

/// The messages on their way: a comet `●•·` from the sender's card to the
/// recipient's, or, with reduced motion, the route lit for a moment.
fn messages(canvas: &mut Canvas<'_>, links: &[Link], context: &CardContext<'_>) {
    let model = context.model();
    let by_child: HashMap<AgentId, &Link> = links.iter().map(|link| (link.child, link)).collect();
    for (pulse, progress) in model.pulses.packets(model.now) {
        let route = route(pulse, &by_child);
        if route.is_empty() {
            continue;
        }
        let color = model
            .theme
            .style(packet_role(pulse.kind))
            .fg
            .unwrap_or(Color::Reset);
        if model.motion == Motion::Reduced {
            let style = Style::default().fg(color);
            for (x, y) in &route {
                let glyph = canvas.symbol(*x, *y).unwrap_or("·").to_owned();
                canvas.put(*x, *y, &glyph, style);
            }
            continue;
        }
        let last = route.len() - 1;
        // Rounded down: the comet reaches the end only when it arrived.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let head = ((progress * last as f64).floor() as usize).min(last);
        let base = model.theme.base();
        for (behind, glyph) in ["●", "•", "·"].into_iter().enumerate() {
            let Some(index) = head.checked_sub(behind) else {
                break;
            };
            let (x, y) = route[index];
            let faded = base.map_or(color, |base| {
                // At most 2: three glyphs.
                #[allow(clippy::cast_precision_loss)]
                let amount = behind as f64 * 0.3;
                mix(color, base, amount)
            });
            let mut style = Style::default().fg(faded);
            if behind == 0 {
                style = style.add_modifier(Modifier::BOLD);
            }
            canvas.put(x, y, glyph, style);
        }
    }
}

/// The cells a message travels: up the links from the sender to the common
/// ancestor of both cards, then down to the recipient. A message from the
/// person comes down the recipient's own link; one to the person, or
/// between cards not on screen, travels nowhere.
fn route(pulse: &Pulse, by_child: &HashMap<AgentId, &Link>) -> Vec<(i32, i32)> {
    let Party::Agent(to) = pulse.to else {
        return Vec::new();
    };
    let from = match pulse.from {
        Party::Agent(from) => from,
        Party::Human => {
            return by_child
                .get(&to)
                .map(|link| link.cells.clone())
                .unwrap_or_default();
        }
        Party::System => return Vec::new(),
    };
    if from == to {
        return Vec::new();
    }
    // Each card's chain of ancestors on screen, itself first.
    let chain = |start: AgentId| -> Vec<AgentId> {
        let mut chain = vec![start];
        let mut current = start;
        while let Some(link) = by_child.get(&current) {
            if chain.contains(&link.parent) {
                break;
            }
            chain.push(link.parent);
            current = link.parent;
        }
        chain
    };
    let up = chain(from);
    let down = chain(to);
    let Some(common) = up.iter().find(|agent| down.contains(agent)).copied() else {
        return Vec::new();
    };
    let mut cells = Vec::new();
    for agent in up.iter().take_while(|agent| **agent != common) {
        if let Some(link) = by_child.get(agent) {
            cells.extend(link.cells.iter().rev());
        }
    }
    let descent: Vec<AgentId> = down
        .iter()
        .take_while(|agent| **agent != common)
        .copied()
        .collect();
    for agent in descent.iter().rev() {
        if let Some(link) = by_child.get(agent) {
            cells.extend(link.cells.iter());
        }
    }
    cells
}

/// The color of a message on its way, by what it is for.
fn packet_role(kind: MessageKind) -> StyleRole {
    match kind {
        MessageKind::Question => StyleRole::EdgeQuestion,
        MessageKind::Report => StyleRole::StateDone,
        MessageKind::Alert => StyleRole::StateFailed,
        MessageKind::Answer => StyleRole::CardFlash,
        MessageKind::Contract | MessageKind::Note => StyleRole::CardMail,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ulid::Ulid;
    use volee_core::message::{Message, Receipt};
    use volee_core::time::Timestamp;
    use volee_core::{MessageId, ThreadId};

    use super::*;
    use crate::demo::{INVOICE_SCHEMA, INVOICE_TOOLS, INVOICING, TOWER, TRACE_GATEWAY};
    use crate::pulses::{HOLD_MILLIS, PULSE_MILLIS};
    use crate::test_support::{id, model};

    fn message(from: Party, to: AgentId, sent: Timestamp) -> Message {
        Message {
            id: MessageId::from_ulid(Ulid::from_parts(9, 1)),
            thread: ThreadId::from_ulid(Ulid::from_parts(9, 2)),
            from,
            kind: MessageKind::Note,
            body: "the schema is in".into(),
            refs: vec![],
            in_reply_to: None,
            options: Vec::new(),
            at: sent,
            receipts: vec![Receipt {
                to: Party::Agent(to),
                delivered_at: None,
                read_at: None,
            }],
        }
    }

    fn pulse(from: Party, to: AgentId) -> Pulse {
        Pulse {
            from,
            to: Party::Agent(to),
            at: Timestamp::from_millis(0),
            kind: MessageKind::Note,
        }
    }

    fn by_child(links: &[Link]) -> HashMap<AgentId, &Link> {
        links.iter().map(|link| (link.child, link)).collect()
    }

    #[test]
    fn a_message_goes_up_to_the_common_ancestor_and_down_again() {
        let model = model(200, 50);
        let links = links(&model.scene);
        let by_child = by_child(&links);
        // Cousins: under two coordinators of the same lead.
        let cousins = route(
            &pulse(Party::Agent(id(INVOICE_SCHEMA)), id(TRACE_GATEWAY)),
            &by_child,
        );
        let schema = by_child[&id(INVOICE_SCHEMA)];
        let gateway = by_child[&id(TRACE_GATEWAY)];
        let invoicing = by_child[&id(INVOICING)];
        assert_eq!(
            cousins.first(),
            schema.cells.last(),
            "it leaves above the sender"
        );
        assert_eq!(
            cousins.last(),
            gateway.cells.last(),
            "it lands above the recipient"
        );
        assert!(
            cousins.contains(&invoicing.cells[0]),
            "up through its coordinator"
        );
        assert!(!by_child.contains_key(&id(TOWER)), "the lead is a root");
        // Siblings meet on their parent's bus: up one link, down the other.
        let siblings = route(
            &pulse(Party::Agent(id(INVOICE_SCHEMA)), id(INVOICE_TOOLS)),
            &by_child,
        );
        let tools = by_child[&id(INVOICE_TOOLS)];
        assert_eq!(siblings.len(), schema.cells.len() + tools.cells.len());
    }

    #[test]
    fn a_message_from_the_person_comes_down_the_recipient_s_link() {
        let model = model(200, 50);
        let links = links(&model.scene);
        let by_child = by_child(&links);
        let down = route(&pulse(Party::Human, id(INVOICE_TOOLS)), &by_child);
        assert_eq!(down, by_child[&id(INVOICE_TOOLS)].cells);
        // To the lead, which hangs from nothing: no route.
        assert_eq!(route(&pulse(Party::Human, id(TOWER)), &by_child), []);
    }

    #[test]
    fn a_message_on_its_way_is_a_comet_on_its_route() {
        let mut model = model(200, 50);
        let sent = model.now.plus_millis(-500);
        model.pulses.record(
            &message(Party::Agent(id(INVOICE_SCHEMA)), id(TRACE_GATEWAY), sent),
            model.now,
        );
        let area = Rect::new(0, 0, 200, 50);
        let draw = |model: &Model| {
            let mut buffer = Buffer::empty(area);
            crate::views::hierarchy::render(model, area, &mut buffer);
            buffer
        };
        let links = links(&model.scene);
        let cells = route(
            &pulse(Party::Agent(id(INVOICE_SCHEMA)), id(TRACE_GATEWAY)),
            &by_child(&links),
        );
        let (ox, oy) = model.scene_offset;
        let on_route = |buffer: &Buffer, glyph: &str| {
            cells.iter().any(|(x, y)| {
                let (Ok(cx), Ok(cy)) = (u16::try_from(x - ox), u16::try_from(y - oy)) else {
                    return false;
                };
                cx < area.width && cy < area.height && buffer[(cx, cy)].symbol() == glyph
            })
        };
        let buffer = draw(&model);
        assert!(on_route(&buffer, "●"), "the head of the comet");
        assert!(on_route(&buffer, "·"), "the end of its tail");
        // Arrived and rested, it is gone.
        model.now = sent.plus_millis(PULSE_MILLIS + HOLD_MILLIS);
        model.pulses.prune(model.now);
        let later = draw(&model);
        assert!(!later.content().iter().any(|cell| cell.symbol() == "●"));
    }
}
