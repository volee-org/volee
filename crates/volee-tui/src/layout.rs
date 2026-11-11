//! Where each panel goes.
//!
//! Panel sizes depend only on the terminal size and on the view, never on
//! the content or on the selection, so nothing jumps while you move around.
//!
//! ```text
//!  wide (100 columns and 28 rows or more; the right column from 160)
//! ┌──────────────────────── top bar ─────────────────────────┐
//! │ agents   │ view (tree, network...)        │ for you      │
//! │          │                                │              │
//! ├──────────┤                                ├──────────────┤
//! │ detail   │                                │ journal      │
//! ├──────────┴────────────────────────────────┴──────────────┤
//! │ preview: the selected agent, its state and its actions,  │
//! │ then its screen                                          │
//! └──────────────────────── key hints ───────────────────────┘
//!
//!  narrow, tree               narrow, other views
//! ┌────── top bar ──────┐     ┌────── top bar ──────┐
//! │ agents  │ detail    │     │ agents  │ view      │
//! │ (as a   ├───────────┤     │         │ (network, │
//! │  tree)  │ preview   │     ├─────────┤  repos,   │
//! │         │           │     │ detail  │  timeline)│
//! └────── key hints ────┘     └────── key hints ────┘
//! ```

use ratatui::layout::Rect;

/// The arrangements of the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Agents, view, detail and preview.
    #[default]
    Wide,
    /// Agents as a tree (the hierarchy), detail and preview.
    Narrow,
    /// Agents and detail on the left, the view on the right: the network,
    /// repositories and timeline views on small screens.
    Split,
}

/// The rectangles of every panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScreenLayout {
    /// Which arrangement.
    pub(crate) mode: Mode,
    /// The top bar.
    pub(crate) top: Rect,
    /// The list of agents.
    pub(crate) rail: Rect,
    /// The current view, absent in narrow mode.
    pub(crate) view: Option<Rect>,
    /// The facts about the selected agent.
    pub(crate) detail: Rect,
    /// The selected agent's screen, absent when there is no room.
    pub(crate) preview: Option<Rect>,
    /// What waits for the person, on wide screens.
    pub(crate) for_you: Option<Rect>,
    /// What happened lately, under it.
    pub(crate) journal: Option<Rect>,
    /// The key hints.
    pub(crate) hints: Rect,
}

/// The fewest rows that get two rows of key hints.
const TWO_HINT_ROWS: u16 = 20;

/// The smallest size with room for the view next to the agents.
const WIDE_MIN: (u16, u16) = (100, 28);

/// The fewest columns that get the column of what waits for you.
const RIGHT_COLUMN_MIN: u16 = 160;

/// The width of the agents' column and of the right column, when both show:
/// at 190 columns, the tree keeps 130, the room of five workers' cards side
/// by side.
const SIDE_COLUMN: (u16, u16) = (30, 28);

impl ScreenLayout {
    /// The layout of a terminal of the given size. On small screens, the
    /// hierarchy fits in the agents list as a tree (`tree_view`); the other
    /// views take the place of the preview.
    pub(crate) fn compute(area: Rect, tree_view: bool) -> Self {
        let top = Rect::new(area.x, area.y, area.width, 1.min(area.height));
        // Two rows of hints when the screen is tall enough, else one.
        let rows = if area.height >= TWO_HINT_ROWS { 2 } else { 1 };
        let hints = Rect::new(
            area.x,
            area.bottom().saturating_sub(rows),
            area.width,
            rows.min(area.height.saturating_sub(1)),
        );
        let main = Rect::new(
            area.x,
            area.y + top.height,
            area.width,
            area.height.saturating_sub(top.height + hints.height),
        );
        if area.width >= WIDE_MIN.0 && area.height >= WIDE_MIN.1 {
            Self::wide(main, top, hints)
        } else if !tree_view {
            let left = (area.width * 3 / 8).clamp(24.min(area.width), 34);
            let right = area.width.saturating_sub(left);
            let detail_height = (main.height * 2 / 5).clamp(6, 10).min(main.height / 2);
            let rail_height = main.height - detail_height;
            Self {
                mode: Mode::Split,
                top,
                rail: Rect::new(main.x, main.y, left, rail_height),
                view: Some(Rect::new(main.x + left, main.y, right, main.height)),
                detail: Rect::new(main.x, main.y + rail_height, left, detail_height),
                preview: None,
                for_you: None,
                journal: None,
                hints,
            }
        } else {
            let left = (area.width * 45 / 100).clamp(24.min(area.width), 44);
            let right = area.width.saturating_sub(left);
            let detail_height = main.height.min(12);
            let preview_height = main.height - detail_height;
            Self {
                mode: Mode::Narrow,
                top,
                rail: Rect::new(main.x, main.y, left, main.height),
                view: None,
                detail: Rect::new(main.x + left, main.y, right, detail_height),
                preview: (preview_height >= 5).then(|| {
                    Rect::new(main.x + left, main.y + detail_height, right, preview_height)
                }),
                for_you: None,
                journal: None,
                hints,
            }
        }
    }

