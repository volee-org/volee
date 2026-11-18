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
//!
//! Both are modal windows in the manner of the cockpit's keyboard help: the
//! screen behind fades, a thick lavender frame, each section's name in
//! capitals, each key drawn as a key of a keyboard.
//!
//! ```text
//! ┏ Leader ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
//! ┃                                                                       ┃
//! ┃  AGENTS               WINDOWS                UI              OTHER     ┃
//!   a n  new agent     w w  wall          u p  preview  g d  diff  n  messages
//!   a m  new mission   w m  full screen   u C  theme…              o  open with…
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;

use super::{modal, modal_fitted, put_spans};
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

/// A column of the leader's panel: a group of keys under its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Column {
    /// The group's name, `Agents` or `Windows`, or `Other` for the keys that
    /// act at once.
    pub(crate) title: String,
    /// Its keys, spelled from the keys typed so far: `a n`.
    pub(crate) entries: Vec<Entry>,
}

/// The longest label of the leader's panel before it is cut.
const PANEL_LABEL: usize = 20;

/// Columns between a keys window's edges and its text: the frame and two
/// spaces.
const PANEL_PAD: usize = 3;

/// The width of the keys of the screen in the middle of the screen, as the
/// cockpit's keyboard help.
const MODAL_WIDTH: usize = 104;

/// Columns between two columns of the panel, at least.
const PANEL_GAP: usize = 3;

/// The share of the screen's width that a panel of several columns takes,
/// in percent, as tuios's prefix panel does; its columns spread over it.
const PANEL_SHARE: usize = 72;

/// The order of the leader's groups in its panel, as LazyVim lists them:
/// agents, windows, interface, git, search, quit; any other after them.
const GROUP_ORDER: &str = "awugsq";

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

/// The headings of the columns of the keys panel (`?`), in order.
const CATEGORIES: [&str; 4] = ["Move", "Agent", "View", "Other"];

/// The column of the keys panel an action shows in: an index of
/// [`CATEGORIES`].
fn category(action: Action) -> usize {
    use Action as A;
    match action {
        A::Down
        | A::Up
        | A::Left
        | A::Right
        | A::First
        | A::Last
        | A::HalfPageDown
        | A::HalfPageUp
        | A::Jump
        | A::JumpBack
        | A::Filter
        | A::NextMatch
        | A::PreviousMatch
        | A::PreviousEvent
        | A::NextEvent
        | A::NextPane
        | A::PreviousPane
        | A::FocusLeft
        | A::FocusDown
        | A::FocusUp
        | A::FocusRight
        | A::NextField
        | A::PreviousField
        | A::NextChoice
        | A::PreviousChoice
        | A::LineStart
        | A::LineEnd => 0,
        A::TakeOver
        | A::Message
        | A::Diff
        | A::FullScreen
        | A::Forget
        | A::Adopt
        | A::OpenWith
        | A::OpenEditor
        | A::OpenDiff
        | A::OpenGit
        | A::OpenIde
        | A::NewAgent
        | A::NewMission
        | A::NewLead
        | A::FocusSubtree
        | A::Reply
        | A::Summarize
        | A::Send
        | A::Messages
        | A::Launch
        | A::Apply
        | A::Confirm
        | A::ToggleFull
        | A::Refresh => 1,
        A::ShowHierarchy
        | A::ShowNetwork
        | A::ShowRepos
        | A::ShowTimeline
        | A::PreviousView
        | A::NextView
        | A::ZoomIn
        | A::ZoomOut
        | A::ZoomAuto
        | A::ShowWall
        | A::TogglePreview
        | A::Wider
        | A::Narrower
        | A::Taller
        | A::Shorter
        | A::EqualPanels
        | A::PickTheme => 2,
        _ => 3,
    }
}

/// The entries of the current screen's popup: the focused panel's keys,
/// then the keys of everywhere, one per action or per pair of actions,
/// sequences behind the leader folded into one entry.
pub(crate) fn screen_entries(model: &Model) -> Vec<Entry> {
    screen_keyed(model)
        .into_iter()
        .map(|(_, entry)| entry)
        .collect()
}

/// The keys of the current screen in the panel's columns: what moves, what
/// acts on an agent, what changes the view, the rest.
pub(crate) fn screen_columns(model: &Model) -> Vec<Column> {
    let keyed = screen_keyed(model);
    CATEGORIES
        .iter()
        .enumerate()
        .filter_map(|(index, title)| {
            let entries: Vec<Entry> = keyed
                .iter()
                .filter(|(column, _)| *column == index)
                .map(|(_, entry)| entry.clone())
                .collect();
            (!entries.is_empty()).then(|| Column {
                title: (*title).to_owned(),
                entries,
            })
        })
        .collect()
}

