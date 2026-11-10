//! Which-key popups at the bottom right of the screen (ADR 0025): the keys
//! of the current screen (`?`), and what can follow a sequence typed so far
//! (the leader, `g`). Both are generated from the keymap.
//!
//! ```text
//!                  ╭ Agents · ? all keys · esc ───────────────╮
//!                  │ h   parent      m   message    ␣   more…  │
//!                  │ j   next        d   diff       ?   keys   │
//!                  │ gg  first       a   take over  q   quit   │
//!                  ╰──────────────────────────────────────────╯
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::{panel, put_spans};
use crate::app::Model;
use crate::keymap::{Action, Context, KeyCombo, KeySeq, group_name, join_keys};
use crate::text;
use crate::theme::{StyleRole, Theme};

/// One entry of a popup: keys and what they do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The keys, as shown: `gg`, `␣`, `ctrl-d`.
    pub(crate) keys: String,
    /// What they do, in a word or two.
    pub(crate) label: String,
    /// Whether they open a group of longer sequences.
    pub(crate) group: bool,
}

/// The longest label shown before it is cut.
const LABEL: usize = 16;

/// The most rows of a popup before it takes another column.
const ROWS: usize = 14;

/// Actions shown as one entry in the keys popup, with one label, so that it
/// stays small: `h/j/k/l move`, `gg/G first/last`, `1-4 views`.
const PAIRS: &[(&[Action], &str)] = &[
    (
        &[Action::Left, Action::Down, Action::Up, Action::Right],
        "move",
    ),
    (&[Action::First, Action::Last], "first/last"),
    (&[Action::HalfPageDown, Action::HalfPageUp], "half page"),
    (&[Action::ZoomIn, Action::ZoomOut], "zoom"),
    (&[Action::PreviousEvent, Action::NextEvent], "events"),
    (
        &[Action::NextMatch, Action::PreviousMatch],
        "next/prev match",
    ),
    (
        &[
            Action::ShowHierarchy,
            Action::ShowNetwork,
            Action::ShowRepos,
            Action::ShowTimeline,
        ],
        "views",
    ),
    (&[Action::PreviousView, Action::NextView], "prev/next view"),
    (&[Action::NextPane, Action::PreviousPane], "panels"),
];

