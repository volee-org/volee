//! The bottom line: key hints for the focused panel, generated from the
//! keymap, or the prompt or the notice of the moment.

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

/// Draws the line.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    if area.is_empty() {
        return;
    }
    let theme = &model.theme;
    let key = theme.style(StyleRole::HintKey);
    let words = theme.style(StyleRole::HintText);
    let width = area.width.saturating_sub(1);
    let x = area.x + 1;

    if let Some(prompt) = model.prompt {
        render_prompt(model, prompt, Rect::new(x, area.y, width, 1), buf);
        return;
    }
    if model.filter.editing {
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
    if let Some(notice) = &model.notice {
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
        Prompt::AdoptAndTakeOver(agent) => {
            let mut spans = vec![
                Span::styled(
                    format!(
                        "{} runs in the background, where volee cannot type. Move it into a \
                         volee host and take it over? Its conversation goes on.",
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
    };
    put_spans(buf, area.x, area.y, &spans, area.width);
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
