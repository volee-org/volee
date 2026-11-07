//! A tidy tree layout for terminal cards.
//!
//! The forest is placed top-down in the style of Reingold and Tilford:
//! children sit side by side under their parent, each subtree is pushed as
//! close to its left neighbours as their contours allow (row by row, so a
//! narrow deep subtree can sit next to a wide shallow one), and every parent
//! is centered over its first and last child.
//!
//! Terminals are wide and short, so two extensions trade width for height:
//!
//! - a parent whose children are all leaves may **stack** them in a column
//!   under its left edge, joined by a spine;
//! - roots without children may be stacked in a column right of the trees.
//!
//! [`fit`] stacks groups, the most profitable first, until the forest fits a
//! width. The layout depends only on the forest, so it does not move when
//! the selection changes.

use std::collections::BTreeSet;

/// A node to place: a card with its satellites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    /// Width in cells.
    pub(crate) width: u16,
    /// Height in cells.
    pub(crate) height: u16,
    /// The column, from the node's left edge, where edges attach: the
    /// middle of a card, or the glyph of a dot.
    pub(crate) anchor: u16,
    /// Children, in display order.
    pub(crate) children: Vec<usize>,
}

/// Nodes and the roots of their trees, in display order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Forest {
    /// Every node.
    pub(crate) nodes: Vec<Node>,
    /// The roots.
    pub(crate) roots: Vec<usize>,
}

/// Room between cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Spacing {
    /// Columns between neighbouring subtrees.
    pub(crate) gap: u16,
    /// Rows between a parent and children placed side by side, for edges.
    pub(crate) level: u16,
    /// Columns from a parent's left edge to its stacked children; the spine
    /// runs in the second of them.
    pub(crate) indent: u16,
    /// Columns between the trees of the forest.
    pub(crate) forest_gap: u16,
}

/// The groups placed in a column instead of a row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stacking {
    /// Parents whose children are stacked.
    pub(crate) parents: BTreeSet<usize>,
    /// Whether roots without children are stacked.
    pub(crate) lone_roots: bool,
}

/// Where a node goes, in cells from the top left of the forest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Placement {
    /// Left column.
    pub(crate) x: u16,
    /// Top row.
    pub(crate) y: u16,
    /// Width.
    pub(crate) width: u16,
    /// Height.
    pub(crate) height: u16,
    /// Where edges attach, from the left edge.
    pub(crate) anchor: u16,
}

impl Placement {
    /// The column where edges arrive and leave.
    pub(crate) fn center(&self) -> u16 {
        self.x + self.anchor
    }

    /// The row below the node.
    pub(crate) fn bottom(&self) -> u16 {
        self.y + self.height
    }
}

/// A placed forest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    /// One placement per node of the forest.
    pub(crate) nodes: Vec<Placement>,
    /// Total width, edges included.
    pub(crate) width: u16,
    /// Total height.
    pub(crate) height: u16,
    /// What was stacked to get here.
    pub(crate) stacking: Stacking,
}

/// Row-by-row extent of a subtree: for each row, the leftmost column used
/// and one past the rightmost.
#[derive(Clone, Debug, Default)]
struct Contour {
    rows: Vec<Option<(i32, i32)>>,
}

impl Contour {
    fn add(&mut self, row: i32, left: i32, right: i32) {
        let Ok(row) = usize::try_from(row) else {
            return;
        };
        if self.rows.len() <= row {
            self.rows.resize(row + 1, None);
        }
        let slot = &mut self.rows[row];
        *slot = Some(match *slot {
            Some((l, r)) => (l.min(left), r.max(right)),
            None => (left, right),
        });
    }

    fn add_rect(&mut self, x: i32, y: i32, width: i32, height: i32) {
        for row in y..y + height {
            self.add(row, x, x + width);
        }
    }

