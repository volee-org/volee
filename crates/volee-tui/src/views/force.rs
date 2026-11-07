//! A stable force-directed layout for the network view.
//!
//! Nodes repel each other, links pull their two ends together, and a weak
//! pull toward the middle keeps lone nodes in view (Fruchterman and
//! Reingold, with a cooling schedule). Communication and lineage are the
//! links, so agents that talk group together whatever mission they belong
//! to: the picture answers "who knows whom". Three properties matter more
//! than beauty in a terminal:
//!
//! - **Deterministic.** Starting points come from a hash of each node's key,
//!   and the arithmetic uses only `+ - * /` and square roots, which IEEE 754
//!   rounds the same way on every machine: the same graph gives the same
//!   picture everywhere.
//! - **Computed once.** The layout runs when the graph changes (an agent or a
//!   pair of correspondents appears), never per frame, so nothing jitters.
//! - **Incremental.** A new layout starts from the previous positions, and a
//!   spring pulls every known node back toward where it was: adding an agent
//!   moves the others little. A new node starts next to its neighbours.

use std::collections::BTreeMap;

/// A position in the layout's plane.
pub(crate) type Point = (f64, f64);

/// The box new nodes start in, wider than tall like terminal labels. Nodes
/// move freely afterwards: walls would pile them up on the borders.
const WIDTH: f64 = 2.4;
const HEIGHT: f64 = 1.0;

/// Pull toward the middle, proportional to the distance: it keeps lone
/// nodes and separate groups in view without walls.
const GRAVITY: f64 = 0.3;

/// Pull of a known node back toward its previous position.
const ANCHOR: f64 = 1.5;

/// Rounds of a first layout, and of a layout that starts from a previous one.
const COLD_ROUNDS: u32 = 400;
const WARM_ROUNDS: u32 = 150;

/// A link between two nodes, by index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Link {
    /// One end.
    pub(crate) a: usize,
    /// The other end.
    pub(crate) b: usize,
    /// How hard it pulls: 1 for a conversation, less for lineage alone.
    pub(crate) strength: f64,
}

/// Places `keys.len()` nodes. `keys` identify nodes across layouts (an
/// agent keeps its key), `previous` holds positions returned by an earlier
/// call. Returns one position per node, in the layout's box; the caller
/// keeps them for the next call and stretches them to the screen.
pub(crate) fn relax(keys: &[u64], links: &[Link], previous: &BTreeMap<u64, Point>) -> Vec<Point> {
    let count = keys.len();
    if count == 0 {
        return Vec::new();
    }
    let ideal = (WIDTH * HEIGHT / float(count)).sqrt();
    let anchors: Vec<Option<Point>> = keys.iter().map(|key| previous.get(key).copied()).collect();
    let warm = anchors.iter().any(Option::is_some);
    let mut points = start(keys, links, &anchors, ideal);

    let (rounds, heat) = if warm {
        (WARM_ROUNDS, 0.04 * WIDTH)
    } else {
        (COLD_ROUNDS, 0.15 * WIDTH)
    };
    let center = (WIDTH / 2.0, HEIGHT / 2.0);
    let mut moves = vec![(0.0, 0.0); count];
    for round in 0..rounds {
        let temperature = heat * (1.0 - float_u32(round) / float_u32(rounds)) + 0.001;
        moves.fill((0.0, 0.0));
        for i in 0..count {
            for j in i + 1..count {
                let (dx, dy) = separation(&points, keys, i, j);
                let distance = (dx * dx + dy * dy).sqrt();
                let push = ideal * ideal / distance;
                let (fx, fy) = (dx / distance * push, dy / distance * push);
                moves[i].0 += fx;
                moves[i].1 += fy;
                moves[j].0 -= fx;
                moves[j].1 -= fy;
            }
        }
        for link in links {
            let (dx, dy) = (
                points[link.a].0 - points[link.b].0,
                points[link.a].1 - points[link.b].1,
            );
            let distance = (dx * dx + dy * dy).sqrt().max(1e-9);
            let pull = distance * distance / ideal * link.strength;
            let (fx, fy) = (dx / distance * pull, dy / distance * pull);
            moves[link.a].0 -= fx;
            moves[link.a].1 -= fy;
            moves[link.b].0 += fx;
            moves[link.b].1 += fy;
        }
        for (index, point) in points.iter_mut().enumerate() {
            let mut step = moves[index];
            step.0 += (center.0 - point.0) * GRAVITY;
            step.1 += (center.1 - point.1) * GRAVITY;
            if let Some(anchor) = anchors[index] {
                step.0 += (anchor.0 - point.0) * ANCHOR;
                step.1 += (anchor.1 - point.1) * ANCHOR;
            }
            let length = (step.0 * step.0 + step.1 * step.1).sqrt();
            if length > 0.0 {
                let scale = length.min(temperature) / length;
                point.0 += step.0 * scale;
                point.1 += step.1 * scale;
            }
        }
    }
    points
}

