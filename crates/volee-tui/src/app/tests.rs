//! `update` without a terminal: keys, mouse, time and events in; model and
//! effects out.

use std::sync::Arc;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use volee_core::AgentId;
use volee_core::time::Timestamp;

use super::*;
use crate::clock::ManualClock;
use crate::demo::{
    DemoSource, EXPLORE_HANDLERS, INFRA, INVOICE_PAGE, INVOICE_SCHEMA, INVOICE_TOOLS, INVOICING,
    LATENCY_AUDIT, TOWER, TRACE_GATEWAY,
};
use crate::views::hierarchy::Zoom;

fn start() -> Timestamp {
    Timestamp::from_millis(1_793_986_617_000)
}

fn id(member: usize) -> AgentId {
    crate::demo::agent_id(start(), member)
}

fn demo() -> DemoSource {
    DemoSource::new(Arc::new(ManualClock::new(start())))
}

/// A model of the demo at its start, in a terminal of the given size.
fn model(width: u16, height: u16) -> Model {
    let mut model = Model::new(
        Theme::default(),
        Keymap::default(),
        None,
        (width, height),
        start(),
    );
    update(&mut model, Msg::Events(demo().events_until(start())));
    model
}

fn key(model: &mut Model, code: KeyCode) -> Vec<Effect> {
    update(
        model,
        Msg::Key(KeyEvent::new_with_kind(
            code,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
    )
}

fn press(model: &mut Model, keys: &str) -> Vec<Effect> {
    keys.chars()
        .flat_map(|c| key(model, KeyCode::Char(c)))
        .collect()
}

fn ctrl(model: &mut Model, c: char) -> Vec<Effect> {
    update(
        model,
        Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)),
    )
}

fn click(model: &mut Model, column: u16, row: u16) -> Vec<Effect> {
    update(
        model,
        Msg::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }),
    )
}

fn names(model: &Model) -> Vec<String> {
    model
        .rail
        .iter()
        .map(|entry| model.registry.agent(entry.agent).unwrap().name.clone())
        .collect()
}

#[test]
fn the_most_urgent_agent_is_selected_and_previewed_first() {
    let mut model = Model::new(
        Theme::default(),
        Keymap::default(),
        None,
        (120, 40),
        start(),
    );
    let effects = update(&mut model, Msg::Events(demo().events_until(start())));
    assert_eq!(model.selected, Some(id(INVOICE_TOOLS)));
    assert_eq!(effects, [Effect::LoadPreview(id(INVOICE_TOOLS))]);
    assert_eq!(
        names(&model)[..3],
        ["invoice-tools", "invoice-schema", "explore-traces"]
    );
}

#[test]
fn j_and_k_follow_the_list() {
    let mut model = model(120, 40);
    assert_eq!(
        press(&mut model, "j"),
        [Effect::LoadPreview(id(INVOICE_SCHEMA))]
    );
    assert_eq!(model.selected, Some(id(INVOICE_SCHEMA)));
    press(&mut model, "k");
    assert_eq!(model.selected, Some(id(INVOICE_TOOLS)));
    press(&mut model, "k");
    assert_eq!(
        model.selected,
        Some(id(INVOICE_TOOLS)),
        "the top stays the top"
    );
    press(&mut model, "G");
    assert_eq!(model.selected, model.rail.last().map(|entry| entry.agent));
    press(&mut model, "g");
    assert_eq!(model.selected, Some(id(INVOICE_TOOLS)));
}

#[test]
fn h_and_l_go_to_the_parent_and_the_first_child() {
    let mut model = model(120, 40);
    press(&mut model, "h");
    assert_eq!(model.selected, Some(id(INVOICING)));
    press(&mut model, "h");
    assert_eq!(model.selected, Some(id(TOWER)));
    press(&mut model, "l");
    assert_eq!(model.selected, Some(id(INVOICING)));
}

#[test]
fn a_quote_and_a_letter_jump_to_an_agent() {
    let mut model = model(120, 40);
    assert_eq!(model.letters.of(id(TOWER)), Some('t'));
    press(&mut model, "'t");
    assert_eq!(model.selected, Some(id(TOWER)));
    assert!(!model.jump_pending);
    press(&mut model, "'z");
    assert_eq!(model.selected, Some(id(TOWER)));
    assert!(model.notice.as_ref().unwrap().text.contains("letter z"));
    press(&mut model, "'");
    key(&mut model, KeyCode::Esc);
    assert!(!model.jump_pending);
    assert_eq!(model.selected, Some(id(TOWER)), "escape cancels the jump");
}

