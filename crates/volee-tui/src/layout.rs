//! Where each panel goes.
//!
//! Panel sizes depend only on the terminal size, on the view and on what
//! the person chose of the panels ([`Panels`]), never on the content or on
//! the selection, so nothing jumps while you move around.
//!
//! ```text
//!  wide (100 columns and 28 rows or more; the right column from 160)
//! ┌──────────────────────── top bar ─────────────────────────┐
//! │ agents   │ view (tree, network...)        │ for you      │
//! │          │                                │              │
//! ├──────────┤                                ├──────────────┤
//! │ detail   │                                │ journal      │
//! ├──────────┴────────────────────────────────┴──────────────┤
//! │ preview, when shown: the selected agent, its state and   │
//! │ its actions, then its screen                             │
//! └──────────────────────── status line ─────────────────────┘
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

/// What the person chose of the panels: whether the selected agent's screen
/// shows, and how far they moved the borders (`Ctrl` and the arrows, as
/// LazyVim resizes its windows).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Panels {
    /// Whether the preview shows: hidden until asked for (`Space u p`).
    pub(crate) preview: bool,
    /// Columns added to the agents' column, or taken from it.
    pub(crate) list: i16,
    /// Rows added to the preview, or taken from it; to the detail when the
    /// preview is hidden.
    pub(crate) lower: i16,
}

impl Panels {
    /// The panels with the preview shown, borders where they start.
    #[cfg(test)]
    pub(crate) fn with_preview() -> Self {
        Self {
            preview: true,
            ..Self::default()
        }
    }
}

/// `base` moved by `delta`, kept between `least` and `most`.
fn moved(base: u16, delta: i16, least: u16, most: u16) -> u16 {
    let most = most.max(least);
    u16::try_from((i32::from(base) + i32::from(delta)).clamp(i32::from(least), i32::from(most)))
        .unwrap_or(least)
}

/// The smallest size with room for the view next to the agents.
const WIDE_MIN: (u16, u16) = (100, 28);

/// The fewest columns that get the column of what waits for you.
const RIGHT_COLUMN_MIN: u16 = 160;

/// The width of the agents' column and of the right column, when both show.
/// The agents' column keeps the width it has without the right column: a
/// name of 26 letters under its parent, such as the coordinator of a
/// mission, shows whole there. At 190 columns, the tree keeps 130, the room
/// of five workers' cards side by side.
const SIDE_COLUMN: (u16, u16) = (36, 24);

impl ScreenLayout {
    /// The layout of a terminal of the given size. On small screens, the
    /// hierarchy fits in the agents list as a tree (`tree_view`); the other
    /// views take the place of the preview.
    pub(crate) fn compute(area: Rect, tree_view: bool, panels: Panels) -> Self {
        let top = Rect::new(area.x, area.y, area.width, 1.min(area.height));
        // One status line: the mode and the keys of the moment.
        let rows = 1;
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
            Self::wide(main, top, hints, panels)
        } else if !tree_view {
            let left = (area.width * 3 / 8).clamp(24.min(area.width), 34);
            let left = moved(
                left,
                panels.list,
                20.min(area.width),
                area.width.saturating_sub(30),
            );
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
            let left = moved(
                left,
                panels.list,
                20.min(area.width),
                area.width.saturating_sub(30),
            );
            let right = area.width.saturating_sub(left);
            let detail_height = if panels.preview {
                moved(
                    main.height.min(12),
                    -panels.lower,
                    4,
                    main.height.saturating_sub(5),
                )
                .min(main.height)
            } else {
                main.height
            };
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
    fn wide(main: Rect, top: Rect, hints: Rect, panels: Panels) -> Self {
        let bottom = if panels.preview {
            moved(
                (main.height * 3 / 10).clamp(10, 14),
                panels.lower,
                5,
                main.height.saturating_sub(12),
            )
        } else {
            0
        };
        let upper = main.height - bottom;
        let right_column = main.width >= RIGHT_COLUMN_MIN;
        let (left, right) = if right_column {
            SIDE_COLUMN
        } else {
            ((main.width * 2 / 7).clamp(28, 36), 0)
        };
        let left = moved(left, panels.list, 20, main.width.saturating_sub(right + 40));
        let view = main.width - left - right;
        let detail = (upper * 2 / 5).clamp(8, 14);
        let detail = if panels.preview {
            detail
        } else {
            moved(detail, panels.lower, 5, upper.saturating_sub(6))
        };
        let rail = upper - detail;
        let for_you = upper / 2;
        Self {
            mode: Mode::Wide,
            top,
            rail: Rect::new(main.x, main.y, left, rail),
            view: Some(Rect::new(main.x + left, main.y, view, upper)),
            detail: Rect::new(main.x, main.y + rail, left, detail),
            preview: panels
                .preview
                .then(|| Rect::new(main.x, main.y + upper, main.width, bottom)),
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
                covers_without_overlap(
                    &ScreenLayout::compute(area, tree_view, Panels::with_preview()),
                    area,
                );
            }
        }
    }

