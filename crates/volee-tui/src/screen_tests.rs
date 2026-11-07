//! Whole screens, drawn with ratatui's test backend at a fixed time and
//! compared with reviewed snapshots (`cargo insta review`), plus tests that
//! keep the user guide in step with the keymap and the theme roles.

use std::sync::Arc;

use insta::assert_snapshot;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use volee_core::time::Timestamp;

use crate::app::{Effect, Model, Msg, update};
use crate::clock::ManualClock;
use crate::demo::{DemoSource, INVOICE_SCHEMA, agent_id};
use crate::keymap::Keymap;
use crate::theme::{StyleRole, Theme};
use crate::view::view;

fn start() -> Timestamp {
    Timestamp::from_millis(1_793_986_617_000)
}

/// The demo at its start, previews loaded, in a terminal of the given size.
fn demo_model(width: u16, height: u16, theme: Theme) -> (Model, DemoSource) {
    let demo = DemoSource::new(Arc::new(ManualClock::new(start())));
    let mut model = Model::new(
        theme,
        Keymap::default(),
        Some("demo · simulated data".into()),
        (width, height),
        start(),
    );
    let effects = update(&mut model, Msg::Events(demo.events_until(start())));
    serve(&demo, &mut model, effects);
    (model, demo)
}

/// Answers preview requests the way the run loop would.
fn serve(demo: &DemoSource, model: &mut Model, effects: Vec<Effect>) {
    for effect in effects {
        if let Effect::LoadPreview(agent) = effect {
            let result = demo.screen_at(agent, start());
            update(model, Msg::PreviewLoaded { agent, result });
        }
    }
}

fn keys(demo: &DemoSource, model: &mut Model, keys: &[KeyCode]) {
    for code in keys {
        let effects = update(model, Msg::Key(KeyEvent::new(*code, KeyModifiers::NONE)));
        serve(demo, model, effects);
    }
}

fn type_text(demo: &DemoSource, model: &mut Model, text: &str) {
    let codes: Vec<KeyCode> = text.chars().map(KeyCode::Char).collect();
    keys(demo, model, &codes);
}

fn terminal(model: &Model) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(model.size.0, model.size.1)).unwrap();
    terminal.draw(|frame| view(model, frame)).unwrap();
    terminal
}

fn draw(model: &Model) -> String {
    terminal(model).backend().to_string()
}

#[test]
fn demo_at_80x24() {
    let (model, _) = demo_model(80, 24, Theme::default());
    assert_snapshot!(draw(&model));
}

#[test]
fn demo_at_120x40() {
    let (model, _) = demo_model(120, 40, Theme::default());
    assert_snapshot!(draw(&model));
}

#[test]
fn demo_at_200x50() {
    let (model, _) = demo_model(200, 50, Theme::default());
    assert_snapshot!(draw(&model));
}

#[test]
fn permission_prompt_in_the_preview() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "'s");
    assert_eq!(model.selected, Some(agent_id(start(), INVOICE_SCHEMA)));
    assert_snapshot!(draw(&model));
}

#[test]
fn help_overlay() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "?");
    assert_snapshot!(draw(&model));
}

#[test]
fn zoom_cards_with_scrolling() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    keys(&demo, &mut model, &[KeyCode::Tab, KeyCode::Char('+')]);
    assert_snapshot!(draw(&model));
}

#[test]
fn zoom_compact() {
    let (mut model, demo) = demo_model(200, 50, Theme::default());
    keys(&demo, &mut model, &[KeyCode::Tab, KeyCode::Char('_')]);
    assert_snapshot!(draw(&model));
}

#[test]
fn zoom_dots() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    keys(&demo, &mut model, &[KeyCode::Tab, KeyCode::Char('_')]);
    assert_snapshot!(draw(&model));
}

#[test]
fn focused_subtree() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "'n");
    keys(&demo, &mut model, &[KeyCode::Enter]);
    assert_snapshot!(draw(&model));
}