/// Starting points: previous positions, then next to placed neighbours,
/// then a spot hashed from the key.
fn start(keys: &[u64], links: &[Link], anchors: &[Option<Point>], ideal: f64) -> Vec<Point> {
    let mut points: Vec<Option<Point>> = anchors.to_vec();
    // Twice, so a chain of new nodes still finds placed neighbours.
    for _ in 0..2 {
        for index in 0..keys.len() {
            if points[index].is_some() {
                continue;
            }
            let neighbours: Vec<Point> = links
                .iter()
                .filter_map(|link| match (link.a == index, link.b == index) {
                    (true, _) => points[link.b],
                    (_, true) => points[link.a],
                    _ => None,
                })
                .collect();
            if neighbours.is_empty() {
                continue;
            }
            let n = float(neighbours.len());
            let (sx, sy) = neighbours
                .iter()
                .fold((0.0, 0.0), |(x, y), point| (x + point.0, y + point.1));
            let hash = mix(keys[index]);
            let jitter = |bits: u64| (unit(bits) - 0.5) * ideal * 0.5;
            points[index] = Some((
                (sx / n + jitter(hash)).clamp(0.0, WIDTH),
                (sy / n + jitter(mix(hash))).clamp(0.0, HEIGHT),
            ));
        }
    }
    keys.iter()
        .zip(points)
        .map(|(key, point)| {
            point.unwrap_or_else(|| {
                let hash = mix(*key);
                (unit(hash) * WIDTH, unit(mix(hash)) * HEIGHT)
            })
        })
        .collect()
}

/// From node `j` to node `i`, never zero: nodes on the same spot are pulled
/// apart in a direction hashed from their keys.
fn separation(points: &[Point], keys: &[u64], i: usize, j: usize) -> (f64, f64) {
    let (dx, dy) = (points[i].0 - points[j].0, points[i].1 - points[j].1);
    if dx * dx + dy * dy > 1e-12 {
        return (dx, dy);
    }
    let hash = mix(keys[i] ^ keys[j].rotate_left(17));
    (
        (unit(hash) - 0.5) * 1e-3 + 1e-6,
        (unit(mix(hash)) - 0.5) * 1e-3,
    )
}

/// Stretches positions to fill `[0, 1]` on each axis; a single position, or
/// positions on one line, sit in the middle of that axis.
pub(crate) fn normalize(points: &[Point]) -> Vec<Point> {
    let range = |values: &mut dyn Iterator<Item = f64>| {
        values.fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
            (low.min(value), high.max(value))
        })
    };
    let (left, right) = range(&mut points.iter().map(|point| point.0));
    let (top, bottom) = range(&mut points.iter().map(|point| point.1));
    let scale = |value: f64, low: f64, high: f64| {
        if high - low < 1e-9 {
            0.5
        } else {
            (value - low) / (high - low)
        }
    };
    points
        .iter()
        .map(|point| (scale(point.0, left, right), scale(point.1, top, bottom)))
        .collect()
}

