//! Where each panel goes.
//!
//! Panel sizes depend only on the terminal size and on the view, never on
//! the content or on the selection, so nothing jumps while you move around.
//!
//! ```text
//!  wide (100 columns and 28 rows or more)      narrow, hierarchy           narrow, other views
//! ┌──────────── top bar ────────────────┐     ┌────── top bar ──────┐     ┌────── top bar ──────┐
//! │ agents   │ view (hierarchy...)      │     │ agents  │ detail    │     │ agents  │ view      │
//! │          │                          │     │ (as a   ├───────────┤     │         │ (network, │
//! ├──────────┼──────────────────────────┤     │  tree)  │ preview   │     ├─────────┤  repos,   │
//! │ detail   │ preview                  │     │         │           │     │ detail  │  timeline)│
//! └──────────── key hints ──────────────┘     └────── key hints ────┘     └────── key hints ────┘
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
    /// The key hints.
    pub(crate) hints: Rect,
}

/// The smallest size with room for the view next to the agents.
const WIDE_MIN: (u16, u16) = (100, 28);

impl ScreenLayout {
    /// The layout of a terminal of the given size. On small screens, the
    /// hierarchy fits in the agents list as a tree (`tree_view`); the other
    /// views take the place of the preview.
    pub(crate) fn compute(area: Rect, tree_view: bool) -> Self {
        let top = Rect::new(area.x, area.y, area.width, 1.min(area.height));
        let hints = Rect::new(
            area.x,
            area.bottom().saturating_sub(1),
            area.width,
            1.min(area.height.saturating_sub(1)),
        );
        let main = Rect::new(
            area.x,
            area.y + top.height,
            area.width,
            area.height.saturating_sub(top.height + hints.height),
        );
        if area.width >= WIDE_MIN.0 && area.height >= WIDE_MIN.1 {
            let left = (area.width * 2 / 7).clamp(30, 40);
            let bottom = (main.height * 3 / 10).clamp(12, 16);
            let upper = main.height - bottom;
            let right = area.width - left;
            Self {
                mode: Mode::Wide,
                top,
                rail: Rect::new(main.x, main.y, left, upper),
                view: Some(Rect::new(main.x + left, main.y, right, upper)),
                detail: Rect::new(main.x, main.y + upper, left, bottom),
                preview: Some(Rect::new(main.x + left, main.y + upper, right, bottom)),
                hints,
            }
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
                hints,
            }
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
        for (width, height) in [(80, 24), (100, 28), (120, 40), (200, 50), (99, 60)] {
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
    }

    #[test]
    fn small_terminals_give_other_views_the_right_side() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 80, 24), false);
        assert_eq!(layout.mode, Mode::Split);
        assert_eq!(layout.view, Some(Rect::new(30, 1, 50, 22)));
        assert_eq!(layout.rail, Rect::new(0, 1, 30, 14));
        assert_eq!(layout.detail, Rect::new(0, 15, 30, 8));
        assert_eq!(layout.preview, None);
    }

    #[test]
    fn wide_terminals_give_the_view_most_of_the_room() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 120, 40), true);
        assert_eq!(
            layout,
            ScreenLayout::compute(Rect::new(0, 0, 120, 40), false)
        );
        assert_eq!(layout.mode, Mode::Wide);
        assert_eq!(layout.rail, Rect::new(0, 1, 34, 26));
        assert_eq!(layout.view, Some(Rect::new(34, 1, 86, 26)));
        assert_eq!(layout.detail, Rect::new(0, 27, 34, 12));
        let layout = ScreenLayout::compute(Rect::new(0, 0, 200, 50), false);
        assert_eq!(layout.view, Some(Rect::new(40, 1, 160, 34)));
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