/// The entries of [`screen_entries`], each with its column on the panel.
fn screen_keyed(model: &Model) -> Vec<(usize, Entry)> {
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
    let mut entries: Vec<(usize, Entry)> = Vec::new();
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
            entries.push((
                category(*action),
                Entry {
                    keys: join_keys(&keys),
                    label: pair_label.to_owned(),
                    group: false,
                },
            ));
        } else {
            done.push(*action);
            entries.push((
                category(*action),
                Entry {
                    keys: keys.replace("shift-", "⇧"),
                    label: (*label).to_owned(),
                    group: false,
                },
            ));
        }
    }
    if behind_leader {
        // Before the last entries (`?` keys, esc, quit), where it is seen.
        let at = entries.len().saturating_sub(3);
        entries.insert(
            at,
            (
                CATEGORIES.len() - 1,
                Entry {
                    keys: leader.short(),
                    label: format!("{}…", group_name(&[leader])),
                    group: true,
                },
            ),
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

/// A group's name as a column heading: `Agents`, `UI`, `Go to`.
fn heading(name: &str) -> String {
    if name.chars().count() <= 2 {
        return name.to_uppercase();
    }
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// What can follow the keys typed so far, in columns, in the manner of
/// tuios's prefix panel: one column per group, its keys spelled from here
/// (`a n  new agent`), and the keys that act at once under `Other`. With no
/// group to open, one column under the name of the group typed so far.
pub(crate) fn pending_columns(model: &Model) -> Vec<Column> {
    let contexts = model.contexts();
    let mut groups: Vec<(usize, Column)> = Vec::new();
    let mut direct: Vec<Entry> = Vec::new();
    for next in model.keymap.continuations(&contexts, &model.pending_keys) {
        if !next.group {
            direct.push(Entry {
                keys: next.key.short(),
                label: next.label.to_owned(),
                group: false,
            });
            continue;
        }
        let mut prefix = model.pending_keys.clone();
        prefix.push(next.key);
        let entries = model
            .keymap
            .continuations(&contexts, &prefix)
            .into_iter()
            .map(|then| Entry {
                keys: format!("{} {}", next.key.short(), then.key.short()),
                label: if then.group {
                    format!("+{}", then.label)
                } else {
                    then.label.to_owned()
                },
                group: then.group,
            })
            .collect();
        let rank = next
            .key
            .typed_char()
            .and_then(|key| GROUP_ORDER.find(key))
            .unwrap_or(GROUP_ORDER.len());
        groups.push((
            rank,
            Column {
                title: heading(next.label),
                entries,
            },
        ));
    }
    groups.sort_by_key(|(rank, _)| *rank);
    let mut columns: Vec<Column> = groups.into_iter().map(|(_, column)| column).collect();
    if !direct.is_empty() {
        let title = if columns.is_empty() {
            heading(group_name(&model.pending_keys))
        } else {
            "Other".to_owned()
        };
        columns.push(Column {
            title,
            entries: direct,
        });
    }
    columns
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

/// Draws the keys of the current screen: the panel of the leader's design,
/// or the box when the panel does not fit.
pub(crate) fn render_screen(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let all = model
        .keymap
        .first_key(Context::Keys, Action::AllKeys)
        .map(KeySeq::short);
    let footer = all.as_ref().map_or_else(
        || "esc to close".to_owned(),
        |all| format!("{all} all keys · esc to close"),
    );
    if render_panel(
        theme,
        &format!("Keys · {}", screen_title(model)),
        &footer,
        &screen_columns(model),
        Placement::Center,
        area,
        buf,
    ) {
        return;
    }
    let mut title = vec![Span::raw(screen_title(model))];
    if let Some(all) = all {
        title.push(Span::styled(
            format!(" · {all} all keys · esc"),
            theme.style(StyleRole::TextDim),
        ));
    }
    render_box(theme, title, &screen_entries(model), area, buf);
}

/// Draws what can follow the keys typed so far: the panel, or the box when
/// the panel does not fit.
pub(crate) fn render_pending(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    // `Leader`, then `Leader › agents` after `Space a`; `Go to`.
    let path: Vec<&str> = (1..=model.pending_keys.len())
        .map(|end| group_name(&model.pending_keys[..end]))
        .collect();
    if render_panel(
        theme,
        &heading(&path.join(" › ")),
        "esc to close",
        &pending_columns(model),
        Placement::BottomRight,
        area,
        buf,
    ) {
        return;
    }
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

/// A count of cells on screen.
fn cells(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

/// The most keys among the columns of a row of the window.
fn tallest(columns: &[Column], row: &[usize]) -> usize {
    row.iter()
        .map(|index| columns[*index].entries.len())
        .max()
        .unwrap_or(0)
}

/// Where a keys window puts its columns.
struct PanelLayout {
    /// For each column, the width of its widest key drawn as a key, and its
    /// own width.
    sizes: Vec<(usize, usize)>,
    /// The columns of each row, left to right.
    rows: Vec<Vec<usize>>,
    /// The width of what the window holds, inside its padding.
    content_width: usize,
    /// The window's height, its frame included.
    height: usize,
}

impl PanelLayout {
    /// Lays `columns` out under a title `title_width` cells wide, in a window
    /// at most `area_width` cells wide: columns left to right, a new row
    /// where the next would not fit. `None` when a column alone is too wide.
    fn new(columns: &[Column], title_width: usize, area_width: u16) -> Option<Self> {
        let sizes: Vec<(usize, usize)> = columns
            .iter()
            .map(|column| {
                // A key drawn as a key: its name with a space on each side.
                let cap = column
                    .entries
                    .iter()
                    .map(|entry| text::width(&entry.keys))
                    .max()
                    .unwrap_or(0)
                    + 2;
                let label = column
                    .entries
                    .iter()
                    .map(|entry| text::width(&entry.label).min(PANEL_LABEL))
                    .max()
                    .unwrap_or(0);
                (cap, (cap + 2 + label).max(text::width(&column.title)))
            })
            .collect();
        let room = usize::from(area_width).saturating_sub(2 * PANEL_PAD);
        let mut rows: Vec<Vec<usize>> = Vec::new();
        let mut used = 0;
        for (index, (_, width)) in sizes.iter().enumerate() {
            match rows.last_mut() {
                Some(row) if used + PANEL_GAP + width <= room => {
                    row.push(index);
                    used += PANEL_GAP + width;
                }
                _ => {
                    if *width > room {
                        return None;
                    }
                    rows.push(vec![index]);
                    used = *width;
                }
            }
        }
        let content_width = rows
            .iter()
            .map(|row| {
                row.iter().map(|index| sizes[*index].1).sum::<usize>()
                    + PANEL_GAP * row.len().saturating_sub(1)
            })
            .max()
            .unwrap_or(0)
            .max(title_width);
        // The frame and a blank row; each row of columns, its sections'
        // names then their keys, a blank row between two; a blank row and
        // the frame.
        let height = 4
            + rows
                .iter()
                .map(|row| 1 + tallest(columns, row))
                .sum::<usize>()
            + rows.len().saturating_sub(1);
        Some(Self {
            sizes,
            rows,
            content_width,
            height,
        })
    }
}

/// Draws one column at the top left of `at`: its section's name in
/// capitals, then each key drawn as a key, and what it does `cap_width` and
/// two cells on.
fn draw_column(theme: &Theme, column: &Column, cap_width: usize, at: Rect, buf: &mut Buffer) {
    put_spans(
        buf,
        at.x,
        at.y,
        &[Span::styled(
            column.title.to_uppercase(),
            theme.style(StyleRole::ModalSection),
        )],
        at.width,
    );
    let offset = cells(cap_width + 2);
    for (row, entry) in column.entries.iter().enumerate() {
        let y = at.y + 1 + cells(row);
        let cap = [Span::styled(
            format!(" {} ", entry.keys),
            theme.style(StyleRole::ModalKey),
        )];
        put_spans(buf, at.x, y, &cap, at.width);
        let label: String = entry.label.chars().take(PANEL_LABEL).collect();
        let role = if entry.group {
            StyleRole::ModalTitle
        } else {
            StyleRole::ModalText
        };
        put_spans(
            buf,
            at.x + offset,
            y,
            &[Span::styled(label, theme.style(role))],
            at.width.saturating_sub(offset),
        );
    }
}

/// Where a keys window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// At the bottom right, above the status line, most of the width wide:
    /// what can follow the leader.
    BottomRight,
    /// In the middle of the screen: the keys of the screen.
    Center,
}

/// Draws columns of keys in a modal window in the manner of the cockpit's
/// keyboard help: the screen behind fades, a thick lavender frame, `title`
/// in it and `footer` in the middle of its bottom, each column under its
/// section's name in capitals, each key drawn as a key. Columns take rows
/// when they do not fit side by side. Draws nothing and returns false when
/// the window does not fit.
pub(crate) fn render_panel(
    theme: &Theme,
    title: &str,
    footer: &str,
    columns: &[Column],
    placement: Placement,
    area: Rect,
    buf: &mut Buffer,
) -> bool {
    let area = area.intersection(buf.area);
    if columns.is_empty() {
        return false;
    }
    let words = (text::width(title) + 2).max(text::width(footer) + 2);
    let Some(layout) = PanelLayout::new(columns, words, area.width) else {
        return false;
    };
    let fitted = layout.content_width + 2 * PANEL_PAD;
    // The window leaves the status line, the area's last row, in sight.
    if fitted > usize::from(area.width) || layout.height >= usize::from(area.height) {
        return false;
    }
    // Several columns spread over a wide window, as in tuios and the
    // cockpit; a single group keeps the width of its keys.
    let width = if columns.len() > 1 {
        let wide = match placement {
            Placement::BottomRight => usize::from(area.width) * PANEL_SHARE / 100,
            Placement::Center => MODAL_WIDTH.min(usize::from(area.width).saturating_sub(4)),
        };
        fitted.max(wide)
    } else {
        fitted
    };
    let inner = width - 2 * PANEL_PAD;
    let (window_width, window_height) = (cells(width), cells(layout.height));
    let window = match placement {
        Placement::BottomRight => Rect::new(
            area.right().saturating_sub(window_width),
            area.bottom().saturating_sub(window_height + 1),
            window_width,
            window_height,
        ),
        Placement::Center => Rect::new(
            area.x + area.width.saturating_sub(window_width) / 2,
            area.y + area.height.saturating_sub(window_height) / 2,
            window_width,
            window_height,
        ),
    };
    modal_fitted(
        buf,
        window,
        vec![Span::raw(title.to_owned())],
        footer,
        theme,
    );
    let left = window.x + cells(PANEL_PAD);
    let mut y = window.y + 2;
    for row in &layout.rows {
        // The room the row leaves, shared between its gaps.
        let used: usize = row.iter().map(|index| layout.sizes[*index].1).sum();
        let gap = match row.len() {
            0 | 1 => 0,
            count => (inner.saturating_sub(used) / (count - 1)).max(PANEL_GAP),
        };
        let mut x = left;
        for index in row {
            let (cap_width, column_width) = layout.sizes[*index];
            let at = Rect::new(x, y, cells(column_width), 0);
            draw_column(theme, &columns[*index], cap_width, at, buf);
            x += cells(column_width + gap);
        }
        y += cells(tallest(columns, row) + 2);
    }
    true
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
    let inner = modal(buf, window, title, "", theme);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{model, press};

    fn titles(columns: &[Column]) -> Vec<&str> {
        columns.iter().map(|column| column.title.as_str()).collect()
    }

    fn keys(column: &Column) -> Vec<(&str, &str)> {
        column
            .entries
            .iter()
            .map(|entry| (entry.keys.as_str(), entry.label.as_str()))
            .collect()
    }

    #[test]
    fn the_leader_shows_one_column_per_group_in_lazyvim_order() {
        let mut model = model(190, 50);
        press(&mut model, " ");
        let columns = pending_columns(&model);
        assert_eq!(
            titles(&columns),
            ["Agents", "Windows", "UI", "Git", "Search", "Quit", "Other"]
        );
        assert_eq!(
            keys(&columns[0]),
            [
                ("a n", "new agent"),
                ("a m", "new mission"),
                ("a l", "new lead"),
                ("a a", "adopt"),
                ("a d", "forget"),
            ]
        );
        assert_eq!(keys(&columns[6]), [("n", "messages"), ("o", "open with…")]);
    }

    #[test]
    fn a_group_shows_its_own_keys_under_its_name() {
        let mut model = model(190, 50);
        press(&mut model, " w");
        let columns = pending_columns(&model);
        assert_eq!(titles(&columns), ["Windows"]);
        assert_eq!(
            keys(&columns[0]),
            [("m", "full screen"), ("w", "wall"), ("=", "equal panels")]
        );
    }

    #[test]
    fn headings_name_the_groups() {
        assert_eq!(heading("agents"), "Agents");
        assert_eq!(heading("ui"), "UI");
        assert_eq!(heading("go to"), "Go to");
    }
}
