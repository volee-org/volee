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
    demo_model_at(width, height, theme, start())
}

/// The demo `at` a moment of its scenario, as if the interface had been
/// open since then.
fn demo_model_at(width: u16, height: u16, theme: Theme, at: Timestamp) -> (Model, DemoSource) {
    let demo = DemoSource::starting_at(start(), Arc::new(ManualClock::new(at)));
    let mut model = Model::new(
        theme,
        Keymap::default(),
        Some("demo · simulated data".into()),
        (width, height),
        at,
    );
    let effects = update(&mut model, Msg::Events(demo.events_until(at)));
    serve(&demo, &mut model, effects);
    (model, demo)
}

/// Answers preview requests the way the run loop would.
fn serve(demo: &DemoSource, model: &mut Model, effects: Vec<Effect>) {
    for effect in effects {
        if let Effect::LoadPreview(agent) = effect {
            let result = demo.screen_at(agent, model.now);
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
fn every_key_in_columns() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "??");
    assert_snapshot!(draw(&model));
}

#[test]
fn the_leader_shows_what_follows() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, " ");
    assert_snapshot!(draw(&model));
}

#[test]
fn the_command_palette() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, ":show");
    assert_snapshot!(draw(&model));
}

#[test]
fn an_agent_screen_full_size() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "z");
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

/// The demo at its start on a view, in a terminal of the given size.
fn view_at(tab: char, width: u16, height: u16) -> String {
    let (mut model, demo) = demo_model(width, height, Theme::default());
    type_text(&demo, &mut model, &tab.to_string());
    draw(&model)
}

#[test]
fn network_at_80x24() {
    assert_snapshot!(view_at('2', 80, 24));
}

#[test]
fn network_at_120x40() {
    assert_snapshot!(view_at('2', 120, 40));
}

#[test]
fn network_at_200x50() {
    assert_snapshot!(view_at('2', 200, 50));
}

#[test]
fn repos_at_80x24() {
    assert_snapshot!(view_at('3', 80, 24));
}

#[test]
fn repos_at_120x40() {
    assert_snapshot!(view_at('3', 120, 40));
}

#[test]
fn repos_at_200x50() {
    assert_snapshot!(view_at('3', 200, 50));
}

#[test]
fn timeline_at_80x24() {
    assert_snapshot!(view_at('4', 80, 24));
}

#[test]
fn timeline_at_120x40() {
    assert_snapshot!(view_at('4', 120, 40));
}

#[test]
fn timeline_at_200x50() {
    assert_snapshot!(view_at('4', 200, 50));
}

#[test]
fn a_message_on_its_way() {
    // Half a second after trace-gateway writes to latency-audit, with the
    // network on screen and trace-gateway selected.
    let at = start().plus_millis(16_500);
    let (mut model, demo) = demo_model_at(120, 40, Theme::default(), at);
    type_text(&demo, &mut model, "2'g");
    assert!(model.pulses.active(model.now).next().is_some());
    assert_snapshot!(draw(&model));
}

#[test]
fn network_zoom_levels() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    keys(
        &demo,
        &mut model,
        &[KeyCode::Char('2'), KeyCode::Tab, KeyCode::Char('_')],
    );
    let less = draw(&model);
    type_text(&demo, &mut model, "_");
    let dots = draw(&model);
    type_text(&demo, &mut model, "++");
    let cards = draw(&model);
    assert_snapshot!("network_zoom_compact", less);
    assert_snapshot!("network_zoom_dots", dots);
    assert_snapshot!("network_zoom_cards", cards);
}

#[test]
fn network_filter_dims_the_others() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "2/latency");
    keys(&demo, &mut model, &[KeyCode::Enter]);
    assert_snapshot!(draw(&model));
}

#[test]
fn repos_one_line_per_agent() {
    let (mut model, demo) = demo_model(200, 50, Theme::default());
    keys(
        &demo,
        &mut model,
        &[KeyCode::Char('3'), KeyCode::Tab, KeyCode::Char('_')],
    );
    assert_snapshot!(draw(&model));
}

#[test]
fn timeline_scrubbed_to_an_event() {
    // invoice-tools asked you a question a minute before the start: `[`
    // takes the cursor back to the events of its lane.
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    type_text(&demo, &mut model, "4'v");
    keys(&demo, &mut model, &[KeyCode::Tab, KeyCode::Char('[')]);
    assert!(model.timeline.cursor.is_some());
    assert_snapshot!(draw(&model));
}

#[test]
fn timeline_zoomed_in() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    keys(
        &demo,
        &mut model,
        &[
            KeyCode::Char('4'),
            KeyCode::Tab,
            KeyCode::Char('+'),
            KeyCode::Char('+'),
        ],
    );
    assert_snapshot!(draw(&model));
}