#[test]
fn minus_goes_back_and_forth() {
    let mut model = model(120, 40);
    press(&mut model, "'t");
    press(&mut model, "'b");
    assert_eq!(model.selected, Some(id(crate::demo::BROKEN_LINKS)));
    press(&mut model, "-");
    assert_eq!(model.selected, Some(id(TOWER)));
    press(&mut model, "-");
    assert_eq!(model.selected, Some(id(crate::demo::BROKEN_LINKS)));
}

#[test]
fn the_filter_narrows_the_list_live_and_escape_clears_it() {
    let mut model = model(120, 40);
    press(&mut model, "/invoic");
    assert!(model.filter.editing);
    assert_eq!(
        names(&model),
        [
            "invoice-tools",
            "invoice-schema",
            "invoice-page",
            "invoicing"
        ]
    );
    // While typing, letters are text, not commands.
    press(&mut model, "q");
    assert_eq!(model.filter.text, "invoicq");
    key(&mut model, KeyCode::Backspace);
    key(&mut model, KeyCode::Enter);
    assert!(!model.filter.editing);
    assert_eq!(model.rail.len(), 4);
    // The mission's title matches too.
    press(&mut model, "/");
    ctrl(&mut model, 'u');
    press(&mut model, "latency");
    key(&mut model, KeyCode::Enter);
    assert_eq!(model.rail.len(), 5);
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.rail.len(), 12);
}

#[test]
fn jumping_to_a_hidden_agent_clears_the_filter() {
    let mut model = model(120, 40);
    press(&mut model, "/invoice");
    key(&mut model, KeyCode::Enter);
    press(&mut model, "'t");
    assert_eq!(model.selected, Some(id(TOWER)));
    assert_eq!(model.filter.text, "");
}

#[test]
fn escape_closes_one_thing_at_a_time_and_never_quits() {
    let mut model = model(120, 40);
    key(&mut model, KeyCode::Tab);
    press(&mut model, "/inv");
    key(&mut model, KeyCode::Enter);
    press(&mut model, "'n");
    key(&mut model, KeyCode::Enter);
    press(&mut model, "?");
    assert_eq!(model.overlay, Some(Overlay::Help { scroll: 0 }));
    assert_eq!(model.subtree, [id(INVOICING)]);

    assert_eq!(key(&mut model, KeyCode::Esc), []);
    assert_eq!(model.overlay, None, "the help closes first");
    key(&mut model, KeyCode::Esc);
    assert!(model.subtree.is_empty(), "then the subtree");
    key(&mut model, KeyCode::Esc);
    assert!(model.filter.text.is_empty(), "then the filter");
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.focus, Pane::Rail, "then the focus comes home");
    assert!(!model.can_go_back());
    for _ in 0..5 {
        assert!(!key(&mut model, KeyCode::Esc).contains(&Effect::Quit));
    }
}

#[test]
fn q_quits_at_once_and_ctrl_c_quits_from_prompts() {
    let mut model = model(120, 40);
    assert_eq!(press(&mut model, "q"), [Effect::Quit]);
    press(&mut model, "/");
    assert_eq!(ctrl(&mut model, 'c'), [Effect::Quit]);
}

#[test]
fn tab_cycles_through_the_panels_on_screen() {
    let mut model = model(120, 40);
    let mut seen = vec![model.focus];
    for _ in 0..3 {
        key(&mut model, KeyCode::Tab);
        seen.push(model.focus);
    }
    assert_eq!(seen, [Pane::Rail, Pane::View, Pane::Detail, Pane::Rail]);
    key(&mut model, KeyCode::BackTab);
    assert_eq!(model.focus, Pane::Detail);

    let mut narrow = model_at(80, 24);
    key(&mut narrow, KeyCode::Tab);
    assert_eq!(narrow.focus, Pane::Detail, "no view panel on small screens");
}

fn model_at(width: u16, height: u16) -> Model {
    model(width, height)
}

#[test]
fn numbers_switch_tabs_and_the_keys_follow_the_panel() {
    let mut model = model(120, 40);
    key(&mut model, KeyCode::Tab);
    assert_eq!(model.contexts(), [Context::Hierarchy, Context::Global]);
    press(&mut model, "2");
    assert_eq!(model.tab, Tab::Network);
    assert_eq!(model.contexts(), [Context::Global]);
    press(&mut model, "1");
    assert_eq!(model.tab, Tab::Hierarchy);
}

