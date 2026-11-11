//! `update` without a terminal: keys, mouse, time and events in; model and
//! effects out.

use std::sync::Arc;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use volee_core::AgentId;
use volee_core::time::Timestamp;
use volee_core::workspace::Location;

use super::*;
use crate::clock::ManualClock;
use crate::demo::{
    DemoSource, EXPLORE_HANDLERS, INFRA, INVOICE_PAGE, INVOICE_SCHEMA, INVOICE_TOOLS, INVOICING,
    LATENCY_AUDIT, TOWER, TRACE_GATEWAY,
};
use crate::source::{Changes, Door, Outgoing, PaneFocus, SourceError, TakeOver};
use crate::views::hierarchy::Zoom;
use crate::views::timeline::Scale;

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
    press(&mut model, "gg");
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
    assert_eq!(model.prompt, None);
    press(&mut model, "'z");
    assert_eq!(model.selected, Some(id(TOWER)));
    assert!(model.notice.as_ref().unwrap().text.contains("letter z"));
    press(&mut model, "'");
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.prompt, None);
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
    press(&mut model, "s");
    press(&mut model, "?");
    assert_eq!(model.overlay, Some(Overlay::Keys));
    assert_eq!(model.subtree, [id(INVOICING)]);

    assert_eq!(key(&mut model, KeyCode::Esc), []);
    assert_eq!(model.overlay, None, "the keys popup closes first");
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
fn the_keys_popup_the_leader_and_vim_keys() {
    let mut model = model(120, 40);
    // `?` shows this screen's keys; another key closes it and acts.
    press(&mut model, "?");
    assert_eq!(model.overlay, Some(Overlay::Keys));
    press(&mut model, "j");
    assert_eq!(model.overlay, None);
    assert_eq!(model.selected, Some(id(INVOICE_SCHEMA)));
    // `? ?` lists every key.
    press(&mut model, "??");
    assert_eq!(model.overlay, Some(Overlay::Help { scroll: 0 }));
    key(&mut model, KeyCode::Esc);
    // A sequence waits for its next key, and Escape drops it.
    press(&mut model, "g");
    assert_eq!(model.pending_keys, [KeyCombo::char('g')]);
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.pending_keys, Vec::<KeyCombo>::new());
    assert_eq!(model.selected, Some(id(INVOICE_SCHEMA)), "nothing moved");
    press(&mut model, "gg");
    assert_eq!(model.selected, Some(id(INVOICE_TOOLS)));
    // The leader opens the launch dialog; `n` no longer does.
    press(&mut model, "n");
    assert!(model.overlay.is_none(), "n is the next match");
    press(&mut model, " n");
    assert!(matches!(model.overlay, Some(Overlay::Launch(_))));
    key(&mut model, KeyCode::Esc);
    // `n` and `N` go round the filtered list.
    press(&mut model, "/inv");
    key(&mut model, KeyCode::Enter);
    let first = model.selected;
    press(&mut model, "n");
    assert_ne!(model.selected, first);
    press(&mut model, "N");
    assert_eq!(model.selected, first);
    // `L` and `H` go through the views.
    assert_eq!(model.tab, Tab::Hierarchy);
    press(&mut model, "L");
    assert_eq!(model.tab, Tab::Network);
    press(&mut model, "HH");
    assert_eq!(model.tab, Tab::Timeline);
}

#[test]
fn the_palette_runs_a_command_by_name() {
    let mut model = model(120, 40);
    press(&mut model, ":");
    assert!(matches!(model.overlay, Some(Overlay::Commands(_))));
    press(&mut model, "show-repos");
    key(&mut model, KeyCode::Enter);
    assert_eq!(model.overlay, None);
    assert_eq!(model.tab, Tab::Repos);
    press(&mut model, ":nothing-like-this");
    key(&mut model, KeyCode::Enter);
    assert_eq!(model.tab, Tab::Repos);
}

#[test]
fn the_theme_picker_shows_themes_live_and_keeps_or_restores() {
    let mut model = model(120, 40);
    let names = ["catppuccin-mocha", "catppuccin-latte"];
    let themes = names
        .iter()
        .map(|name| ((*name).to_owned(), Theme::load(name, None).unwrap()))
        .collect();
    model.themes = crate::themes::ThemeMenu::new(themes, "catppuccin-mocha");
    let mocha = model.theme.clone();
    press(&mut model, " t");
    assert!(matches!(model.overlay, Some(Overlay::Themes { .. })));
    press(&mut model, "j");
    assert_ne!(model.theme, mocha, "shown at once");
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.theme, mocha, "Escape restores it");
    press(&mut model, " tj");
    let effects = key(&mut model, KeyCode::Enter);
    assert_eq!(effects, [Effect::SaveTheme("catppuccin-latte".into())]);
    assert_eq!(model.overlay, None);
    // The palette picks a theme by name too.
    press(&mut model, ":theme mocha");
    let effects = key(&mut model, KeyCode::Enter);
    assert_eq!(effects, [Effect::SaveTheme("catppuccin-mocha".into())]);
    assert_eq!(model.theme, mocha);
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
    assert_eq!(model.contexts(), [Context::Network, Context::Global]);
    press(&mut model, "3");
    assert_eq!(model.contexts(), [Context::Repos, Context::Global]);
    press(&mut model, "4");
    assert_eq!(model.contexts(), [Context::Timeline, Context::Global]);
    press(&mut model, "1");
    assert_eq!(model.tab, Tab::Hierarchy);
}