#[test]
fn filter_prompt() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "/latency");
    assert_snapshot!(draw(&model));
}

#[test]
fn jump_prompt() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "'");
    assert_snapshot!(draw(&model));
}

#[test]
fn coming_views() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "2");
    assert_snapshot!(draw(&model));
}

#[test]
fn composer_answering_a_question() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "'vm");
    type_text(
        &demo,
        &mut model,
        "Yes: accept draft, false by default, and issue the final invoice when the draft is \
         confirmed.",
    );
    update(
        &mut model,
        Msg::Paste("\n\nKeep the PDF for finals only.".into()),
    );
    assert_snapshot!(draw(&model));
}

#[test]
fn composer_for_a_session_started_outside_volee() {
    let (mut model, demo) = demo_model(80, 24, Theme::default());
    type_text(&demo, &mut model, "'im");
    assert_snapshot!(draw(&model));
}

#[test]
fn quit_with_a_draft() {
    let (mut model, demo) = demo_model(80, 24, Theme::default());
    type_text(&demo, &mut model, "'sm");
    type_text(&demo, &mut model, "Is total_cents stored?");
    keys(&demo, &mut model, &[KeyCode::Esc, KeyCode::Char('q')]);
    assert_snapshot!(draw(&model));
}

#[test]
fn changes_window() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "'pd");
    let page = agent_id(start(), crate::demo::INVOICE_PAGE);
    for full in [false, true] {
        if full {
            type_text(&demo, &mut model, "f");
        }
        let result =
            futures_util::FutureExt::now_or_never(crate::source::Source::diff(&demo, page, full))
                .unwrap();
        update(
            &mut model,
            Msg::DiffLoaded {
                agent: page,
                full,
                result,
            },
        );
        assert_snapshot!(format!("changes_window_full_{full}"), draw(&model));
    }
}

#[test]
fn offline_after_the_daemon_left() {
    let (mut model, _) = demo_model(120, 40, Theme::default());
    update(
        &mut model,
        Msg::SourceClosed(Some("lost the connection to the volee daemon".into())),
    );
    assert_snapshot!(draw(&model));
}

#[test]
fn messages_at_every_size() {
    for (width, height) in [(80, 24), (120, 40), (200, 50)] {
        let (mut model, demo) = demo_model(width, height, Theme::default());
        type_text(&demo, &mut model, "i");
        assert_snapshot!(format!("messages_at_{width}x{height}"), draw(&model));
    }
}

#[test]
fn messages_reading_a_mission_thread_with_its_summary() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    // The second row after the pinned question and the inbox: the
    // Invoicing mission's thread.
    type_text(&demo, &mut model, "ijj");
    keys(&demo, &mut model, &[KeyCode::Enter]);
    let effects = update(
        &mut model,
        Msg::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)),
    );
    for effect in effects {
        if let Effect::Summarize(thread) = effect {
            let result = futures_util::FutureExt::now_or_never(crate::source::Source::summarize(
                &demo, thread,
            ))
            .unwrap();
            update(&mut model, Msg::Summarized { thread, result });
        }
    }
    assert_snapshot!(draw(&model));
}

#[test]
fn tools_menu_offers_what_was_found() {
    use crate::tools::{Tool, ToolKind};
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    model.settings.tools = vec![
        Tool {
            kind: ToolKind::Editor,
            label: "nvim".into(),
            command: vec!["nvim".into(), ".".into()],
        },
        Tool {
            kind: ToolKind::Diff,
            label: "nvim".into(),
            command: vec!["nvim".into(), "-c".into(), "DiffviewOpen {base}".into()],
        },
        Tool {
            kind: ToolKind::Ide,
            label: "VS Code".into(),
            command: vec!["code".into(), "{dir}".into()],
        },
    ];
    type_text(&demo, &mut model, "'po");
    assert_snapshot!(draw(&model));
}