#[test]
fn a_takes_over_the_selected_agent() {
    let mut model = model(120, 40);
    press(&mut model, "'s");
    assert_eq!(
        press(&mut model, "a"),
        [Effect::TakeOver(id(INVOICE_SCHEMA))]
    );
    let effects = update(
        &mut model,
        Msg::Resumed {
            agent: id(INVOICE_SCHEMA),
            result: Ok(()),
        },
    );
    assert_eq!(
        model.selected,
        Some(id(INVOICE_SCHEMA)),
        "back on the same row"
    );
    assert_eq!(effects, [Effect::LoadPreview(id(INVOICE_SCHEMA))]);
    assert_eq!(
        model.notice.as_ref().unwrap().text,
        "Back from invoice-schema."
    );
    update(&mut model, Msg::TakeOverUnavailable(id(INFRA)));
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.error);
    assert_eq!(
        notice.text,
        "Cannot take over infra: it runs in another terminal; switch to it there."
    );
    update(&mut model, Msg::TakeOverUnavailable(id(EXPLORE_HANDLERS)));
    assert!(
        model
            .notice
            .as_ref()
            .unwrap()
            .text
            .ends_with("inside its parent's session.")
    );
}

#[test]
fn enter_focuses_a_subtree_until_the_selection_leaves_it() {
    let mut model = model(120, 40);
    press(&mut model, "'l");
    key(&mut model, KeyCode::Enter);
    assert_eq!(model.subtree, [id(LATENCY_AUDIT)]);
    assert_eq!(model.focus, Pane::View);
    assert_eq!(model.scene.nodes.len(), 3);
    press(&mut model, "j");
    assert!(model.subtree.contains(&id(LATENCY_AUDIT)));
    press(&mut model, "'t");
    assert_eq!(model.subtree, []);
    assert_eq!(model.scene.nodes.len(), 10);
}

#[test]
fn view_keys_move_through_the_picture() {
    let mut model = model(200, 50);
    press(&mut model, "'t");
    key(&mut model, KeyCode::Tab);
    assert_eq!(model.focus, Pane::View);
    press(&mut model, "j");
    let below_tower = model.selected.unwrap();
    assert!(below_tower == id(INVOICING) || below_tower == id(LATENCY_AUDIT));
    press(&mut model, "k");
    assert_eq!(model.selected, Some(id(TOWER)));
    press(&mut model, "'g");
    press(&mut model, "j");
    assert_eq!(
        model.selected,
        Some(id(EXPLORE_HANDLERS)),
        "satellites are below"
    );
}

#[test]
fn zoom_keys_override_the_automatic_level() {
    let mut model = model(200, 50);
    assert_eq!(model.scene.zoom, Zoom::Cards);
    key(&mut model, KeyCode::Tab);
    press(&mut model, "_");
    assert_eq!(model.zoom, ZoomChoice::Fixed(Zoom::Compact));
    press(&mut model, "_");
    assert_eq!(model.scene.zoom, Zoom::Dots);
    press(&mut model, "_");
    assert_eq!(model.scene.zoom, Zoom::Dots);
    press(&mut model, "+");
    assert_eq!(model.scene.zoom, Zoom::Compact);
    press(&mut model, "0");
    assert_eq!(model.zoom, ZoomChoice::Auto);
    assert_eq!(model.scene.zoom, Zoom::Cards);
}

#[test]
fn small_screens_show_the_hierarchy_in_the_list() {
    let mut model = model(120, 40);
    key(&mut model, KeyCode::Tab);
    update(
        &mut model,
        Msg::Resize {
            width: 80,
            height: 24,
        },
    );
    assert_eq!(model.layout.mode, Mode::Narrow);
    assert_eq!(model.focus, Pane::Rail);
    assert_eq!(model.rail_order, RailOrder::Tree);
    assert_eq!(names(&model)[..3], ["tower", "invoicing", "invoice-schema"]);
    assert_eq!(model.rail[2].guides, "│  ├─ ");
    press(&mut model, "2");
    assert_eq!(model.rail_order, RailOrder::Urgency);
}

#[test]
fn the_selection_does_not_move_the_panels() {
    let mut model = model(120, 40);
    let layout = model.layout;
    let scene = model.scene.clone();
    for keys in ["j", "j", "'t", "'p", "G"] {
        press(&mut model, keys);
        assert_eq!(model.layout, layout);
        assert_eq!(model.scene, scene);
    }
}