/// Moves an agent into a volee host, as its adoption does.
fn hosted(model: &mut Model, agent: AgentId) {
    update(
        model,
        Msg::Events(vec![Event::new(
            Timestamp::from_millis(1_793_986_618_000),
            EventKind::LocationChanged {
                agent,
                location: Location::Host { id: "h1".into() },
            },
        )]),
    );
}

#[test]
fn adopting_asks_first_and_says_what_it_costs() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    // `A` moves a session into a volee host: asked first, never needed to
    // open it.
    assert_eq!(press(&mut model, "A"), []);
    assert_eq!(model.prompt, Some(Prompt::AdoptAndTakeOver(schema)));
    // No: nothing happens.
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.prompt, None);
    // Yes: it is adopted, then taken over once the adoption is done.
    press(&mut model, "A");
    let effects = press(&mut model, "y");
    assert_eq!(
        effects,
        [Effect::Adopt {
            agent: schema,
            name: "invoice-schema".into()
        }]
    );
    let effects = update(
        &mut model,
        Msg::Adopted {
            agent: schema,
            name: "invoice-schema".into(),
            result: Ok(()),
        },
    );
    assert_eq!(effects, [Effect::PrepareTakeOver(schema)]);
    assert_eq!(model.pending_take_over, Some(schema));
    // An agent already in a volee host has nothing to adopt.
    update(
        &mut model,
        Msg::TakeOverReady {
            agent: schema,
            result: Err(SourceError::Failed("no".into())),
        },
    );
    hosted(&mut model, schema);
    assert_eq!(press(&mut model, "A"), []);
    assert_eq!(model.prompt, None);
    assert_eq!(
        model.notice.as_ref().unwrap().text,
        "invoice-schema already runs in a volee host."
    );
}

#[test]
fn a_failed_adoption_takes_nothing_over() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    press(&mut model, "Ay");
    let effects = update(
        &mut model,
        Msg::Adopted {
            agent: schema,
            name: "invoice-schema".into(),
            result: Err(SourceError::Failed("no".into())),
        },
    );
    assert_eq!(effects, []);
    assert_eq!(model.pending_take_over, None);
    assert_eq!(model.take_over_after_adopt, None);
    assert!(model.notice.as_ref().is_some_and(|notice| notice.error));
}

/// The panel an agent's terminal gets in a 120x40 interface.
const PANEL: volee_host::Size = volee_host::Size {
    cols: 117,
    rows: 37,
};

fn view_socket() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/volee-test/views/view-1.sock")
}

#[test]
fn enter_i_and_a_open_the_selected_agent_where_it_runs() {
    let schema = id(INVOICE_SCHEMA);
    let open = [Effect::Open {
        agent: schema,
        size: PANEL,
    }];
    for keys in ["i", "a"] {
        let mut model = model(120, 40);
        press(&mut model, "'s");
        assert_eq!(press(&mut model, keys), open, "{keys}");
    }
    let mut model = model(120, 40);
    press(&mut model, "'s");
    // The demo's agents run in the background: no prompt, no adoption, the
    // source is asked for a terminal on the session.
    assert_eq!(key(&mut model, KeyCode::Enter), open);
    assert_eq!(model.prompt, None);
    assert!(press(&mut model, "a").is_empty(), "one at a time");

    // The source opened a view on it: edit mode, on that socket.
    let effects = update(
        &mut model,
        Msg::Opened {
            agent: schema,
            result: Ok(Door::Host {
                socket: view_socket(),
                view: true,
            }),
        },
    );
    assert!(
        effects.contains(&Effect::LiveOpen {
            link: 1,
            socket: view_socket(),
            edit: true,
            size: PANEL,
        }),
        "{effects:?}"
    );
    assert_eq!(model.overlay, Some(Overlay::Screen));
    update(
        &mut model,
        Msg::LiveOpened {
            link: 1,
            welcome: crate::test_support::welcome(PANEL, true),
        },
    );
    assert!(crate::edit::editing(&model));

    // The leave key closes the view and comes back to the list; the agent
    // keeps running where it runs.
    let effects = update(
        &mut model,
        Msg::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
    );
    assert_eq!(effects, [Effect::LiveClose { link: 1 }]);
    assert_eq!(model.overlay, None);
    assert_eq!(model.door, None);
    assert!(model.live.is_none());
    assert_eq!(
        model.notice.as_ref().unwrap().text,
        "Left invoice-schema: it keeps running."
    );
    assert_eq!(model.selected, Some(schema), "back on the same row");

    // Opened again: the source is asked again, and gives the view that runs
    // or a new one.
    assert_eq!(key(&mut model, KeyCode::Enter), open);
}

#[test]
fn an_agent_with_no_door_says_what_to_do() {
    let mut model = model(120, 40);
    press(&mut model, "'i");
    let infra = id(INFRA);
    assert_eq!(
        press(&mut model, "i"),
        [Effect::Open {
            agent: infra,
            size: PANEL
        }]
    );
    update(
        &mut model,
        Msg::Opened {
            agent: infra,
            result: Err(SourceError::Unsupported(
                "infra runs in a terminal volee cannot reach: switch to that terminal to use it."
                    .into(),
            )),
        },
    );
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.error);
    assert_eq!(
        notice.text,
        "Cannot open infra: infra runs in a terminal volee cannot reach: switch to that \
         terminal to use it."
    );
    assert_eq!(model.overlay, None);
    assert_eq!(model.opening, None, "the next one can open");
}