/// The entries of the current screen's popup: the focused panel's keys,
/// then the keys of everywhere, one per action or per pair of actions,
/// sequences behind the leader folded into one entry.
pub(crate) fn screen_entries(model: &Model) -> Vec<Entry> {
    let contexts = model.screen_contexts();
    let leader = KeyCombo::char(' ');
    // The first key that reaches each action from here, in table order: one
    // that a more specific context does not take, and not behind the leader.
    let mut keyed: Vec<(Action, String, &'static str)> = Vec::new();
    let mut behind_leader = false;
    for context in &contexts {
        for binding in model.keymap.bindings() {
            if binding.context != *context
                || keyed.iter().any(|(action, ..)| *action == binding.action)
            {
                continue;
            }
            match binding.keys.iter().find(|seq| {
                seq.keys().first() != Some(&leader)
                    && seq.single().is_none_or(|key| {
                        model.keymap.action(&contexts, key) == Some(binding.action)
                    })
            }) {
                Some(seq) => keyed.push((binding.action, seq.short(), binding.label)),
                None => {
                    behind_leader |= binding
                        .keys
                        .iter()
                        .any(|seq| seq.keys().first() == Some(&leader));
                }
            }
        }
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut done: Vec<Action> = Vec::new();
    for (action, keys, label) in &keyed {
        if done.contains(action) {
            continue;
        }
        let pair = PAIRS
            .iter()
            .find(|(actions, _)| actions.contains(action))
            .map(|(actions, label)| {
                let members: Vec<&(Action, String, &str)> = actions
                    .iter()
                    .filter_map(|member| keyed.iter().find(|(a, ..)| a == member))
                    .collect();
                (members, *label)
            })
            .filter(|(members, _)| members.len() > 1);
        if let Some((members, pair_label)) = pair {
            let keys: Vec<String> = members.iter().map(|(_, keys, _)| keys.clone()).collect();
            done.extend(members.iter().map(|(action, ..)| *action));
            entries.push(Entry {
                keys: join_keys(&keys),
                label: pair_label.to_owned(),
                group: false,
            });
        } else {
            done.push(*action);
            entries.push(Entry {
                keys: keys.replace("shift-", "⇧"),
                label: (*label).to_owned(),
                group: false,
            });
        }
    }
    if behind_leader {
        // Before the last entries (`?` keys, esc, quit), where it is seen.
        let at = entries.len().saturating_sub(3);
        entries.insert(
            at,
            Entry {
                keys: leader.short(),
                label: format!("{}…", group_name(&[leader])),
                group: true,
            },
        );
    }
    entries
}

/// The entries that continue the sequence typed so far.
pub(crate) fn pending_entries(model: &Model) -> Vec<Entry> {
    model
        .keymap
        .continuations(&model.contexts(), &model.pending_keys)
        .into_iter()
        .map(|next| Entry {
            keys: next.key.short(),
            label: if next.group {
                format!("{}…", next.label)
            } else {
                next.label.to_owned()
            },
            group: next.group,
        })
        .collect()
}

/// The title of the popup of the current screen: the focused panel's name.
fn screen_title(model: &Model) -> &'static str {
    model
        .screen_contexts()
        .first()
        .map_or("Keys", |context| match context {
            Context::Agents => "Agents",
            other => other.title(),
        })
}

/// Draws the popup of the current screen's keys.
pub(crate) fn render_screen(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let all = model
        .keymap
        .first_key(Context::Keys, Action::AllKeys)
        .map(KeySeq::short);
    let mut title = vec![Span::raw(screen_title(model))];
    if let Some(all) = all {
        title.push(Span::styled(
            format!(" · {all} all keys · esc"),
            theme.style(StyleRole::TextDim),
        ));
    }
    render_box(theme, title, &screen_entries(model), area, buf);
}

/// Draws what can follow the keys typed so far.
pub(crate) fn render_pending(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let typed: String = model
        .pending_keys
        .iter()
        .map(|key| key.short())
        .collect::<Vec<_>>()
        .join(" ");
    let title = vec![
        Span::raw(typed),
        Span::styled(
            format!(" {} · esc", group_name(&model.pending_keys)),
            theme.style(StyleRole::TextDim),
        ),
    ];
    render_box(theme, title, &pending_entries(model), area, buf);
}

/// Draws entries in columns, in a bordered box at the bottom right of
/// `area`, above its last line (the hint bar). As many columns as fit; the
/// box never covers more than the area.
pub(crate) fn render_box(
    theme: &Theme,
    title: Vec<Span<'_>>,
    entries: &[Entry],
    area: Rect,
    buf: &mut Buffer,
) {
    let area = area.intersection(buf.area);
    if entries.is_empty() || area.width < 12 || area.height < 4 {
        return;
    }
    let keys_width = entries
        .iter()
        .map(|entry| text::width(&entry.keys))
        .max()
        .unwrap_or(1);
    let label_width = entries
        .iter()
        .map(|entry| text::width(&entry.label).min(LABEL))
        .max()
        .unwrap_or(1);
    // `keys`, two spaces, `label`, and three spaces to the next column.
    let column = keys_width + 2 + label_width;
    let gap = 3;
    let room = usize::from(area.width.saturating_sub(4));
    let most_rows = usize::from(area.height.saturating_sub(3)).max(1);
    let fit = ((room + gap) / (column + gap)).max(1);
    // The fewest columns that keep every entry in sight, else all that fit.
    let columns = (1..=fit)
        .find(|columns| entries.len().div_ceil(*columns) <= most_rows.min(ROWS))
        .unwrap_or(fit);
    let rows = entries.len().div_ceil(columns).min(most_rows);
    let title_width = title
        .iter()
        .map(|span| text::width(&span.content))
        .sum::<usize>()
        + 4;
    let inner_width = (columns * column + (columns - 1) * gap).max(title_width);
    let width = u16::try_from(inner_width + 4)
        .unwrap_or(u16::MAX)
        .min(area.width);
    let height = u16::try_from(rows + 2)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(1));
    let window = Rect::new(
        area.right().saturating_sub(width + 1),
        area.bottom().saturating_sub(height + 1),
        width,
        height,
    );
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Background));
    let inner = panel(buf, window, title, true, theme);
    let visible = usize::from(inner.height);
    for (index, entry) in entries.iter().enumerate() {
        let (column_index, row) = (index / rows.max(1), index % rows.max(1));
        if row >= visible || column_index >= columns {
            continue;
        }
        let x = inner.x + 1 + u16::try_from(column_index * (column + gap)).unwrap_or(u16::MAX);
        let y = inner.y + u16::try_from(row).unwrap_or(u16::MAX);
        if x >= inner.right() {
            continue;
        }
        let padding = keys_width.saturating_sub(text::width(&entry.keys)) + 2;
        let label: String = entry.label.chars().take(LABEL).collect();
        let label_style = theme.style(if entry.group {
            StyleRole::Title
        } else {
            StyleRole::Text
        });
        let spans = [
            Span::styled(entry.keys.clone(), theme.style(StyleRole::HintKey)),
            Span::raw(" ".repeat(padding)),
            Span::styled(label, label_style),
        ];
        put_spans(buf, x, y, &spans, inner.right().saturating_sub(x));
    }
    if entries.len() > rows * columns {
        buf.set_string(
            window.right().saturating_sub(3),
            window.bottom().saturating_sub(1),
            "…",
            theme.style(StyleRole::TextDim),
        );
    }
}
