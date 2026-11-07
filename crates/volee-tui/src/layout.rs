//! Where each panel goes.
//!
//! Panel sizes depend only on the terminal size, never on the content or on
//! the selection, so nothing jumps while you move around.
//!
//! ```text
//!  wide (100 columns and 28 rows or more)      narrow (below)
//! ┌──────────── top bar ────────────────┐     ┌──────── top bar ────────┐
//! │ agents   │ view (hierarchy...)      │     │ agents   │ detail       │
//! │          │                          │     │ (as a    ├──────────────┤
//! ├──────────┼──────────────────────────┤     │  tree)   │ preview      │
//! │ detail   │ preview                  │     │          │              │
//! └──────────── key hints ──────────────┘     └──────── key hints ──────┘
//! ```

use ratatui::layout::Rect;

/// The two arrangements of the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Agents, view, detail and preview.
    #[default]
    Wide,
    /// Agents (as a tree on the hierarchy tab), detail and preview.
    Narrow,
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
    /// The layout of a terminal of the given size.
    pub(crate) fn compute(area: Rect) -> Self {
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
            covers_without_overlap(&ScreenLayout::compute(area), area);
        }
    }

    #[test]
    fn small_terminals_drop_the_view() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 80, 24));
        assert_eq!(layout.mode, Mode::Narrow);
        assert_eq!(layout.view, None);
        assert_eq!(layout.rail.width, 36);
        assert!(layout.preview.is_some());
    }

    #[test]
    fn wide_terminals_give_the_view_most_of_the_room() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 120, 40));
        assert_eq!(layout.mode, Mode::Wide);
        assert_eq!(layout.rail, Rect::new(0, 1, 34, 26));
        assert_eq!(layout.view, Some(Rect::new(34, 1, 86, 26)));
        assert_eq!(layout.detail, Rect::new(0, 27, 34, 12));
        let layout = ScreenLayout::compute(Rect::new(0, 0, 200, 50));
        assert_eq!(layout.view, Some(Rect::new(40, 1, 160, 34)));
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        for (width, height) in [(0, 0), (1, 1), (10, 3), (30, 8)] {
            let _ = ScreenLayout::compute(Rect::new(0, 0, width, height));
        }
    }
}