    /// The wide arrangement: the agents and the detail of the selected one
    /// in a column at the left, the view in the middle, what waits for you
    /// and the journal in a column at the right when there is room, and the
    /// selected agent's screen across the bottom.
    fn wide(main: Rect, top: Rect, hints: Rect) -> Self {
        let bottom = (main.height * 3 / 10).clamp(10, 14);
        let upper = main.height - bottom;
        let right_column = main.width >= RIGHT_COLUMN_MIN;
        let (left, right) = if right_column {
            SIDE_COLUMN
        } else {
            ((main.width * 2 / 7).clamp(28, 36), 0)
        };
        let view = main.width - left - right;
        let detail = (upper * 2 / 5).clamp(8, 14);
        let rail = upper - detail;
        let for_you = upper / 2;
        Self {
            mode: Mode::Wide,
            top,
            rail: Rect::new(main.x, main.y, left, rail),
            view: Some(Rect::new(main.x + left, main.y, view, upper)),
            detail: Rect::new(main.x, main.y + rail, left, detail),
            preview: Some(Rect::new(main.x, main.y + upper, main.width, bottom)),
            for_you: right_column.then(|| Rect::new(main.x + left + view, main.y, right, for_you)),
            journal: right_column.then(|| {
                Rect::new(
                    main.x + left + view,
                    main.y + for_you,
                    right,
                    upper - for_you,
                )
            }),
            hints,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covers_without_overlap(layout: &ScreenLayout, area: Rect) {
        let mut panels = vec![layout.top, layout.rail, layout.detail, layout.hints];
        panels.extend(layout.view);
        panels.extend(layout.preview);
        panels.extend(layout.for_you);
        panels.extend(layout.journal);
        let total: u32 = panels.iter().map(|r| r.area()).sum();
        assert_eq!(total, area.area(), "{layout:?}");
        for (i, a) in panels.iter().enumerate() {
            for b in &panels[i + 1..] {
                assert!(!a.intersects(*b), "{a:?} and {b:?}");
            }
        }
    }

    #[test]
    fn common_sizes_tile_the_screen() {
        for (width, height) in [
            (80, 24),
            (100, 28),
            (120, 40),
            (159, 40),
            (160, 40),
            (190, 50),
            (200, 50),
            (99, 60),
        ] {
            let area = Rect::new(0, 0, width, height);
            for tree_view in [true, false] {
                covers_without_overlap(&ScreenLayout::compute(area, tree_view), area);
            }
        }
    }

    #[test]
    fn small_terminals_show_the_hierarchy_in_the_list() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 80, 24), true);
        assert_eq!(layout.mode, Mode::Narrow);
        assert_eq!(layout.view, None);
        assert_eq!(layout.rail.width, 36);
        assert!(layout.preview.is_some());
        assert_eq!(layout.for_you, None);
    }

    #[test]
    fn small_terminals_give_other_views_the_right_side() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 80, 24), false);
        assert_eq!(layout.mode, Mode::Split);
        assert_eq!(layout.view, Some(Rect::new(30, 1, 50, 21)));
        assert_eq!(layout.rail, Rect::new(0, 1, 30, 13));
        assert_eq!(layout.detail, Rect::new(0, 14, 30, 8));
        assert_eq!(layout.preview, None);
    }

    #[test]
    fn wide_terminals_keep_the_detail_under_the_agents_and_the_screen_below() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 120, 40), true);
        assert_eq!(
            layout,
            ScreenLayout::compute(Rect::new(0, 0, 120, 40), false)
        );
        assert_eq!(layout.mode, Mode::Wide);
        assert_eq!(layout.rail, Rect::new(0, 1, 34, 16));
        assert_eq!(layout.detail, Rect::new(0, 17, 34, 10));
        assert_eq!(layout.view, Some(Rect::new(34, 1, 86, 26)));
        assert_eq!(layout.preview, Some(Rect::new(0, 27, 120, 11)));
        assert_eq!(layout.for_you, None, "no room for the right column");
    }

    #[test]
    fn the_right_column_leaves_the_tree_the_room_of_five_cards() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 190, 50), true);
        let view = layout.view.unwrap();
        // 130 columns inside the border: five cards of 24 and their gaps.
        assert_eq!(view.width, 132);
        assert_eq!(layout.for_you, Some(Rect::new(162, 1, 28, 16)));
        assert_eq!(layout.journal, Some(Rect::new(162, 17, 28, 17)));
        assert_eq!(layout.preview, Some(Rect::new(0, 34, 190, 14)));
        assert_eq!(view.height, 33);
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        for (width, height) in [(0, 0), (1, 1), (10, 3), (30, 8), (60, 12)] {
            for tree_view in [true, false] {
                let _ = ScreenLayout::compute(Rect::new(0, 0, width, height), tree_view);
            }
        }
    }
}