#[test]
fn new_agent_dialog_with_a_refusal() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    model.settings.repos = demo.repos();
    type_text(&demo, &mut model, "n");
    keys(&demo, &mut model, &[KeyCode::Right, KeyCode::Tab]);
    type_text(&demo, &mut model, "feat/invoice-export");
    keys(&demo, &mut model, &[KeyCode::Tab]);
    type_text(
        &demo,
        &mut model,
        "Export invoices as CSV, one file per month.",
    );
    keys(&demo, &mut model, &[KeyCode::Enter]);
    update(
        &mut model,
        Msg::Launched(Err(crate::source::SourceError::Failed(
            "Claude Code does not trust /home/dev/code/dashboard: run `claude` there once and \
             accept the trust prompt"
                .into(),
        ))),
    );
    assert_snapshot!(draw(&model));
}

#[test]
fn new_mission_dialog_on_a_small_screen() {
    let (mut model, demo) = demo_model(80, 24, Theme::default());
    model.settings.repos = demo.repos();
    type_text(&demo, &mut model, "N");
    type_text(&demo, &mut model, "Invoice export");
    keys(
        &demo,
        &mut model,
        &[KeyCode::Tab, KeyCode::Tab, KeyCode::Char(' ')],
    );
    assert_snapshot!(draw(&model));
}

#[test]
fn themes_change_colors_only() {
    let (mocha, _) = demo_model(120, 40, Theme::default());
    for name in Theme::builtin_names() {
        let (model, _) = demo_model(120, 40, Theme::builtin(name).unwrap());
        assert_eq!(draw(&model), draw(&mocha), "{name}");
    }
}

#[test]
fn states_are_colored_by_the_theme() {
    let theme = Theme::builtin("terminal").unwrap();
    let (model, _) = demo_model(120, 40, theme.clone());
    let terminal = terminal(&model);
    let buffer = terminal.backend().buffer();
    let rail = model.layout.rail;
    // The first row of the list is an agent that needs you.
    let glyph = &buffer[(rail.x + 4, rail.y + 1)];
    assert_eq!(glyph.symbol(), "!");
    assert_eq!(Some(glyph.fg), theme.style(StyleRole::StateNeedsInput).fg);
    assert_eq!(glyph.fg, Color::Yellow);
}

/// Checks that a page of the user guide holds `expected` between
/// `<!-- name:start -->` and `<!-- name:end -->`. With `VOLEE_BLESS=1` in
/// the environment, writes it there instead.
fn check_generated(page: &str, name: &str, expected: &str) {
    let expected = expected.trim_end_matches('\n');
    let path = format!("{}/../../docs/src/guide/{page}", env!("CARGO_MANIFEST_DIR"));
    let document = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    let start = format!("<!-- {name}:start -->\n");
    let end = format!("<!-- {name}:end -->");
    let (Some(from), Some(to)) = (document.find(&start), document.find(&end)) else {
        panic!("{page} has no {name} markers");
    };
    let from = from + start.len();
    if document[from..to].trim_end_matches('\n') == expected {
        return;
    }
    assert!(
        std::env::var_os("VOLEE_BLESS").is_some(),
        "docs/src/guide/{page} is out of date: run `VOLEE_BLESS=1 cargo test -p volee-tui` \
         to regenerate it"
    );
    let updated = format!("{}{expected}\n{}", &document[..from], &document[to..]);
    std::fs::write(&path, updated).unwrap();
}

#[test]
fn the_user_guide_lists_every_key() {
    check_generated("interface.md", "keys", &Keymap::default().markdown());
}

#[test]
fn the_theme_guide_lists_every_role() {
    use std::fmt::Write as _;
    let mut expected = String::from("| Role | Used for |\n| --- | --- |");
    for role in StyleRole::ALL {
        write!(expected, "\n| `{}` | {} |", role.key(), role.description()).unwrap();
    }
    check_generated("themes.md", "roles", &expected);
}
