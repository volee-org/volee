//! The status line, at the bottom, in the manner of vim's: the mode the
//! keyboard is in at the left (`VOLEE` while the keys drive volee, `EDIT`
//! and the agent's name while they go to an agent, `FILTER`, `MESSAGE`...),
//! then the few keys of the moment, generated from the keymap, and the way
//! to every key (`? help`) at the right; or the prompt or the notice of the
//! moment. On the main screen it shows only the keys that are not on screen
//! elsewhere: opening and writing to an agent sit on its preview's border.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use super::put_spans;
use crate::app::{Model, Prompt};
use crate::keymap::{Action, Context};
use crate::text;
use crate::theme::StyleRole;

/// Columns between two hints.
const SEPARATOR: &str = "   ";

/// Draws the status line, on the last row of `area`.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    if area.is_empty() {
        return;
    }
    let row = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    render_row(model, row, buf);
}

/// The mode the keyboard is in, vim style, and how its badge looks: the
/// main screen is `VOLEE`, a window or a field says what it is, and edit
/// mode stands out in the agent's color.
fn mode(model: &Model, contexts: &[Context]) -> (&'static str, Style) {
    let theme = &model.theme;
    let window = theme.style(StyleRole::TabActive);
    let typing = theme.style(StyleRole::Notice).add_modifier(Modifier::BOLD);
    let found = [
        (Context::Attached, "EDIT", typing),
        (Context::Filter, "FILTER", typing),
        (Context::Composer, "MESSAGE", typing),
        (Context::Dialog, "LAUNCH", typing),
        (Context::Commands, "COMMAND", typing),
        (Context::Themes, "THEME", window),
        (Context::Help, "HELP", window),
        (Context::Keys, "KEYS", window),
        (Context::Inbox, "MESSAGES", window),
        (Context::Diff, "CHANGES", window),
        (Context::Tools, "OPEN", window),
        (Context::Wall, "WALL", window),
        (Context::Screen, "VIEW", window),
    ]
    .into_iter()
    .find(|(context, _, _)| contexts.contains(context));
    match found {
        Some((_, name, style)) => (name, style),
        None => ("VOLEE", theme.style(StyleRole::TopbarBrand)),
    }
}

/// Draws the mode's badge at `x` and returns the cells it took, with the
/// gap after it.
fn put_mode(
    model: &Model,
    contexts: &[Context],
    x: u16,
    y: u16,
    width: u16,
    buf: &mut Buffer,
) -> u16 {
    let (name, style) = mode(model, contexts);
    let used = put_spans(
        buf,
        x,
        y,
        &[Span::styled(format!(" {name} "), style)],
        width,
    );
    (used + 2).min(width)
}

/// The order of the keys on the main screen's status line.
fn importance(action: Option<Action>) -> u8 {
    match action {
        Some(Action::Back) => 0,
        Some(Action::TakeOver) => 1,
        Some(Action::Message) => 2,
        Some(Action::Messages) => 3,
        Some(Action::Filter) => 4,
        None => 5,
        Some(Action::Commands) => 6,
        Some(Action::Quit) => 7,
        Some(_) => 8,
    }
}

/// Whether a key shows on the main screen's status line. The others are a
/// `?` away; opening an agent and writing to it are on its preview's border
/// when the layout draws its action bar.
fn featured(action: Option<Action>, action_bar: bool) -> bool {
    match action {
        // `None` is the leader: `␣ more`.
        None
        | Some(
            Action::Filter
            | Action::Messages
            | Action::Commands
            | Action::Quit
            | Action::Help
            | Action::Back,
        ) => true,
        Some(Action::TakeOver | Action::Message) => !action_bar,
        Some(_) => false,
    }
}

/// Draws the status line, or the prompt, the filter or the notice.
fn render_row(model: &Model, area: Rect, buf: &mut Buffer) {
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
        let skip = put_mode(model, &[Context::Filter], x, area.y, width, buf);
        let (x, width) = (x + skip, width.saturating_sub(skip));
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
    if model.notice.is_none() && crate::edit::editing(model) {
        render_editing(model, Rect::new(x, area.y, width, 1), buf);
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
        let back = vec![
            Span::styled(format!(" {} ", esc.short()), key),
            Span::styled(" back", words),
        ];
        groups.insert(0, (Some(Action::Back), back));
    }
    let main_screen = model.overlay.is_none() && contexts.contains(&Context::Global);
    if main_screen {
        // Opening and writing sit on the preview's border in a wide layout.
        let action_bar =
            model.layout.mode == crate::layout::Mode::Wide && model.layout.preview.is_some();
        groups.retain(|(action, _)| featured(*action, action_bar));
        // The most useful first: a narrow terminal cuts the end of the line.
        groups.sort_by_key(|(action, _)| importance(*action));
    }
    // The way to the help stays visible, at the right, whatever the width.
    let help = groups
        .iter()
        .position(|(action, _)| *action == Some(Action::Help))
        .map(|index| groups.remove(index).1);
    let help_width = help.as_deref().map_or(0, spans_width);
    let skip = put_mode(model, &contexts, x, area.y, width, buf);
    let (x, width) = (x + skip, width.saturating_sub(skip));
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

/// The cells some spans take.
fn spans_width(spans: &[Span<'_>]) -> u16 {
    u16::try_from(
        spans
            .iter()
            .map(|span| text::width(&span.content))
            .sum::<usize>(),
    )
    .unwrap_or(u16::MAX)
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
    // The badge in the agent's color: whose keys these are shows from afar.
    let accent = model
        .live
        .as_ref()
        .and_then(|live| model.registry.agent(live.agent))
        .map(|agent| crate::widgets::card::CardContext::new(model).accent(agent));
    let badge = match accent.and_then(|style| style.fg) {
        Some(color) => Style {
            fg: theme.style(StyleRole::TopbarBrand).fg,
            bg: Some(color),
            ..Style::default()
        }
        .add_modifier(Modifier::BOLD),
        None => theme.style(StyleRole::Notice).add_modifier(Modifier::BOLD),
    };
    let spans = vec![
        Span::styled(" EDIT ", badge),
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
            // A key drawn as a key: a cap with room around its name.
            let spans = vec![
                Span::styled(format!(" {} ", hint.keys), theme.style(StyleRole::HintKey)),
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
