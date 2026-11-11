//! The top bar: the volee badge, the numbered tabs and the counters:
//! questions waiting for the person (`?`) and unread messages (`✉`) when
//! there are any, the agents that need you and those at work, then the
//! agents by role and the pull requests open.
//!
//! ```text
//!  volee   1 Tree  2 Network  3 Repos  4 Timeline   ? 1 · ✉ 2 · ! 2 need you · ▸ 6 working · lead 1 · coordinators 2 · workers 5 · subagents 2 · on their own 2 · PR 1
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

/// The counters, in full and in short: questions and unread messages
/// when there are any, then the agents.
fn counter_spans(model: &Model) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let theme = &model.theme;
    let (needs, working, all) = counters(&model.registry);
    let dim = theme.style(StyleRole::TextDim);
    let needs_style = theme.style(if needs > 0 {
        StyleRole::CounterUrgent
    } else {
        StyleRole::Counter
    });
    let counter = theme.style(StyleRole::Counter);
    let messages = crate::inbox::counts(&model.registry);
    let mut full = Vec::new();
    let mut short = Vec::new();
    let urgent = theme.style(StyleRole::CounterUrgent);
    for (count, glyph, style) in [
        (messages.questions, "?", urgent),
        (messages.unread, "✉", counter),
    ] {
        if count > 0 {
            full.push(Span::styled(format!("{glyph} {count}"), style));
            full.push(Span::styled(" · ", dim));
            short.push(Span::styled(format!("{glyph}{count}"), style));
            short.push(Span::styled(" ", dim));
        }
    }
    full.extend([
        Span::styled(
            format!(
                "! {needs} {}",
                if needs == 1 { "needs you" } else { "need you" }
            ),
            needs_style,
        ),
        Span::styled(" · ", dim),
        Span::styled(format!("▸ {working} working"), counter),
    ]);
    // The agents by role, as the tree shows them, then the pull requests
    // open: what a glance at the cockpit tells.
    let roles = by_role(&model.registry);
    let prs = model
        .registry
        .agents()
        .filter(|agent| {
            agent.delivery.pull_request.as_ref().is_some_and(|pr| {
                matches!(
                    pr.state,
                    volee_core::delivery::PrState::Open | volee_core::delivery::PrState::Draft
                )
            })
        })
        .count();
    for (count, one, many) in [
        (roles[0], "lead", "leads"),
        (roles[1], "coordinator", "coordinators"),
        (roles[2], "worker", "workers"),
        (roles[3], "subagent", "subagents"),
        (roles[4], "on its own", "on their own"),
        (prs, "PR", "PR"),
    ] {
        if count > 0 {
            full.push(Span::styled(" · ", dim));
            let word = if count == 1 { one } else { many };
            let text = if word == "PR" {
                format!("PR {count}")
            } else {
                format!("{word} {count}")
            };
            full.push(Span::styled(text, counter));
        }
    }
    if roles.iter().all(|count| *count == 0) {
        full.push(Span::styled(" · ", dim));
        full.push(Span::styled(
            format!("{all} {}", if all == 1 { "agent" } else { "agents" }),
            counter,
        ));
    }
    short.extend([
        Span::styled(format!("!{needs}"), needs_style),
        Span::styled(" ", dim),
        Span::styled(format!("▸{working}"), counter),
        Span::styled(" ", dim),
        Span::styled(format!("{all}"), counter),
    ]);
    (full, short)
}

/// How many agents have each role: leads, coordinators, workers, subagents
/// and agents on their own.
fn by_role(registry: &volee_core::Registry) -> [usize; 5] {
    let mut counts = [0; 5];
    for agent in registry.agents() {
        let index = match agent.role {
            volee_core::role::Role::Lead => 0,
            volee_core::role::Role::Coordinator => 1,
            volee_core::role::Role::Worker => 2,
            volee_core::role::Role::Subagent => 3,
            volee_core::role::Role::Independent => 4,
        };
        counts[index] += 1;
    }
    counts
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

    let (full, short) = counter_spans(model);
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