    fn merge(&mut self, other: &Self, dx: i32, dy: i32) {
        for (row, extent) in (0..).zip(&other.rows) {
            if let Some((left, right)) = extent {
                self.add(row + dy, left + dx, right + dx);
            }
        }
    }

    /// The smallest shift that keeps `right` at least `gap` columns right of
    /// `left` on every row they share.
    fn separation(left: &Self, right: &Self, gap: i32) -> i32 {
        left.rows
            .iter()
            .zip(&right.rows)
            .filter_map(|pair| match pair {
                (Some((_, l_right)), Some((r_left, _))) => Some(l_right + gap - r_left),
                _ => None,
            })
            .max()
            .unwrap_or_else(|| left.right() + gap - right.left())
    }

    fn left(&self) -> i32 {
        self.rows
            .iter()
            .flatten()
            .map(|(l, _)| *l)
            .min()
            .unwrap_or(0)
    }

    fn right(&self) -> i32 {
        self.rows
            .iter()
            .flatten()
            .map(|(_, r)| *r)
            .max()
            .unwrap_or(0)
    }

    /// The bounding box as a contour.
    fn boxed(&self) -> Self {
        let mut boxed = Self::default();
        boxed.add_rect(self.left(), 0, self.right() - self.left(), self.height());
        boxed
    }

    fn height(&self) -> i32 {
        i32::try_from(self.rows.len()).unwrap_or(i32::MAX)
    }
}

/// A placed subtree, relative to its own origin.
#[derive(Clone, Debug, Default)]
struct Sub {
    places: Vec<(usize, i32, i32)>,
    contour: Contour,
    /// The column of the root's top center.
    anchor: i32,
}

impl Sub {
    fn merge(&mut self, other: Self, dx: i32, dy: i32) {
        self.contour.merge(&other.contour, dx, dy);
        self.places.extend(
            other
                .places
                .into_iter()
                .map(|(n, x, y)| (n, x + dx, y + dy)),
        );
    }
}

struct Placer<'a> {
    forest: &'a Forest,
    spacing: Spacing,
    stacking: &'a Stacking,
}

impl Placer<'_> {
    fn node(&self, index: usize) -> (i32, i32) {
        let node = &self.forest.nodes[index];
        (i32::from(node.width), i32::from(node.height))
    }

    fn is_leaf(&self, index: usize) -> bool {
        self.forest.nodes[index].children.is_empty()
    }

    fn anchor(&self, index: usize) -> i32 {
        i32::from(self.forest.nodes[index].anchor)
    }

    fn leaf(&self, index: usize) -> Sub {
        let (width, height) = self.node(index);
        let mut contour = Contour::default();
        contour.add_rect(0, 0, width, height);
        Sub {
            places: vec![(index, 0, 0)],
            contour,
            anchor: self.anchor(index),
        }
    }

    fn subtree(&self, index: usize) -> Sub {
        let mut sub = self.leaf(index);
        let children = &self.forest.nodes[index].children;
        if children.is_empty() {
            return sub;
        }
        let (width, height) = self.node(index);
        if self.stacking.parents.contains(&index) && children.iter().all(|c| self.is_leaf(*c)) {
            let indent = i32::from(self.spacing.indent);
            let mut y = height;
            for child in children {
                let (child_width, child_height) = self.node(*child);
                sub.places.push((*child, indent, y));
                // From the spine, under the parent's left edge, to the
                // child's right edge.
                sub.contour
                    .add_rect(0, y, indent + child_width, child_height);
                y += child_height;
            }
            return sub;
        }

        let (row, anchors) = row(children.iter().map(|c| self.subtree(*c)), self.spacing.gap);
        let (first, last) = (anchors[0], anchors[anchors.len() - 1]);
        let middle = (first + last).div_euclid(2);
        let x = middle - self.anchor(index);
        let level = i32::from(self.spacing.level);
        let mut placed = Sub {
            places: vec![(index, x, 0)],
            contour: Contour::default(),
            anchor: middle,
        };
        placed.contour.add_rect(x, 0, width, height);
        // Keep the edges' rows clear of other subtrees.
        for row in height..height + level {
            placed
                .contour
                .add(row, first.min(middle), last.max(middle) + 1);
        }
        placed.merge(row, 0, height + level);
        placed
    }

    fn forest(&self) -> Sub {
        let roots = &self.forest.roots;
        let mut items: Vec<Sub> = Vec::new();
        if self.stacking.lone_roots {
            let mut column = Sub::default();
            let mut y = 0;
            for root in roots {
                if self.is_leaf(*root) {
                    column.merge(self.leaf(*root), 0, y);
                    y += self.node(*root).1;
                } else {
                    items.push(self.subtree(*root));
                }
            }
            if !column.places.is_empty() {
                items.push(column);
            }
        } else {
            items.extend(roots.iter().map(|root| self.subtree(*root)));
        }
        // Trees never interleave: each one is kept in its bounding box.
        let boxed = items.into_iter().map(|mut item| {
            item.contour = item.contour.boxed();
            item
        });
        row(boxed, self.spacing.forest_gap).0
    }
}