#[test]
fn a_view_that_ends_comes_back_to_the_list() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    press(&mut model, "i");
    let opened = || Msg::Opened {
        agent: schema,
        result: Ok(Door::Host {
            socket: view_socket(),
            view: true,
        }),
    };
    update(&mut model, opened());
    update(
        &mut model,
        Msg::LiveOpened {
            link: 1,
            welcome: crate::test_support::welcome(PANEL, true),
        },
    );
    // The person leaves through the door itself (Ctrl+Z in `claude
    // attach`): the door's program ends well, the agent goes on.
    let exited = |code| Msg::LiveUpdate {
        link: 1,
        update: volee_host::Update::Event(volee_host::Event::Exited {
            exit: volee_host::Exit {
                code: Some(code),
                signal: None,
                at: 0,
            },
        }),
    };
    update(&mut model, exited(0));
    assert_eq!(model.overlay, None);
    assert_eq!(
        model.notice.as_ref().unwrap().text,
        "Left invoice-schema: it keeps running."
    );

    // A door that fails stays on screen, to be read.
    press(&mut model, "i");
    update(&mut model, opened());
    update(
        &mut model,
        Msg::LiveOpened {
            link: 2,
            welcome: crate::test_support::welcome(PANEL, true),
        },
    );
    let failed = Msg::LiveUpdate {
        link: 2,
        update: volee_host::Update::Event(volee_host::Event::Exited {
            exit: volee_host::Exit {
                code: Some(1),
                signal: None,
                at: 0,
            },
        }),
    };
    update(&mut model, failed);
    assert_eq!(model.overlay, Some(Overlay::Screen));
    assert!(!crate::edit::editing(&model));
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.error);
    assert!(
        notice.text.contains("closed with an error"),
        "{}",
        notice.text
    );
    // Its connection closing changes nothing more.
    update(
        &mut model,
        Msg::LiveClosed {
            link: 2,
            reason: "the host closed the connection".into(),
        },
    );
    assert_eq!(model.overlay, Some(Overlay::Screen));
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.overlay, None);
    assert_eq!(model.door, None);
}

#[test]
fn a_view_that_does_not_answer_is_said_plainly() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    press(&mut model, "i");
    update(
        &mut model,
        Msg::Opened {
            agent: schema,
            result: Ok(Door::Host {
                socket: view_socket(),
                view: true,
            }),
        },
    );
    update(
        &mut model,
        Msg::LiveClosed {
            link: 1,
            reason: "No such file or directory (os error 2)".into(),
        },
    );
    assert_eq!(model.overlay, None);
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.error);
    assert!(
        notice
            .text
            .starts_with("Cannot open invoice-schema: its terminal does not answer"),
        "{}",
        notice.text
    );
}

#[test]
fn a_door_that_closed_at_once_shows_what_it_said() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    press(&mut model, "i");
    update(
        &mut model,
        Msg::Opened {
            agent: schema,
            result: Ok(Door::Host {
                socket: view_socket(),
                view: true,
            }),
        },
    );
    // The session was removed meanwhile: its door said so and ended.
    let mut welcome = crate::test_support::welcome(PANEL, true);
    welcome.input = false;
    welcome.exit = Some(volee_host::Exit {
        code: Some(1),
        signal: None,
        at: 0,
    });
    update(&mut model, Msg::LiveOpened { link: 1, welcome });
    assert_eq!(
        model.overlay,
        Some(Overlay::Screen),
        "its last screen stays"
    );
    assert!(!crate::edit::editing(&model));
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.error);
    assert!(notice.text.contains("closed at once"), "{}", notice.text);
}

#[test]
fn an_answer_nobody_waits_for_opens_nothing() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    let effects = update(
        &mut model,
        Msg::Opened {
            agent: schema,
            result: Ok(Door::Host {
                socket: view_socket(),
                view: true,
            }),
        },
    );
    assert_eq!(effects, []);
    assert_eq!(model.overlay, None);
    assert_eq!(model.door, None);
}

#[test]
fn a_double_click_opens_an_agent_and_a_single_one_selects_it() {
    let mut model = model(120, 40);
    let click = |model: &mut Model, row: u16| {
        update(
            model,
            Msg::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 3,
                row,
                modifiers: KeyModifiers::NONE,
            }),
        )
    };
    // The rows of the list, under the top bar and the panel's title.
    let first = model.rail[0].agent;
    let row = (0..model.size.1)
        .find(|row| matches!(model.hit(3, *row), Some(Hit::Agent(agent, _)) if agent == first))
        .expect("a row of the list");
    let effects = click(&mut model, row);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Open { .. })),
        "one click selects"
    );
    assert_eq!(model.selected, Some(first));
    let effects = click(&mut model, row);
    assert!(
        effects.contains(&Effect::Open {
            agent: first,
            size: PANEL
        }),
        "{effects:?}"
    );
    // Two clicks far apart are two single clicks.
    update(
        &mut model,
        Msg::Opened {
            agent: first,
            result: Err(SourceError::Unsupported("no".into())),
        },
    );
    click(&mut model, row);
    let long_ago = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(2))
        .unwrap();
    model.last_click = Some((first, long_ago));
    let effects = click(&mut model, row);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Open { .. }))
    );
}

