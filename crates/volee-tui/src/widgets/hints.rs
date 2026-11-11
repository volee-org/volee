//! The bottom lines: key hints for the focused panel, generated from the
//! keymap, or the prompt or the notice of the moment. On the main screen,
//! with room for two rows, the top one says how to move around and the
//! bottom one what to do.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;

use super::put_spans;
use crate::app::{Model, Prompt};
use crate::keymap::{Action, Context};
use crate::text;
use crate::theme::StyleRole;

/// Columns between two hints.
const SEPARATOR: &str = "   ";

/// Which hints a row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    /// Every hint, on one row.
    All,
    /// How to move around: moves, views, panels, the filter and its matches.
    Moves,
    /// What to do with the selected agent and the rest.
    Actions,
}

/// Whether a hint moves around rather than acts.
fn moves(action: Option<Action>) -> bool {
    matches!(
        action,
        Some(
            Action::Left
                | Action::Down
                | Action::Up
                | Action::Right
                | Action::First
                | Action::Last
                | Action::HalfPageDown
                | Action::HalfPageUp
                | Action::Filter
                | Action::NextMatch
                | Action::PreviousMatch
                | Action::Jump
                | Action::JumpBack
                | Action::ShowHierarchy
                | Action::ShowNetwork
                | Action::ShowRepos
                | Action::ShowTimeline
                | Action::PreviousView
                | Action::NextView
                | Action::NextPane
                | Action::PreviousPane
                | Action::FocusSubtree
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::ZoomAuto
                | Action::PreviousEvent
                | Action::NextEvent
                | Action::Back
        )
    )
}

/// Draws the hints: two rows on the main screen when there is room, else
/// one, at the bottom.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    if area.is_empty() {
        return;
    }
    let bottom = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    let main_screen = model.contexts().contains(&Context::Global);
    if area.height >= 2 && main_screen {
        render_row(
            model,
            Row::Moves,
            Rect::new(area.x, area.y, area.width, 1),
            buf,
        );
        render_row(model, Row::Actions, bottom, buf);
    } else {
        render_row(model, Row::All, bottom, buf);
    }
}

/// Draws one row of hints, or the prompt, the filter or the notice.
fn render_row(model: &Model, row: Row, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let key = theme.style(StyleRole::HintKey);
    let words = theme.style(StyleRole::HintText);
    let width = area.width.saturating_sub(1);
    let x = area.x + 1;

    if row != Row::Moves
        && let Some(prompt) = model.prompt
    {
        render_prompt(model, prompt, Rect::new(x, area.y, width, 1), buf);
        return;
    }
    if row != Row::Moves && model.filter.editing {
        let mut spans = vec![
            Span::styled("/", key),
            Span::styled(model.filter.text.clone(), theme.style(StyleRole::Input)),
            Span::styled("▏", key),
            Span::raw(SEPARATOR),
        ];
        let hints = groups(model, &[Context::Filter])
            .into_iter()
            .map(|(_, group)| group);
        spans.extend(join(hints, usize::from(width)));
        put_spans(buf, x, area.y, &spans, width);
        return;
    }
    if row != Row::Moves && model.notice.is_none() && crate::edit::editing(model) {
        render_editing(model, Rect::new(x, area.y, width, 1), buf);
        return;
    }
    if row != Row::Moves
        && let Some(notice) = &model.notice
    {
        let style = theme.style(if notice.error {
            StyleRole::NoticeError
        } else {
            StyleRole::Notice
        });
        let room = usize::from(width.saturating_sub(2));
        let message = format!(" {} ", text::truncate(&notice.text, room));
        put_spans(buf, x, area.y, &[Span::styled(message, style)], width);
        return;
    }
    let contexts = model.contexts();
    let mut groups = groups(model, &contexts);
    // A draft waits while the composer is closed: say so first.
    if model.overlay.is_none()
        && let Some(draft) = &model.draft
    {
        let mark = vec![Span::styled(
            format!("✎ draft to {}", model.name_of(draft.to)),
            theme.style(StyleRole::Input),
        )];
        groups.insert(0, (Some(Action::Message), mark));
    }
    // Windows and prompts show their own way out.
    if model.can_go_back()
        && contexts.contains(&Context::Global)
        && let Some(esc) = model.keymap.first_key(Context::Global, Action::Back)
    {
        let back = vec![Span::styled(esc.short(), key), Span::styled(" back", words)];
        groups.insert(0, (Some(Action::Back), back));
    }
    match row {
        Row::All => {}
        Row::Moves => groups.retain(|(action, _)| moves(*action)),
        Row::Actions => groups.retain(|(action, _)| !moves(*action)),
    }
    // The way to the help stays visible, at the right, whatever the width.
    let help = groups
        .iter()
        .position(|(action, _)| *action == Some(Action::Help))
        .map(|index| groups.remove(index).1);
    let help_width = help.as_ref().map_or(0, |group| {
        u16::try_from(
            group
                .iter()
                .map(|span| text::width(&span.content))
                .sum::<usize>(),
        )
        .unwrap_or(0)
    });
    let room = width.saturating_sub(help_width + 2);
    let rest = groups.into_iter().map(|(_, group)| group);
    put_spans(buf, x, area.y, &join(rest, usize::from(room)), room);
    if let Some(help) = help
        && help_width < width
    {
        put_spans(
            buf,
            area.right() - help_width - 1,
            area.y,
            &help,
            help_width,
        );
    }
}