    #[test]
    fn small_terminals_show_the_hierarchy_in_the_list() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 80, 24), true, Panels::with_preview());
        assert_eq!(layout.mode, Mode::Narrow);
        assert_eq!(layout.view, None);
        assert_eq!(layout.rail.width, 36);
        assert!(layout.preview.is_some());
        assert_eq!(layout.for_you, None);
    }

    #[test]
    fn small_terminals_give_other_views_the_right_side() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 80, 24), false, Panels::with_preview());
        assert_eq!(layout.mode, Mode::Split);
        assert_eq!(layout.view, Some(Rect::new(30, 1, 50, 22)));
        assert_eq!(layout.rail, Rect::new(0, 1, 30, 14));
        assert_eq!(layout.detail, Rect::new(0, 15, 30, 8));
        assert_eq!(layout.preview, None);
    }

    #[test]
    fn wide_terminals_keep_the_detail_under_the_agents_and_the_screen_below() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 120, 40), true, Panels::with_preview());
        assert_eq!(
            layout,
            ScreenLayout::compute(Rect::new(0, 0, 120, 40), false, Panels::with_preview())
        );
        assert_eq!(layout.mode, Mode::Wide);
        assert_eq!(layout.rail, Rect::new(0, 1, 34, 17));
        assert_eq!(layout.detail, Rect::new(0, 18, 34, 10));
        assert_eq!(layout.view, Some(Rect::new(34, 1, 86, 27)));
        assert_eq!(layout.preview, Some(Rect::new(0, 28, 120, 11)));
        assert_eq!(layout.for_you, None, "no room for the right column");
    }

    #[test]
    fn the_right_column_leaves_the_tree_the_room_of_five_cards() {
        let layout = ScreenLayout::compute(Rect::new(0, 0, 190, 50), true, Panels::with_preview());
        let view = layout.view.unwrap();
        // 128 columns inside the border: five cards of 24 and their gaps.
        assert_eq!(view.width, 130);
        assert_eq!(
            layout.rail.width, 36,
            "the width of the list without the right column"
        );
        assert_eq!(layout.for_you, Some(Rect::new(166, 1, 24, 17)));
        assert_eq!(layout.journal, Some(Rect::new(166, 18, 24, 17)));
        assert_eq!(layout.preview, Some(Rect::new(0, 35, 190, 14)));
        assert_eq!(view.height, 34);
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        for (width, height) in [(0, 0), (1, 1), (10, 3), (30, 8), (60, 12)] {
            for tree_view in [true, false] {
                let _ = ScreenLayout::compute(
                    Rect::new(0, 0, width, height),
                    tree_view,
                    Panels::with_preview(),
                );
            }
        }
    }

    #[test]
    fn the_preview_hides_until_asked_for_and_the_view_takes_its_rows() {
        let area = Rect::new(0, 0, 190, 50);
        let hidden = ScreenLayout::compute(area, true, Panels::default());
        assert_eq!(hidden.preview, None);
        let shown = ScreenLayout::compute(area, true, Panels::with_preview());
        let preview = shown.preview.unwrap();
        // The view goes down to the status line instead.
        assert_eq!(
            hidden.view.unwrap().height,
            shown.view.unwrap().height + preview.height
        );
        covers_without_overlap(&hidden, area);
    }

    #[test]
    fn the_borders_move_by_the_cells_asked_and_stop_short() {
        let area = Rect::new(0, 0, 190, 50);
        let start = ScreenLayout::compute(area, true, Panels::with_preview());
        let moved = ScreenLayout::compute(
            area,
            true,
            Panels {
                preview: true,
                list: 6,
                lower: 4,
            },
        );
        assert_eq!(moved.rail.width, start.rail.width + 6);
        assert_eq!(moved.view.unwrap().width, start.view.unwrap().width - 6);
        assert_eq!(
            moved.preview.unwrap().height,
            start.preview.unwrap().height + 4
        );
        covers_without_overlap(&moved, area);
        // However far, every panel keeps room.
        let far = ScreenLayout::compute(
            area,
            true,
            Panels {
                preview: true,
                list: 500,
                lower: 500,
            },
        );
        assert!(far.view.unwrap().width >= 40, "{:?}", far.view);
        assert!(far.preview.unwrap().height <= 50 - 12);
        covers_without_overlap(&far, area);
        let narrow = ScreenLayout::compute(
            area,
            true,
            Panels {
                preview: false,
                list: -500,
                lower: -500,
            },
        );
        assert_eq!(narrow.rail.width, 20);
        covers_without_overlap(&narrow, area);
    }
}