/// Subtrees side by side, as close as their contours allow. Returns the row
/// and the anchor of each subtree in it.
fn row(subs: impl Iterator<Item = Sub>, gap: u16) -> (Sub, Vec<i32>) {
    let gap = i32::from(gap);
    let mut row = Sub::default();
    let mut anchors = Vec::new();
    for (index, sub) in subs.enumerate() {
        let dx = if index == 0 {
            0
        } else {
            Contour::separation(&row.contour, &sub.contour, gap)
        };
        anchors.push(sub.anchor + dx);
        row.merge(sub, dx, 0);
    }
    (row, anchors)
}

/// Places a forest with the given groups stacked.
pub(crate) fn layout(forest: &Forest, spacing: Spacing, stacking: &Stacking) -> Layout {
    let placer = Placer {
        forest,
        spacing,
        stacking,
    };
    let sub = placer.forest();
    let left = sub.contour.left();
    let mut nodes = vec![Placement::default(); forest.nodes.len()];
    for (index, x, y) in &sub.places {
        let node = &forest.nodes[*index];
        nodes[*index] = Placement {
            x: u16::try_from(x - left).unwrap_or(u16::MAX),
            y: u16::try_from(*y).unwrap_or(u16::MAX),
            width: node.width,
            height: node.height,
            anchor: node.anchor,
        };
    }
    Layout {
        nodes,
        width: u16::try_from(sub.contour.right() - left).unwrap_or(u16::MAX),
        height: u16::try_from(sub.contour.height()).unwrap_or(u16::MAX),
        stacking: stacking.clone(),
    }
}