#[test]
fn a_pane_only_its_multiplexer_shows_is_taken_over_the_old_way() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    press(&mut model, "a");
    let effects = update(
        &mut model,
        Msg::Opened {
            agent: schema,
            result: Ok(Door::Elsewhere),
        },
    );
    assert_eq!(effects, [Effect::PrepareTakeOver(schema)]);
    let take_over = TakeOver {
        program: "claude".into(),
        args: vec!["attach".into(), "7f3k2m9q".into()],
        cwd: None,
        env_remove: vec!["CLAUDE_CODE_SESSION_ID".into()],
        hint: Some("Press Ctrl+Z to come back.".into()),
        focus: None,
    };
    let effects = update(
        &mut model,
        Msg::TakeOverReady {
            agent: schema,
            result: Ok(take_over.clone()),
        },
    );
    let said = TakeOver {
        hint: Some("Taking over invoice-schema. Press Ctrl+Z to come back.".into()),
        ..take_over
    };
    assert_eq!(
        effects,
        [Effect::TakeOver {
            agent: schema,
            take_over: said
        }]
    );
    let effects = update(
        &mut model,
        Msg::Resumed {
            agent: schema,
            result: Ok(()),
        },
    );
    assert_eq!(model.selected, Some(schema), "back on the same row");
    assert_eq!(effects, [Effect::LoadPreview(schema)]);
    assert_eq!(
        model.notice.as_ref().unwrap().text,
        "Back from invoice-schema."
    );

    update(
        &mut model,
        Msg::TakeOverReady {
            agent: id(INFRA),
            result: Err(SourceError::Unsupported(
                "it runs in another terminal; switch to it there".into(),
            )),
        },
    );
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.error);
    assert_eq!(
        notice.text,
        "Cannot take over infra: it runs in another terminal; switch to it there."
    );
}

#[test]
fn z_shows_the_agent_screen_full_size_and_comes_back() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    assert_eq!(press(&mut model, "z"), [Effect::LoadPreview(schema)]);
    assert_eq!(model.overlay, Some(Overlay::Screen));
    assert_eq!(model.contexts(), [Context::Screen]);
    // Scrolling stays within the screen; G goes back to the newest lines.
    let before = model.preview.scroll;
    press(&mut model, "k");
    assert!(model.preview.scroll >= before);
    press(&mut model, "G");
    assert_eq!(model.preview.scroll, 0);
    // z, q or esc come back.
    press(&mut model, "z");
    assert_eq!(model.overlay, None);
    press(&mut model, "z");
    assert_eq!(key(&mut model, KeyCode::Esc), []);
    assert_eq!(model.overlay, None);
}

#[test]
fn the_full_screen_view_scrolls_over_wrapped_lines() {
    use std::fmt::Write as _;
    // 120x40: the screen's text takes 117 columns and 37 rows.
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    press(&mut model, "z");
    // Thirty rows of 234 characters, two lines each, then the cursor's row.
    let mut text = String::new();
    for n in 0..30 {
        write!(text, "{n:02}{}\r\n", "x".repeat(232)).unwrap();
    }
    update(
        &mut model,
        Msg::PreviewLoaded {
            agent: schema,
            result: Ok(text),
        },
    );
    for _ in 0..40 {
        press(&mut model, "k");
    }
    assert_eq!(model.preview.scroll, 61 - 37, "up to the first line");
    // Back in the main view, the preview starts from its newest rows.
    press(&mut model, "z");
    assert_eq!(model.overlay, None);
    assert_eq!(model.preview.scroll, 0);
}

#[test]
fn forgetting_an_agent_asks_first_and_says_how_it_went() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    // F asks; n and Esc leave everything as it was.
    assert_eq!(press(&mut model, "F"), []);
    assert_eq!(model.prompt, Some(Prompt::Forget(schema)));
    assert_eq!(model.contexts(), [Context::Confirm]);
    assert_eq!(press(&mut model, "n"), []);
    assert_eq!(model.prompt, None);
    press(&mut model, "F");
    assert_eq!(key(&mut model, KeyCode::Esc), []);
    assert_eq!(model.prompt, None);
    // y asks the source, with the name for the notice that follows.
    press(&mut model, "F");
    assert_eq!(
        press(&mut model, "y"),
        [Effect::Forget {
            agent: schema,
            name: "invoice-schema".into()
        }]
    );
    assert_eq!(model.prompt, None);
    update(
        &mut model,
        Msg::Forgot {
            name: "invoice-schema".into(),
            result: Ok(()),
        },
    );
    let notice = model.notice.as_ref().unwrap();
    assert!(!notice.error);
    assert_eq!(
        notice.text,
        "Forgot invoice-schema: volee no longer lists it. Its sessions, worktree and branch stay \
         as they are."
    );
    // The daemon's refusal, in its own words.
    update(
        &mut model,
        Msg::Forgot {
            name: "invoice-schema".into(),
            result: Err(SourceError::Unsupported(
                "invoice-schema is still running: stop it first (`volee stop invoice-schema`), \
                 then forget it"
                    .into(),
            )),
        },
    );
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.error);
    assert_eq!(
        notice.text,
        "Could not forget invoice-schema: invoice-schema is still running: stop it first (`volee \
         stop invoice-schema`), then forget it."
    );
    // Ctrl+C still quits from the question.
    press(&mut model, "F");
    assert_eq!(ctrl(&mut model, 'c'), [Effect::Quit]);
}