#[test]
fn clicks_select_agents_and_tabs() {
    let mut model = model(120, 40);
    let (area, hit) = model
        .hits
        .iter()
        .find(|(_, hit)| matches!(hit, Hit::Agent(agent, Pane::View) if *agent == id(INVOICE_PAGE)))
        .copied()
        .unwrap();
    click(&mut model, area.x + 1, area.y);
    assert_eq!(model.selected, Some(id(INVOICE_PAGE)));
    assert_eq!(model.focus, Pane::View);
    assert!(matches!(hit, Hit::Agent(..)));

    let (rail_row, _) = model
        .hits
        .iter()
        .find(|(_, hit)| matches!(hit, Hit::Agent(agent, Pane::Rail) if *agent == id(TOWER)))
        .copied()
        .unwrap();
    click(&mut model, rail_row.x, rail_row.y);
    assert_eq!(model.selected, Some(id(TOWER)));
    assert_eq!(model.focus, Pane::Rail);

    let (tab, _) = model
        .hits
        .iter()
        .find(|(_, hit)| *hit == Hit::Tab(Tab::Timeline))
        .copied()
        .unwrap();
    click(&mut model, tab.x, tab.y);
    assert_eq!(model.tab, Tab::Timeline);
}

#[test]
fn the_wheel_scrolls_the_view_without_moving_the_selection() {
    let mut model = model(120, 40);
    key(&mut model, KeyCode::Tab);
    press(&mut model, "+");
    let selected = model.selected;
    let before = model.scene_offset;
    let view = model.layout.view.unwrap();
    update(
        &mut model,
        Msg::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: view.x + 5,
            row: view.y + 5,
            modifiers: KeyModifiers::NONE,
        }),
    );
    assert_eq!(model.selected, selected);
    assert_eq!(model.scene_offset.1, before.1 + 2);
}

#[test]
fn previews_are_kept_for_the_selected_agent_only() {
    let mut model = model(120, 40);
    let tools = id(INVOICE_TOOLS);
    update(
        &mut model,
        Msg::PreviewLoaded {
            agent: id(TOWER),
            result: Ok("stale".into()),
        },
    );
    assert!(model.preview.screen.is_none());
    update(
        &mut model,
        Msg::PreviewLoaded {
            agent: tools,
            result: Ok("\x1b[1mhello\x1b[0m".into()),
        },
    );
    assert!(!model.preview.loading);
    assert_eq!(model.preview.screen.as_ref().unwrap().text()[0], "hello");
}

#[test]
fn ticks_animate_expire_notices_and_refresh_busy_previews() {
    let mut model = model(120, 40);
    press(&mut model, "'t");
    update(
        &mut model,
        Msg::PreviewLoaded {
            agent: id(TOWER),
            result: Ok(String::new()),
        },
    );
    press(&mut model, "'z");
    assert!(model.notice.is_some());
    let effects = update(&mut model, Msg::Tick(start().plus_millis(40)));
    assert!(effects.is_empty(), "too early to refresh");
    let effects = update(&mut model, Msg::Tick(start().plus_millis(5_000)));
    assert_eq!(effects, [Effect::LoadPreview(id(TOWER))]);
    assert!(model.notice.is_none());
    assert!(model.is_animating());
}

#[test]
fn forgotten_agents_leave_the_list_and_free_their_letter() {
    let mut model = model(120, 40);
    press(&mut model, "'p");
    update(
        &mut model,
        Msg::Events(vec![Event::new(
            start().plus_millis(1_000),
            EventKind::AgentForgotten {
                agent: id(INVOICE_PAGE),
            },
        )]),
    );
    assert_eq!(model.rail.len(), 11);
    assert_eq!(model.letters.agent('p'), None);
    assert_ne!(model.selected, Some(id(INVOICE_PAGE)));
    assert!(model.selected.is_some());
}

#[test]
fn bad_events_are_ignored() {
    let mut model = model(120, 40);
    let before = model.registry.agents().count();
    update(
        &mut model,
        Msg::Events(vec![Event::new(
            start(),
            EventKind::ActivityRecorded {
                agent: AgentId::from_ulid(ulid::Ulid::from_parts(1, 1)),
                summary: "ghost".into(),
            },
        )]),
    );
    assert_eq!(model.registry.agents().count(), before);
    let gateway = model.registry.agent(id(TRACE_GATEWAY)).unwrap();
    assert_eq!(gateway.name, "trace-gateway");
}