/// Places a forest within `max_width` if it can: groups are stacked one at
/// a time, each time the one that leaves the narrowest forest, until it fits
/// or nothing is left to stack. The result may still be wider than
/// `max_width`; the view then scrolls.
pub(crate) fn fit(forest: &Forest, spacing: Spacing, max_width: u16) -> Layout {
    let mut best = layout(forest, spacing, &Stacking::default());
    while best.width > max_width {
        let stackable_parents = forest.nodes.iter().enumerate().filter(|(index, node)| {
            node.children.len() >= 2
                && node
                    .children
                    .iter()
                    .all(|child| forest.nodes[*child].children.is_empty())
                && !best.stacking.parents.contains(index)
        });
        let mut candidates: Vec<Stacking> = stackable_parents
            .map(|(index, _)| {
                let mut stacking = best.stacking.clone();
                stacking.parents.insert(index);
                stacking
            })
            .collect();
        let lone = forest
            .roots
            .iter()
            .filter(|root| forest.nodes[**root].children.is_empty())
            .count();
        if lone >= 2 && !best.stacking.lone_roots {
            candidates.push(Stacking {
                lone_roots: true,
                ..best.stacking.clone()
            });
        }
        let next = candidates
            .iter()
            .map(|stacking| layout(forest, spacing, stacking))
            .min_by_key(|candidate| (candidate.width, candidate.height));
        match next {
            Some(candidate) if candidate.width <= best.width => best = candidate,
            _ => break,
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACING: Spacing = Spacing {
        gap: 2,
        level: 2,
        indent: 3,
        forest_gap: 4,
    };

    fn node(width: u16, height: u16, children: &[usize]) -> Node {
        Node {
            width,
            height,
            anchor: width / 2,
            children: children.to_vec(),
        }
    }

    /// The demo's shape: a lead with two missions, one worker with
    /// satellites (taller), and two independent sessions.
    fn demo_forest() -> Forest {
        Forest {
            nodes: vec![
                node(30, 6, &[1, 5]),    // 0 lead
                node(30, 6, &[2, 3, 4]), // 1 coordinator
                node(30, 6, &[]),        // 2 worker
                node(30, 6, &[]),        // 3 worker
                node(30, 6, &[]),        // 4 worker
                node(30, 6, &[6, 7]),    // 5 coordinator
                node(30, 8, &[]),        // 6 worker with two satellites
                node(30, 6, &[]),        // 7 worker
                node(30, 6, &[]),        // 8 independent
                node(30, 6, &[]),        // 9 independent
            ],
            roots: vec![0, 8, 9],
        }
    }

    fn overlaps(a: &Placement, b: &Placement) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }

    fn assert_no_overlap(layout: &Layout) {
        for (i, a) in layout.nodes.iter().enumerate() {
            assert!(a.x + a.width <= layout.width && a.y + a.height <= layout.height);
            for b in &layout.nodes[i + 1..] {
                assert!(!overlaps(a, b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn a_single_node_sits_at_the_origin() {
        let forest = Forest {
            nodes: vec![node(10, 3, &[])],
            roots: vec![0],
        };
        let layout = layout(&forest, SPACING, &Stacking::default());
        assert_eq!(
            layout.nodes[0],
            Placement {
                x: 0,
                y: 0,
                width: 10,
                height: 3,
                anchor: 5
            }
        );
        assert_eq!((layout.width, layout.height), (10, 3));
    }

    #[test]
    fn parents_are_centered_over_their_children() {
        let layout = layout(&demo_forest(), SPACING, &Stacking::default());
        assert_no_overlap(&layout);
        for (parent, first, last) in [(0, 1, 5), (1, 2, 4), (5, 6, 7)] {
            let n = &layout.nodes;
            let middle = n[first].center().midpoint(n[last].center());
            assert_eq!(n[parent].center(), middle, "parent {parent}");
        }
    }

    #[test]
    fn children_sit_below_their_parent_with_room_for_edges() {
        let layout = layout(&demo_forest(), SPACING, &Stacking::default());
        let n = &layout.nodes;
        assert_eq!(n[1].y, n[0].bottom() + SPACING.level);
        assert_eq!(n[2].y, n[3].y);
        assert_eq!(n[3].x, n[2].x + 30 + SPACING.gap);
        // The roots of the forest share the top row.
        assert_eq!((n[0].y, n[8].y, n[9].y), (0, 0, 0));
    }

    #[test]
    fn subtrees_pack_by_contour_but_trees_do_not_interleave() {
        // A narrow deep subtree next to a wide shallow one: the deep one's
        // child can tuck under the shallow sibling's free rows.
        let forest = Forest {
            nodes: vec![
                node(4, 1, &[1, 2]),
                node(4, 1, &[3]),
                node(20, 1, &[]),
                node(4, 1, &[]),
                node(4, 1, &[]),
            ],
            roots: vec![0, 4],
        };
        let layout = layout(&forest, SPACING, &Stacking::default());
        assert_no_overlap(&layout);
        let n = &layout.nodes;
        assert_eq!(n[2].x, n[1].x + 4 + SPACING.gap);
        // The second tree starts after the first tree's bounding box.
        assert_eq!(n[4].x, n[2].x + 20 + SPACING.forest_gap);
    }

    #[test]
    fn stacked_children_hang_under_the_parent_edge() {
        let stacking = Stacking {
            parents: BTreeSet::from([1]),
            lone_roots: false,
        };
        let layout = layout(&demo_forest(), SPACING, &stacking);
        assert_no_overlap(&layout);
        let n = &layout.nodes;
        for (child, y) in [
            (2, n[1].bottom()),
            (3, n[1].bottom() + 6),
            (4, n[1].bottom() + 12),
        ] {
            assert_eq!(n[child].x, n[1].x + SPACING.indent);
            assert_eq!(n[child].y, y);
        }
        let side_by_side = super::layout(&demo_forest(), SPACING, &Stacking::default());
        assert!(layout.width < side_by_side.width);
        assert!(layout.height > side_by_side.height);
    }

    #[test]
    fn lone_roots_can_form_a_column() {
        let stacking = Stacking {
            parents: BTreeSet::new(),
            lone_roots: true,
        };
        let layout = layout(&demo_forest(), SPACING, &stacking);
        assert_no_overlap(&layout);
        let n = &layout.nodes;
        assert_eq!(n[8].x, n[9].x);
        assert_eq!(n[9].y, n[8].bottom());
    }

    #[test]
    fn fitting_stacks_the_most_profitable_groups_first() {
        let forest = demo_forest();
        let wide = fit(&forest, SPACING, u16::MAX);
        assert_eq!(wide.stacking, Stacking::default());

        let narrower = fit(&forest, SPACING, wide.width - 1);
        assert!(narrower.width < wide.width);
        assert!(narrower.stacking.parents.contains(&1) || narrower.stacking.lone_roots);
        assert_no_overlap(&narrower);

        let narrowest = fit(&forest, SPACING, 1);
        assert_eq!(narrowest.stacking.parents, BTreeSet::from([1, 5]));
        assert!(narrowest.stacking.lone_roots);
        assert_no_overlap(&narrowest);
    }

    #[test]
    fn edges_can_attach_at_the_left_of_a_node() {
        // Dots attach at their glyph, the first column.
        let dot = |width: u16, children: &[usize]| Node {
            width,
            height: 1,
            anchor: 0,
            children: children.to_vec(),
        };
        let forest = Forest {
            nodes: vec![dot(10, &[1, 2]), dot(8, &[]), dot(12, &[])],
            roots: vec![0],
        };
        let layout = layout(&forest, SPACING, &Stacking::default());
        assert_no_overlap(&layout);
        let n = &layout.nodes;
        assert_eq!(n[0].center(), n[1].center().midpoint(n[2].center()));
        assert_eq!(n[1].center(), n[1].x);
    }

    #[test]
    fn layout_is_deterministic() {
        let forest = demo_forest();
        assert_eq!(fit(&forest, SPACING, 100), fit(&forest, SPACING, 100));
    }

    #[test]
    fn many_shapes_never_overlap() {
        // Every forest of up to five nodes in a chain or a fan, in many widths.
        for fan in 1..6_u16 {
            let mut nodes = vec![node(7, 2, &[])];
            for child in 1..=fan {
                let index = nodes.len();
                nodes.push(node(5 + child, 3, &[]));
                nodes[0].children.push(index);
                if child % 2 == 0 {
                    let grandchild = nodes.len();
                    nodes.push(node(9, 1, &[]));
                    nodes[index].children.push(grandchild);
                }
            }
            nodes.push(node(12, 4, &[]));
            let lone = nodes.len() - 1;
            let forest = Forest {
                nodes,
                roots: vec![0, lone],
            };
            for width in [1, 20, 40, 200] {
                assert_no_overlap(&fit(&forest, SPACING, width));
            }
        }
    }
}