#[test]
fn views_survive_tiny_terminals() {
    for (width, height) in [(1, 1), (20, 6), (40, 10), (60, 15), (99, 27)] {
        for tab in ['1', '2', '3', '4'] {
            let (mut model, demo) = demo_model(width, height, Theme::default());
            keys(&demo, &mut model, &[KeyCode::Char(tab), KeyCode::Tab]);
            for code in [KeyCode::Char('j'), KeyCode::Char('l'), KeyCode::Char('_')] {
                keys(&demo, &mut model, &[code]);
            }
            let _ = draw(&model);
        }
    }
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

/// An agent in a volee host, its screen live inside volee's frame: watched
/// with `z`, then typed into in edit mode.
#[test]
fn edit_mode_inside_volee() {
    let (mut model, _) = crate::test_support::hosted_model(80, 24);
    type_keys(&mut model, "i");
    let size = volee_host::Size { cols: 77, rows: 21 };
    update(
        &mut model,
        Msg::LiveOpened {
            link: 1,
            welcome: crate::test_support::welcome(size, true),
        },
    );
    let screen = "\x1b[2J\x1b[H\x1b[1m⏺\x1b[0m The tests pass: 356 of 356.\r\n\r\n\
                  \x1b[2m╭──────────────────────────────╮\x1b[0m\r\n\
                  \x1b[2m│\x1b[0m > commit and push            \x1b[2m│\x1b[0m\r\n\
                  \x1b[2m╰──────────────────────────────╯\x1b[0m\r\n\
                  \x1b[4;20H";
    update(
        &mut model,
        Msg::LiveUpdate {
            link: 1,
            update: volee_host::Update::Snapshot(screen.as_bytes().to_vec()),
        },
    );
    let terminal = terminal(&model);
    assert_snapshot!("edit_mode", terminal.backend().to_string());
    // The agent's cursor is the terminal's: after "commit and push".
    let mut terminal = terminal;
    let position = terminal.get_cursor_position().unwrap();
    assert_eq!((position.x, position.y), (2 + 19, 1 + 3));
    terminal.backend_mut().assert_cursor_position((21, 4));

    // Back to volee: the same screen, watched.
    update(
        &mut model,
        Msg::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
    );
    update(
        &mut model,
        Msg::LiveOpened {
            link: 2,
            welcome: crate::test_support::welcome(size, true),
        },
    );
    assert_snapshot!("watching_live", draw(&model));
}

fn type_keys(model: &mut Model, text: &str) {
    for c in text.chars() {
        update(
            model,
            Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
        );
    }
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
        type_text(&demo, &mut model, "I");
        assert_snapshot!(format!("messages_at_{width}x{height}"), draw(&model));
    }
}

#[test]
fn messages_reading_a_mission_thread_with_its_summary() {
    let (mut model, demo) = demo_model(120, 40, Theme::default());
    // The row after the pinned question: the Invoicing mission's thread,
    // where volee's supervisor wrote last.
    type_text(&demo, &mut model, "Ij");
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
    type_text(&demo, &mut model, " n");
    keys(&demo, &mut model, &[KeyCode::Right, KeyCode::Tab]);
    type_text(&demo, &mut model, "feat/invoice-export");
    keys(&demo, &mut model, &[KeyCode::Tab]);
    type_text(
        &demo,
        &mut model,
        "Export invoices as CSV, one file per month.",
    );
    // Enter starts a second line of the task; ctrl-s launches.
    keys(&demo, &mut model, &[KeyCode::Enter]);
    type_text(&demo, &mut model, "Keep amounts in cents.");
    let effects = update(
        &mut model,
        Msg::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
    );
    assert!(
        matches!(effects.as_slice(), [Effect::Spawn(_)]),
        "{effects:?}"
    );
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
    type_text(&demo, &mut model, " N");
    type_text(&demo, &mut model, "Invoice export");
    keys(
        &demo,
        &mut model,
        &[KeyCode::Tab, KeyCode::Tab, KeyCode::Char(' ')],
    );
    assert_snapshot!(draw(&model));
}

/// A machine where no agent ever ran, on the given tab.
fn empty_machine(tab: char, width: u16, height: u16, claude_code_available: bool) -> Model {
    let mut model = Model::new(
        Theme::default(),
        Keymap::default(),
        None,
        (width, height),
        start(),
    );
    model.settings.claude_code_available = claude_code_available;
    update(&mut model, Msg::Events(Vec::new()));
    update(
        &mut model,
        Msg::Key(KeyEvent::new(KeyCode::Char(tab), KeyModifiers::NONE)),
    );
    model
}

#[test]
fn without_claude_code_the_empty_views_say_where_to_look() {
    for tab in ['1', '2', '3', '4'] {
        let found = draw(&empty_machine(tab, 120, 40, true));
        assert!(!found.contains("volee daemon status"), "{found}");
        let missing = draw(&empty_machine(tab, 120, 40, false));
        assert!(missing.contains("volee daemon status"), "{missing}");
        assert!(!missing.contains("Start Claude Code"), "{missing}");
    }
    for (tab, view) in [('1', "hierarchy"), ('2', "network")] {
        assert_snapshot!(
            format!("no_claude_code_{view}_at_120x40"),
            draw(&empty_machine(tab, 120, 40, false))
        );
    }
    assert_snapshot!(
        "no_claude_code_hierarchy_at_80x24",
        draw(&empty_machine('1', 80, 24, false))
    );
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