/// Edit mode: where the keys go, and the key that gives them back.
fn render_editing(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let name = model
        .live
        .as_ref()
        .map(|live| model.name_of(live.agent))
        .unwrap_or_default();
    let leave = model.keymap.leave_key().short();
    let spans = vec![
        Span::styled(
            " EDIT ",
            theme
                .style(StyleRole::Notice)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        Span::styled(
            format!("  your keys go to {name}"),
            theme.style(StyleRole::Input),
        ),
        Span::raw(SEPARATOR),
        Span::styled(leave, theme.style(StyleRole::HintKey)),
        Span::styled(" back to volee", theme.style(StyleRole::HintText)),
    ];
    put_spans(buf, area.x, area.y, &spans, area.width);
}

/// The question of a prompt and its answers.
fn render_prompt(model: &Model, prompt: Prompt, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let key = theme.style(StyleRole::HintKey);
    let words = theme.style(StyleRole::HintText);
    let spans = match prompt {
        Prompt::Jump => vec![
            Span::styled("jump to", theme.style(StyleRole::Input)),
            Span::styled(": press the letter of an agent   ", words),
            Span::styled("esc", key),
            Span::styled(" cancel", words),
        ],
        Prompt::Quit => {
            let to = model
                .draft
                .as_ref()
                .map(|draft| model.name_of(draft.to))
                .unwrap_or_default();
            let mut spans = vec![
                Span::styled(
                    format!("Quit and lose the draft to {to}?"),
                    theme.style(StyleRole::Input),
                ),
                Span::raw(SEPARATOR),
            ];
            let answers = groups(model, &[Context::Confirm])
                .into_iter()
                .map(|(_, group)| group);
            spans.extend(join(answers, usize::from(area.width)));
            spans
        }
        Prompt::Forget(agent) => {
            let mut spans = vec![
                Span::styled(
                    format!(
                        "Forget {}? volee stops listing it and ignores its sessions; nothing is \
                         stopped or deleted.",
                        model.name_of(agent)
                    ),
                    theme.style(StyleRole::Input),
                ),
                Span::raw(SEPARATOR),
            ];
            let answers = groups(model, &[Context::Confirm])
                .into_iter()
                .map(|(_, group)| group);
            spans.extend(join(answers, usize::from(area.width)));
            spans
        }
        Prompt::AwaitMove(agent) => await_move(model, agent),
        Prompt::AdoptAndTakeOver(agent) => {
            let mut spans = vec![
                Span::styled(
                    format!(
                        "Move {} into a volee host? Its session is stopped and resumed there \
                         with volee's settings in place of its own. Opening it (enter) needs \
                         none of this.",
                        model.name_of(agent)
                    ),
                    theme.style(StyleRole::Input),
                ),
                Span::raw(SEPARATOR),
            ];
            let answers = groups(model, &[Context::Confirm])
                .into_iter()
                .map(|(_, group)| group);
            spans.extend(join(answers, usize::from(area.width)));
            spans
        }
        Prompt::Answer { question, option } => {
            let asked = model.registry.message(question);
            let answer = asked
                .and_then(|message| message.options.get(option))
                .map(String::as_str)
                .unwrap_or_default();
            let to = asked.map_or_else(String::new, |message| match message.from {
                volee_core::message::Party::Agent(agent) => model.name_of(agent),
                other => other.to_string(),
            });
            let mut spans = vec![
                Span::styled(
                    format!("Answer \"{answer}\" to {to}?"),
                    theme.style(StyleRole::Input),
                ),
                Span::raw(SEPARATOR),
            ];
            let answers = groups(model, &[Context::Confirm])
                .into_iter()
                .map(|(_, group)| group);
            spans.extend(join(answers, usize::from(area.width)));
            spans
        }
    };
    put_spans(buf, area.x, area.y, &spans, area.width);
}

/// What the bottom line says while volee waits for a session to leave the
/// terminal that holds it (ADR 0034): where to go, the key to press there,
/// named in words, and the key that gives the wait up. A window says it in
/// full when the terminal has room for one ([`super::handover`]); what
/// matters comes early in the sentence, since a narrow terminal cuts its
/// end.
fn await_move(model: &Model, agent: volee_core::AgentId) -> Vec<Span<'static>> {
    let theme = &model.theme;
    vec![
        Span::styled(
            format!(
                "To open {}, go to its terminal and press the left arrow key (←) on its \
                 empty input: volee is waiting.",
                model.name_of(agent)
            ),
            theme.style(StyleRole::Input),
        ),
        Span::raw(SEPARATOR),
        Span::styled(
            model.keymap.key_of(Context::Confirm, Action::Back, "esc"),
            theme.style(StyleRole::HintKey),
        ),
        Span::styled(" cancel", theme.style(StyleRole::HintText)),
    ]
}

/// One group of spans per hint (`j/k` and ` move`), with its action.
fn groups(model: &Model, contexts: &[Context]) -> Vec<(Option<Action>, Vec<Span<'static>>)> {
    let theme = &model.theme;
    model
        .keymap
        .hints(contexts)
        .into_iter()
        .map(|hint| {
            let spans = vec![
                Span::styled(hint.keys, theme.style(StyleRole::HintKey)),
                Span::styled(format!(" {}", hint.text), theme.style(StyleRole::HintText)),
            ];
            (hint.action, spans)
        })
        .collect()
}

/// Whole hints, separated, as many as fit in `width`.
fn join(groups: impl IntoIterator<Item = Vec<Span<'static>>>, width: usize) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut used = 0;
    for group in groups {
        let separator = if spans.is_empty() { 0 } else { SEPARATOR.len() };
        let group_width: usize = group.iter().map(|span| text::width(&span.content)).sum();
        if used + separator + group_width > width {
            break;
        }
        if separator > 0 {
            spans.push(Span::raw(SEPARATOR));
        }
        used += separator + group_width;
        spans.extend(group);
    }
    spans
}