#[test]
fn the_preview_empties_when_the_last_agent_leaves() {
    let mut model = model(120, 40);
    let ids: Vec<AgentId> = model.registry.agents().map(|agent| agent.id).collect();
    assert!(model.preview.agent.is_some());
    let gone: Vec<Event> = ids
        .into_iter()
        .map(|agent| Event::new(start(), EventKind::AgentForgotten { agent }))
        .collect();
    update(&mut model, Msg::Events(gone));
    assert_eq!(model.selected, None);
    assert_eq!(model.preview.agent, None, "no screen of an agent that left");
}

#[test]
fn an_agent_in_a_pane_is_brought_forward_instead_of_taken_over() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    press(&mut model, "a");
    let effects = update(
        &mut model,
        Msg::Opened {
            agent: schema,
            result: Ok(Door::Elsewhere),
        },
    );
    assert_eq!(effects, [Effect::PrepareTakeOver(schema)]);
    let focus = PaneFocus {
        multiplexer: "tuios".into(),
        session: "work".into(),
        steps: vec![vec!["tuios".into(), "switch-session".into(), "work".into()]],
        attach: vec!["tuios".into(), "attach".into(), "work".into()],
    };
    let effects = update(
        &mut model,
        Msg::TakeOverReady {
            agent: schema,
            result: Ok(TakeOver {
                program: String::new(),
                args: Vec::new(),
                cwd: None,
                env_remove: Vec::new(),
                hint: None,
                focus: Some(focus.clone()),
            }),
        },
    );
    assert_eq!(
        effects,
        [Effect::FocusPane {
            agent: schema,
            focus: focus.clone()
        }]
    );
    update(
        &mut model,
        Msg::PaneFocused {
            agent: schema,
            focus: focus.clone(),
            result: Ok(()),
        },
    );
    let notice = model.notice.as_ref().unwrap();
    assert!(!notice.error);
    assert_eq!(
        notice.text,
        "invoice-schema is in front: tuios pane, session work."
    );
    // The pane is focused in its session, but no tuios window shows it: say
    // where it is, and how to see it.
    update(
        &mut model,
        Msg::PaneFocused {
            agent: schema,
            focus: focus.clone(),
            result: Err(FocusFailure {
                done: 1,
                program: "tuios".into(),
                reason: "switch-session failed: no client is attached to switch".into(),
            }),
        },
    );
    let notice = model.notice.as_ref().unwrap();
    assert!(!notice.error);
    assert_eq!(
        notice.text,
        "invoice-schema is in front in tuios session work: run `tuios attach work` to see it."
    );
    // The pane itself could not be focused.
    update(
        &mut model,
        Msg::PaneFocused {
            agent: schema,
            focus,
            result: Err(FocusFailure {
                done: 0,
                program: "tuios".into(),
                reason: "no window named 3f2a in session work".into(),
            }),
        },
    );
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.error);
    assert_eq!(
        notice.text,
        "Could not bring invoice-schema forward in tuios: tuios says: no window named 3f2a in \
         session work."
    );
}

#[test]
fn m_writes_to_the_selected_agent_and_enter_sends() {
    let mut model = model(120, 40);
    let schema = id(INVOICE_SCHEMA);
    press(&mut model, "'s");
    assert!(press(&mut model, "m").is_empty(), "the composer opens");
    assert_eq!(model.overlay, Some(Overlay::Composer));
    assert_eq!(model.contexts(), [Context::Composer]);
    // Keys that do something elsewhere are text here.
    press(&mut model, "quick: is it a generated column?");
    key(&mut model, KeyCode::Tab);
    let draft = model.draft.as_ref().unwrap();
    assert_eq!(draft.kind, MessageKind::Question);
    assert_eq!(draft.field.text(), "quick: is it a generated column?");
    key(&mut model, KeyCode::BackTab);
    key(&mut model, KeyCode::BackTab);
    assert_eq!(model.draft.as_ref().unwrap().kind, MessageKind::Alert);
    key(&mut model, KeyCode::Tab);
    key(&mut model, KeyCode::Tab);
    assert_eq!(
        key(&mut model, KeyCode::Enter),
        [Effect::Send(Outgoing {
            to: schema,
            kind: MessageKind::Question,
            body: "quick: is it a generated column?".into(),
            in_reply_to: None,
            thread: None,
        })]
    );
    assert!(key(&mut model, KeyCode::Enter).is_empty(), "sent once");
    press(&mut model, "x");
    assert_eq!(
        model.draft.as_ref().unwrap().field.text(),
        "quick: is it a generated column?",
        "no edits while sending"
    );
    update(
        &mut model,
        Msg::MessageSent {
            to: schema,
            result: Ok(()),
        },
    );
    assert_eq!(model.overlay, None);
    assert_eq!(model.draft, None);
    assert_eq!(
        model.notice.as_ref().unwrap().text,
        "Message sent to invoice-schema."
    );
}