/// A count as a float; counts here stay far below 2^32.
fn float(count: usize) -> f64 {
    f64::from(u32::try_from(count).unwrap_or(u32::MAX))
}

fn float_u32(value: u32) -> f64 {
    f64::from(value)
}

/// A well-mixed 64-bit hash (splitmix64).
fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The high bits of a hash as a number in `[0, 1)`.
fn unit(bits: u64) -> f64 {
    f64::from(u32::try_from(bits >> 32).unwrap_or(0)) / 4_294_967_296.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two clusters joined by one link, and a lone node.
    fn graph() -> (Vec<u64>, Vec<Link>) {
        let keys: Vec<u64> = (1..=9).map(|n| n * 1_000_003).collect();
        let link = |a, b| Link {
            a,
            b,
            strength: 1.0,
        };
        let links = vec![
            link(0, 1),
            link(0, 2),
            link(1, 2),
            link(3, 4),
            link(3, 5),
            link(4, 5),
            link(2, 3),
            link(6, 7),
        ];
        (keys, links)
    }

    fn distance(a: Point, b: Point) -> f64 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    #[test]
    fn the_same_graph_gives_the_same_positions() {
        let (keys, links) = graph();
        let first = relax(&keys, &links, &BTreeMap::new());
        let second = relax(&keys, &links, &BTreeMap::new());
        assert_eq!(first, second);
        // Gravity keeps everything near the box new nodes start in.
        assert!(
            first
                .iter()
                .all(|(x, y)| (-WIDTH..=2.0 * WIDTH).contains(x)
                    && (-WIDTH..=2.0 * WIDTH).contains(y))
        );
    }

    #[test]
    fn linked_nodes_end_closer_than_strangers() {
        let (keys, links) = graph();
        let points = relax(&keys, &links, &BTreeMap::new());
        let within = distance(points[0], points[1]);
        let across = distance(points[0], points[5]);
        assert!(within < across, "{within} vs {across}");
        for i in 0..points.len() {
            for j in i + 1..points.len() {
                assert!(distance(points[i], points[j]) > 0.05, "{i} and {j} overlap");
            }
        }
    }

    #[test]
    fn adding_a_node_moves_the_others_little() {
        let (mut keys, mut links) = graph();
        let before = relax(&keys, &links, &BTreeMap::new());
        let previous: BTreeMap<u64, Point> = keys.iter().copied().zip(before.clone()).collect();
        keys.push(77);
        links.push(Link {
            a: 9,
            b: 4,
            strength: 1.0,
        });
        let after = relax(&keys, &links, &previous);
        let (old, new) = (normalize(&before), normalize(&after[..9]));
        let moved = old
            .iter()
            .zip(&new)
            .map(|(a, b)| distance(*a, *b))
            .fold(0.0, f64::max);
        assert!(moved < 0.15, "a known node moved by {moved}");
        assert!(
            distance(after[9], after[4]) < distance(after[9], after[0]),
            "the new node starts next to its neighbour"
        );
        // Relaxing again from the result changes almost nothing: stable.
        let again: BTreeMap<u64, Point> = keys.iter().copied().zip(after.clone()).collect();
        let settled = relax(&keys, &links, &again);
        let drift = normalize(&after)
            .iter()
            .zip(&normalize(&settled))
            .map(|(a, b)| distance(*a, *b))
            .fold(0.0, f64::max);
        assert!(drift < 0.05, "drift {drift}");
    }

    #[test]
    fn normalizing_fills_the_unit_square() {
        let points = normalize(&[(0.25, 0.5), (1.25, 0.5), (0.75, 1.0)]);
        assert_eq!(points, [(0.0, 0.0), (1.0, 0.0), (0.5, 1.0)]);
        assert_eq!(normalize(&[(0.3, 0.3)]), [(0.5, 0.5)]);
        assert_eq!(relax(&[], &[], &BTreeMap::new()), Vec::<Point>::new());
    }
}
