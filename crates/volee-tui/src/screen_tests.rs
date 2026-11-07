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
            let result = demo.screen_at(agent, model.now).map_err(|e| e.to_string());
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