#[test]
fn an_agent_that_asked_you_something_gets_an_answer() {
    let mut model = model(120, 40);
    let tools = id(INVOICE_TOOLS);
    press(&mut model, "'v");
    assert_eq!(model.selected, Some(tools));
    press(&mut model, "m");
    let draft = model.draft.as_ref().unwrap();
    assert_eq!(draft.kind, MessageKind::Answer);
    let question = draft.question.unwrap();
    assert!(
        model
            .registry
            .message(question)
            .unwrap()
            .body
            .starts_with("Should create_invoice accept a draft flag")
    );
    press(&mut model, "Yes");
    ctrl(&mut model, 'j');
    update(&mut model, Msg::Paste("false by default\r\n".into()));
    let sent = Outgoing {
        to: tools,
        kind: MessageKind::Answer,
        body: "Yes\nfalse by default".into(),
        in_reply_to: Some(question),
        thread: None,
    };
    assert_eq!(key(&mut model, KeyCode::Enter), [Effect::Send(sent)]);
    update(
        &mut model,
        Msg::MessageSent {
            to: tools,
            result: Err(SourceError::Failed("the daemon is busy.".into())),
        },
    );
    assert_eq!(model.overlay, Some(Overlay::Composer), "still open");
    assert!(!model.draft.as_ref().unwrap().sending);
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.error);
    assert_eq!(
        notice.text,
        "Could not send to invoice-tools: the daemon is busy."
    );
    // Another kind is not an answer: it does not reply to the question.
    key(&mut model, KeyCode::Tab);
    let Some(Effect::Send(note)) = key(&mut model, KeyCode::Enter).pop() else {
        panic!("nothing sent");
    };
    assert_eq!(note.kind, MessageKind::Contract);
    assert_eq!(note.in_reply_to, None);
}

#[test]
fn quitting_asks_first_when_a_draft_would_be_lost() {
    let mut model = model(120, 40);
    press(&mut model, "'s");
    press(&mut model, "m");
    press(&mut model, "half a thought");
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.overlay, None);
    assert!(model.draft.is_some(), "escape keeps the draft");
    assert!(press(&mut model, "q").is_empty(), "q asks first");
    assert_eq!(model.prompt, Some(Prompt::Quit));
    assert_eq!(model.contexts(), [Context::Confirm]);
    assert!(
        press(&mut model, "x").is_empty(),
        "other keys wait for an answer"
    );
    assert!(press(&mut model, "n").is_empty(), "n stays");
    assert_eq!(model.prompt, None);

    // The draft follows the selection when the composer opens again.
    press(&mut model, "'p");
    press(&mut model, "m");
    let draft = model.draft.as_ref().unwrap();
    assert_eq!(draft.to, id(INVOICE_PAGE));
    assert_eq!(draft.field.text(), "half a thought");
    assert!(ctrl(&mut model, 'c').is_empty(), "ctrl-c asks too");
    assert_eq!(press(&mut model, "y"), [Effect::Quit]);

    // An empty draft is not worth a question.
    let mut model = model_at(120, 40);
    press(&mut model, "m");
    press(&mut model, "  ");
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.draft, None);
    assert_eq!(press(&mut model, "q"), [Effect::Quit]);
}

#[test]
fn d_shows_the_changes_and_f_switches_to_the_patch() {
    let mut model = model(120, 40);
    let page = id(INVOICE_PAGE);
    press(&mut model, "'p");
    assert_eq!(
        press(&mut model, "d"),
        [Effect::LoadDiff {
            agent: page,
            full: false
        }]
    );
    assert_eq!(model.contexts(), [Context::Diff]);
    update(
        &mut model,
        Msg::DiffLoaded {
            agent: page,
            full: false,
            result: Ok(Changes {
                text: "line\n".repeat(80),
                truncated: false,
            }),
        },
    );
    let scroll = |model: &Model| match &model.overlay {
        Some(Overlay::Diff(diff)) => diff.scroll,
        other => panic!("no changes window: {other:?}"),
    };
    press(&mut model, "jjj");
    assert_eq!(scroll(&model), 3);
    press(&mut model, "G");
    // 40 rows: a window of 36 rows, 34 inside, for 80 lines.
    assert_eq!(scroll(&model), 46);
    press(&mut model, "jj");
    assert_eq!(scroll(&model), 46, "not past the end");
    assert_eq!(
        press(&mut model, "f"),
        [Effect::LoadDiff {
            agent: page,
            full: true
        }]
    );
    assert_eq!(scroll(&model), 0);
    // A late answer for the summary does not replace the patch.
    update(
        &mut model,
        Msg::DiffLoaded {
            agent: page,
            full: false,
            result: Ok(Changes::default()),
        },
    );
    let Some(Overlay::Diff(diff)) = &model.overlay else {
        panic!("closed");
    };
    assert_eq!(diff.changes, None);
    update(
        &mut model,
        Msg::DiffLoaded {
            agent: page,
            full: true,
            result: Err(SourceError::Unsupported(
                "this volee daemon cannot show changes yet".into(),
            )),
        },
    );
    let Some(Overlay::Diff(diff)) = &model.overlay else {
        panic!("closed");
    };
    assert_eq!(
        diff.error.as_deref(),
        Some("this volee daemon cannot show changes yet")
    );
    press(&mut model, "d");
    assert_eq!(model.overlay, None);
}

