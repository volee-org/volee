//! The top bar: the volee badge, the numbered tabs and the counters.
//!
//! ```text
//!  volee   1 Hierarchy  2 Network  3 Repos  4 Timeline     ! 2 need you · ▸ 6 working · 12 agents
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;

use super::put_spans;
use crate::app::{Model, Tab, counters};
use crate::text;
use crate::theme::StyleRole;

const BRAND: &str = " volee ";

/// Where each tab is drawn, shared by drawing and the mouse.
fn tabs(width: u16) -> Vec<(Tab, u16, String)> {
    let full: u16 = Tab::ALL
        .iter()
        .map(|tab| u16::try_from(tab.title().len()).unwrap_or(0) + 4)
        .sum();
    let short = width < 9 + full + 24;
    let mut x = u16::try_from(BRAND.len()).unwrap_or(0) + 1;
    Tab::ALL
        .iter()
        .zip(1..)
        .map(|(tab, number)| {
            let label = if short {
                format!(" {number} {} ", &tab.title()[..1])
            } else {
                format!(" {number} {} ", tab.title())
            };
            let start = x;
            x += u16::try_from(label.len()).unwrap_or(0) + 1;
            (*tab, start, label)
        })
        .collect()
}

/// The areas of the tabs, for the mouse.
pub(crate) fn tab_areas(model: &Model) -> Vec<(Tab, Rect)> {
    let top = model.layout.top;
    tabs(top.width)
        .into_iter()
        .filter(|(_, x, label)| x + u16::try_from(label.len()).unwrap_or(0) <= top.width)
        .map(|(tab, x, label)| {
            (
                tab,
                Rect::new(top.x + x, top.y, u16::try_from(label.len()).unwrap_or(0), 1),
            )
        })
        .collect()
}

/// Draws the bar.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    if area.is_empty() {
        return;
    }
    let theme = &model.theme;
    buf.set_style(area, theme.style(StyleRole::Topbar));
    buf.set_stringn(
        area.x,
        area.y,
        BRAND,
        usize::from(area.width),
        theme.style(StyleRole::TopbarBrand),
    );
    for (tab, x, label) in tabs(area.width) {
        if x + u16::try_from(label.len()).unwrap_or(0) > area.width {
            break;
        }
        let style = theme.style(if tab == model.tab {
            StyleRole::TabActive
        } else {
            StyleRole::Tab
        });
        buf.set_string(area.x + x, area.y, &label, style);
        // The number stands out, so the key to press is obvious.
        let number = theme.style(StyleRole::TabNumber).patch(Style {
            bg: style.bg,
            ..Style::default()
        });
        buf.set_string(area.x + x + 1, area.y, &label[1..2], number);
    }
    let tabs_end = tabs(area.width).last().map_or(0, |(_, x, label)| {
        x + u16::try_from(label.len()).unwrap_or(0)
    });

    let (needs, working, all) = counters(&model.registry);
    let dim = theme.style(StyleRole::TextDim);
    let needs_style = theme.style(if needs > 0 {
        StyleRole::CounterUrgent
    } else {
        StyleRole::Counter
    });
    let counter = theme.style(StyleRole::Counter);
    let full = vec![
        Span::styled(format!("! {needs} need you"), needs_style),
        Span::styled(" · ", dim),
        Span::styled(format!("▸ {working} working"), counter),
        Span::styled(" · ", dim),
        Span::styled(format!("{all} agents"), counter),
    ];
    let short = vec![
        Span::styled(format!("!{needs}"), needs_style),
        Span::styled(" ", dim),
        Span::styled(format!("▸{working}"), counter),
        Span::styled(" ", dim),
        Span::styled(format!("{all}"), counter),
    ];
    let width_of = |spans: &[Span<'_>]| -> u16 {
        u16::try_from(
            spans
                .iter()
                .map(|span| text::width(&span.content))
                .sum::<usize>(),
        )
        .unwrap_or(0)
    };
    // Once the source stopped, saying so matters more than the banner.
    let (banner, banner_style) = if model.offline.is_some() {
        (Some(" offline ".to_owned()), StyleRole::NoticeError)
    } else {
        (
            model.banner.as_deref().map(|banner| format!("{banner} ")),
            StyleRole::Placeholder,
        )
    };
    let banner_width = banner
        .as_deref()
        .map_or(0, |banner| u16::try_from(text::width(banner)).unwrap_or(0));
    let room = area.width.saturating_sub(tabs_end + 2);
    let (spans, show_banner) = if width_of(&full) + banner_width + 3 <= room {
        (full, true)
    } else if width_of(&full) < room {
        (full, model.offline.is_some())
    } else {
        (short, model.offline.is_some())
    };
    let mut x = area.width;
    if show_banner && let Some(banner) = &banner {
        x = x.saturating_sub(banner_width);
        buf.set_string(area.x + x, area.y, banner, theme.style(banner_style));
        x = x.saturating_sub(2);
    } else {
        x -= 1;
    }
    let counters_width = width_of(&spans);
    if counters_width <= x.saturating_sub(tabs_end) {
        put_spans(
            buf,
            area.x + x - counters_width,
            area.y,
            &spans,
            counters_width,
        );
    }
}