#[test]
fn unsupported_previews_and_a_closed_source_stop_the_refresh() {
    let mut model = model(120, 40);
    let tower = id(TOWER);
    press(&mut model, "'t");
    update(
        &mut model,
        Msg::PreviewLoaded {
            agent: tower,
            result: Err(SourceError::Unsupported(
                "this volee daemon cannot show agent screens yet".into(),
            )),
        },
    );
    assert_eq!(
        model.preview.error.as_deref(),
        Some("this volee daemon cannot show agent screens yet")
    );
    let effects = update(&mut model, Msg::Tick(start().plus_millis(5_000)));
    assert!(effects.is_empty(), "not asked again on its own");

    let mut model = model_at(120, 40);
    press(&mut model, "'t");
    update(
        &mut model,
        Msg::SourceClosed(Some("lost the connection to the volee daemon".into())),
    );
    let notice = model.notice.as_ref().unwrap();
    assert!(notice.error);
    assert_eq!(
        notice.text,
        "Lost the connection to the volee daemon: what you see is no longer updated."
    );
    assert!(model.offline.is_some());
    update(
        &mut model,
        Msg::PreviewLoaded {
            agent: tower,
            result: Ok(String::new()),
        },
    );
    let effects = update(&mut model, Msg::Tick(start().plus_millis(5_000)));
    assert!(effects.is_empty(), "offline: {effects:?}");
}

#[test]
fn s_focuses_a_subtree_until_the_selection_leaves_it() {
    let mut model = model(120, 40);
    press(&mut model, "'l");
    press(&mut model, "s");
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
    let mut model = model(200, 52);
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
fn zoom_keys_reach_the_view_from_the_agents_list() {
    let mut model = model(200, 52);
    assert_eq!(model.focus, Pane::Rail);
    press(&mut model, "_");
    assert_eq!(model.zoom, ZoomChoice::Fixed(Zoom::Compact));
    press(&mut model, "2");
    assert_eq!(model.focus, Pane::Rail);
    press(&mut model, "_");
    assert_eq!(model.network.zoom, ZoomChoice::Fixed(Zoom::Compact));
    press(&mut model, "0");
    assert_eq!(model.network.zoom, ZoomChoice::Auto);
    assert_eq!(
        model.zoom,
        ZoomChoice::Fixed(Zoom::Compact),
        "each view keeps its own"
    );
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
fn the_preview_of_an_agent_at_rest_is_read_again_and_not_once_it_ended() {
    let mut model = model(120, 40);
    let resting = model
        .rail
        .iter()
        .map(|entry| entry.agent)
        .find(|agent| {
            let state = &model.registry.agent(*agent).unwrap().state;
            state.is_live() && !state.is_busy()
        })
        .expect("the demo has an agent at rest");
    let mut effects = Vec::new();
    model.select(resting, &mut effects);
    let read = |model: &mut Model| {
        update(
            model,
            Msg::PreviewLoaded {
                agent: resting,
                result: Ok(String::new()),
            },
        );
    };
    let asked = |effects: &[Effect]| effects.contains(&Effect::LoadPreview(resting));
    let tick =
        |model: &mut Model, millis: i64| update(model, Msg::Tick(start().plus_millis(millis)));
    read(&mut model);
    // The agent does nothing, yet its screen may change: a person types in
    // its pane, the last lines of its turn land after the last reading.
    assert!(!asked(&tick(&mut model, 4_000)), "too early");
    assert!(asked(&tick(&mut model, 5_000)));
    // A request under way is not doubled, and the pace starts again from it.
    assert!(!asked(&tick(&mut model, 6_000)));
    read(&mut model);
    assert!(!asked(&tick(&mut model, 9_000)));
    assert!(asked(&tick(&mut model, 10_000)));
    read(&mut model);

    // Once the agent ended, its screen is the one it left: never read again.
    let at = model.now;
    update(
        &mut model,
        Msg::Events(vec![Event::new(
            at,
            EventKind::StateChanged {
                agent: resting,
                state: volee_core::state::AgentState::Stopped,
                detail: None,
            },
        )]),
    );
    assert_eq!(model.selected, Some(resting), "it stays selected");
    for millis in [15_000, 30_000, 120_000] {
        assert!(!asked(&tick(&mut model, millis)));
    }
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

/// A model of the demo `at` a moment, as if the interface was open then.
fn model_at_time(width: u16, height: u16, at: Timestamp) -> Model {
    let mut model = Model::new(
        Theme::default(),
        Keymap::default(),
        None,
        (width, height),
        at,
    );
    update(&mut model, Msg::Events(demo().events_until(at)));
    model
}

#[test]
fn network_arrows_follow_the_picture_and_its_zoom_is_its_own() {
    let mut model = model(200, 50);
    press(&mut model, "2");
    key(&mut model, KeyCode::Tab);
    assert_eq!(model.focus, Pane::View);
    press(&mut model, "'g");
    let rect = |model: &Model, agent| {
        model
            .network
            .targets()
            .into_iter()
            .find(|(id, _)| *id == agent)
            .unwrap()
            .1
    };
    let gateway = rect(&model, id(TRACE_GATEWAY));
    press(&mut model, "h");
    let left = model.selected.unwrap();
    assert!(rect(&model, left).right() <= gateway.x, "h goes left");
    press(&mut model, "'g");
    press(&mut model, "k");
    let above = model.selected.unwrap();
    assert!(rect(&model, above).bottom() <= gateway.y, "k goes up");
    assert!(
        model.rail.iter().any(|entry| entry.agent == above),
        "the list shows the same selection"
    );
    press(&mut model, "_");
    assert_eq!(model.network.zoom, ZoomChoice::Fixed(Zoom::Compact));
    assert_eq!(model.zoom, ZoomChoice::Auto, "the hierarchy keeps its zoom");
    press(&mut model, "0");
    assert_eq!(model.network.zoom, ZoomChoice::Auto);
    press(&mut model, "gg");
    let corner = model.selected.unwrap();
    assert!(
        model.network.targets().iter().all(
            |(_, other)| (other.y, other.x) >= (rect(&model, corner).y, rect(&model, corner).x)
        )
    );
}

#[test]
fn clicks_on_the_network_and_the_timeline_select_agents() {
    let mut model = model(120, 40);
    press(&mut model, "2");
    let (node, _) = model
        .hits
        .iter()
        .find(|(_, hit)| *hit == Hit::Agent(id(INFRA), Pane::View))
        .copied()
        .unwrap();
    click(&mut model, node.x, node.y);
    assert_eq!(model.selected, Some(id(INFRA)));
    press(&mut model, "4");
    let (lane, _) = model
        .hits
        .iter()
        .find(|(_, hit)| *hit == Hit::Lane(id(TOWER)))
        .copied()
        .unwrap();
    click(&mut model, lane.x + 10, lane.y);
    assert_eq!(model.selected, Some(id(TOWER)));
    assert_eq!(model.focus, Pane::View);
    let cursor = model.timeline.cursor.unwrap();
    assert_eq!(
        model.timeline.window(model.now).column_of(cursor),
        Some(10),
        "the cursor moved to the clicked column"
    );
}

#[test]
fn the_timeline_scrubs_jumps_to_events_and_escape_comes_back() {
    let mut model = model(120, 40);
    press(&mut model, "4");
    key(&mut model, KeyCode::Tab);
    press(&mut model, "'v");
    press(&mut model, "h");
    assert!(model.timeline.cursor.is_some());
    assert!(model.can_go_back());
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.timeline.cursor, None, "escape goes back to now");
    assert_eq!(model.focus, Pane::View, "before it leaves the panel");
    press(&mut model, "[");
    let last = model.timeline.cursor.unwrap();
    press(&mut model, "[");
    let before = model.timeline.cursor.unwrap();
    assert!(before < last);
    press(&mut model, "]");
    assert_eq!(model.timeline.cursor, Some(last));
    press(&mut model, "]");
    assert_eq!(model.timeline.cursor, None, "past the last event, now");
    press(&mut model, "+");
    assert!(matches!(model.timeline.scale, Scale::Fixed(_)));
    press(&mut model, "h0");
    assert_eq!(model.timeline.scale, Scale::Auto);
    assert_eq!(model.timeline.cursor, None);
    // Lanes follow the hierarchy, so they never jump around.
    press(&mut model, "gg");
    assert_eq!(model.selected, Some(id(TOWER)));
    press(&mut model, "j");
    assert_eq!(model.selected, Some(id(INVOICING)));
}

#[test]
fn repository_keys_walk_agents_and_repositories() {
    let mut model = model(200, 50);
    press(&mut model, "3");
    key(&mut model, KeyCode::Tab);
    press(&mut model, "gg");
    assert_eq!(
        model.selected,
        Some(id(INVOICE_SCHEMA)),
        "billing comes first"
    );
    press(&mut model, "j");
    assert_eq!(model.selected, Some(id(INVOICE_PAGE)));
    press(&mut model, "l");
    assert_eq!(model.selected, Some(id(TRACE_GATEWAY)), "next repository");
    press(&mut model, "h");
    assert_eq!(model.selected, Some(id(INVOICE_PAGE)));
    press(&mut model, "G");
    assert_eq!(model.selected, model.repos.order.last().copied());
    press(&mut model, "_");
    assert_eq!(model.repos.zoom, ZoomChoice::Fixed(Zoom::Compact));
}

#[test]
fn messages_pulse_while_they_travel_unless_motion_is_reduced() {
    // Just before trace-gateway writes to latency-audit, 16 s in.
    let at = start().plus_millis(15_900);
    let mut model = model_at_time(120, 40, at);
    press(&mut model, "2");
    let next: Vec<Event> = demo()
        .events_until(start().plus_millis(16_000))
        .into_iter()
        .filter(|event| event.at > at)
        .collect();
    update(&mut model, Msg::Events(next));
    assert!(model.pulses.is_moving(model.now), "about to leave");
    update(&mut model, Msg::Tick(start().plus_millis(16_500)));
    assert_eq!(model.pulses.active(model.now).count(), 1);
    update(&mut model, Msg::Tick(start().plus_millis(17_100)));
    assert_eq!(model.pulses.active(model.now).count(), 0, "arrived");
    model.motion = Motion::Reduced;
    assert!(
        !model.is_animating(),
        "nothing ticks fast with reduced motion"
    );
}

#[test]
fn small_screens_put_other_views_next_to_the_list() {
    let mut model = model(80, 24);
    press(&mut model, "2");
    assert_eq!(model.layout.mode, Mode::Split);
    assert!(model.layout.view.is_some());
    let mut seen = vec![model.focus];
    for _ in 0..3 {
        key(&mut model, KeyCode::Tab);
        seen.push(model.focus);
    }
    assert_eq!(seen, [Pane::Rail, Pane::View, Pane::Detail, Pane::Rail]);
    key(&mut model, KeyCode::Tab);
    press(&mut model, "1");
    assert_eq!(model.layout.mode, Mode::Narrow);
    assert_eq!(model.focus, Pane::Rail, "the hierarchy lives in the list");
}
