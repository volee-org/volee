//! The wall of screens, driven by keys, the mouse, time and events.

use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use volee_core::event::EventKind;
use volee_core::{AgentState, Event};
use volee_host::{Size, Update};

use super::*;
use crate::app::{Msg, update};
use crate::source::Door;
use crate::test_support::{ctrl, hosted_model, key, model, press, start, welcome};

const SIZE: Size = Size {
    cols: 100,
    rows: 30,
};

/// The tiles of a page in the terminal of these tests, 160 by 48.
const PAGE: usize = 9;

fn loads(effects: &[Effect]) -> Vec<AgentId> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::LoadPreview(agent) => Some(*agent),
            _ => None,
        })
        .collect()
}

fn opens(effects: &[Effect]) -> Vec<(u64, bool)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::LiveOpen { link, edit, .. } => Some((*link, *edit)),
            _ => None,
        })
        .collect()
}

fn closes(effects: &[Effect]) -> Vec<u64> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::LiveClose { link } => Some(*link),
            _ => None,
        })
        .collect()
}

/// The agents on the page shown.
fn shown(model: &Model) -> Vec<AgentId> {
    model
        .wall
        .as_ref()
        .map(|wall| wall.tiles.iter().map(|tile| tile.agent).collect())
        .unwrap_or_default()
}

fn tile(model: &Model, agent: AgentId) -> &TileScreen {
    &model
        .wall
        .as_ref()
        .unwrap()
        .tiles
        .iter()
        .find(|tile| tile.agent == agent)
        .unwrap()
        .screen
}

fn reading(model: &Model, agent: AgentId) -> &Reading {
    match tile(model, agent) {
        TileScreen::Read(reading) => reading,
        TileScreen::Live(_) => panic!("followed live"),
    }
}

fn live(model: &Model, agent: AgentId) -> &Live {
    match tile(model, agent) {
        TileScreen::Live(live) => live,
        TileScreen::Read(_) => panic!("read through the source"),
    }
}

fn state(model: &mut Model, agent: AgentId, state: AgentState) -> Vec<Effect> {
    let at = model.now;
    update(
        model,
        Msg::Events(vec![Event::new(
            at,
            EventKind::StateChanged {
                agent,
                state,
                detail: None,
            },
        )]),
    )
}

fn tick(model: &mut Model, seconds: i64) -> Vec<Effect> {
    update(model, Msg::Tick(start().plus_millis(seconds * 1_000)))
}

fn screen(model: &mut Model, agent: AgentId, text: &str) {
    update(
        model,
        Msg::PreviewLoaded {
            agent,
            result: Ok(text.into()),
        },
    );
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

#[test]
fn the_wall_holds_the_agents_that_run_nine_to_a_page() {
    let mut model = model(160, 48);
    let all = running(&model);
    assert!(all.len() > PAGE, "the demo fills more than a page");
    press(&mut model, "gg");
    let selected = model.selected.unwrap();
    assert_eq!(all[0], selected, "the list starts on its first agent");

    let effects = press(&mut model, "5");
    assert_eq!(model.overlay, Some(Overlay::Wall));
    assert_eq!(model.contexts(), [crate::keymap::Context::Wall]);
    assert_eq!(shown(&model), all[..PAGE]);
    let wall = model.wall.as_ref().unwrap();
    assert_eq!((wall.count, wall.pages()), (all.len(), (0, 2)));
    // Every screen is read through the source; the selected agent's was
    // asked for already, for the preview: that answer serves its tile.
    assert_eq!(loads(&effects), all[1..PAGE]);
    assert_eq!(opens(&effects), [], "no agent of the demo is in a host");

    // An answer fills its tile, whichever agent is selected.
    screen(&mut model, all[3], "\x1b[2J\x1b[Hthree");
    screen(&mut model, selected, "\x1b[2J\x1b[Hfirst");
    let text = |agent| {
        let screen = reading(&model, agent).screen.as_ref()?;
        screen.text().into_iter().next()
    };
    assert_eq!(text(all[3]).as_deref(), Some("three"));
    assert_eq!(text(selected).as_deref(), Some("first"));
    assert_eq!(text(all[4]), None);
}

#[test]
fn moving_on_the_wall_moves_in_the_list() {
    let mut model = model(160, 48);
    let all = running(&model);
    press(&mut model, "gg5");
    press(&mut model, "l");
    assert_eq!(model.selected, Some(all[1]));
    // Nine tiles are three rows of three.
    press(&mut model, "j");
    assert_eq!(model.selected, Some(all[4]));
    press(&mut model, "h");
    assert_eq!(model.selected, Some(all[3]));
    press(&mut model, "k");
    assert_eq!(model.selected, Some(all[0]));
    // Nothing above the first row, nothing left of the first tile.
    press(&mut model, "kh");
    assert_eq!(model.selected, Some(all[0]));

    // Below the last row of a page: the next page, whose screens are read.
    press(&mut model, "jj");
    assert_eq!(model.selected, Some(all[6]));
    let effects = press(&mut model, "j");
    let last = all.len() - 1;
    assert_eq!(model.selected, Some(all[9.min(last)]));
    assert_eq!(shown(&model), all[PAGE..]);
    assert_eq!(model.wall.as_ref().unwrap().pages(), (1, 2));
    assert_eq!(loads(&effects), all[PAGE..]);
    // And back.
    press(&mut model, "gg");
    assert_eq!(model.selected, Some(all[0]));
    assert_eq!(shown(&model), all[..PAGE]);
    press(&mut model, "G");
    assert_eq!(model.selected, Some(all[last]));

    // Leaving the wall leaves the list on the agent the wall was on.
    key(&mut model, KeyCode::Esc);
    assert_eq!(model.overlay, None);
    assert!(model.wall.is_none());
    assert_eq!(model.selected, Some(all[last]));
}

#[test]
fn an_agent_in_a_volee_host_is_followed_live() {
    let (mut model, agent) = hosted_model(160, 48);
    // The preview follows the selected agent's screen already.
    assert_eq!(opens(&tick(&mut model, 1)), [(1, false)]);

    let effects = press(&mut model, "5");
    // The wall follows every screen itself: the preview's connection
    // closes, the tile opens its own, which only watches.
    assert_eq!(
        (closes(&effects), opens(&effects)),
        (vec![1], vec![(2, false)])
    );
    assert!(model.live.is_none());
    assert!(
        !loads(&effects).contains(&agent),
        "a live screen is not read"
    );
    update(
        &mut model,
        Msg::LiveOpened {
            link: 2,
            welcome: welcome(SIZE, true),
        },
    );
    update(
        &mut model,
        Msg::LiveUpdate {
            link: 2,
            update: Update::Snapshot(b"\x1b[2J\x1b[H> fix the tests".to_vec()),
        },
    );
    assert_eq!(live(&model, agent).text().trim_end(), "> fix the tests");
    assert!(follows(&model, agent));
    assert!(
        model.live.is_none(),
        "the tile took its connection's messages"
    );
    // Nothing else connects while the wall shows, tick after tick.
    for second in 2..5 {
        let effects = tick(&mut model, second);
        assert_eq!(
            (opens(&effects), loads(&effects).contains(&agent)),
            (vec![], false)
        );
    }

    // Closing the wall closes its connections; the preview follows again.
    let effects = press(&mut model, "q");
    assert_eq!(closes(&effects), [2]);
    assert!(model.wall.is_none());
    assert_eq!(opens(&tick(&mut model, 6)), [(3, false)]);
}

#[test]
fn what_is_opened_from_the_wall_comes_back_to_the_wall() {
    let (mut model, agent) = hosted_model(160, 48);
    let effects = press(&mut model, "5");
    assert_eq!(opens(&effects), [(1, false)]);

    // Enter opens the selected tile's agent: edit mode, and the wall's own
    // connections close while it is covered.
    let effects = key(&mut model, KeyCode::Enter);
    assert_eq!(model.overlay, Some(Overlay::Screen));
    assert_eq!(
        (closes(&effects), opens(&effects)),
        (vec![1], vec![(2, true)])
    );
    assert!(crate::edit::editing(&model) || model.live.as_ref().is_some_and(|live| live.editing));
    update(
        &mut model,
        Msg::LiveOpened {
            link: 2,
            welcome: welcome(SIZE, true),
        },
    );
    // The leave key gives the keys back; the screen stays, full size.
    let effects = ctrl(&mut model, 'q');
    assert_eq!(model.overlay, Some(Overlay::Screen));
    assert_eq!(
        (closes(&effects), opens(&effects)),
        (vec![2], vec![(3, false)])
    );
    // Back: the wall, whose tile connects again.
    let effects = key(&mut model, KeyCode::Esc);
    assert_eq!(model.overlay, Some(Overlay::Wall));
    assert_eq!(
        (closes(&effects), opens(&effects)),
        (vec![3], vec![(4, false)])
    );
    assert!(model.live.is_none());
    assert_eq!(live(&model, agent).link, 4);

    // `z` shows the selected screen full size, and comes back the same way.
    let effects = press(&mut model, "z");
    assert_eq!(model.overlay, Some(Overlay::Screen));
    assert_eq!(
        (closes(&effects), opens(&effects)),
        (vec![4], vec![(5, false)])
    );
    let effects = press(&mut model, "z");
    assert_eq!(model.overlay, Some(Overlay::Wall));
    assert_eq!(
        (closes(&effects), opens(&effects)),
        (vec![5], vec![(6, false)])
    );

    // Leaving the wall itself leaves it for good.
    press(&mut model, "q");
    assert_eq!(model.overlay, None);
    assert!(model.wall.is_none());
    tick(&mut model, 1);
    assert_eq!(model.overlay, None);
}

#[test]
fn an_agent_that_runs_elsewhere_opens_through_its_door_and_comes_back() {
    let mut model = model(160, 48);
    let all = running(&model);
    press(&mut model, "gg5l");
    let agent = all[1];
    let effects = press(&mut model, "i");
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::Open { agent: opened, .. } if *opened == agent))
    );
    assert_eq!(
        model.overlay,
        Some(Overlay::Wall),
        "until the source answers"
    );
    let effects = update(
        &mut model,
        Msg::Opened {
            agent,
            result: Ok(Door::Host {
                socket: "/tmp/volee-test-views/v1.sock".into(),
                view: true,
            }),
        },
    );
    assert_eq!(model.overlay, Some(Overlay::Screen));
    let link = opens(&effects)[0].0;
    update(
        &mut model,
        Msg::LiveOpened {
            link,
            welcome: welcome(SIZE, true),
        },
    );
    // Leaving a view closes it: straight back to the wall, on that tile.
    ctrl(&mut model, 'q');
    assert_eq!(model.overlay, Some(Overlay::Wall));
    assert_eq!(model.selected, Some(agent));
    assert_eq!(shown(&model), all[..PAGE]);
    assert!(model.live.is_none());
}

#[test]
fn screens_read_through_the_source_are_read_again_often_at_work_and_less_often_at_rest() {
    let mut model = model(160, 48);
    let all = running(&model);
    press(&mut model, "gg5");
    let busy = |model: &Model, agent: AgentId| model.registry.agent(agent).unwrap().state.is_busy();
    let working: Vec<AgentId> = all[1..PAGE]
        .iter()
        .copied()
        .filter(|agent| busy(&model, *agent))
        .collect();
    let resting: Vec<AgentId> = all[1..PAGE]
        .iter()
        .copied()
        .filter(|agent| !busy(&model, *agent))
        .collect();
    assert!(!working.is_empty() && !resting.is_empty());
    for agent in &all[..PAGE] {
        screen(&mut model, *agent, "\x1b[2J\x1b[Hread");
    }
    // The selected agent's screen is the preview's, read on its own pace:
    // the tiles of the others are what the wall asks for.
    let others = |effects: &[Effect]| -> Vec<AgentId> {
        loads(effects)
            .into_iter()
            .filter(|agent| *agent != all[0])
            .collect()
    };
    // Not before five seconds, and then only the agents that work.
    assert_eq!(others(&tick(&mut model, 4)), []);
    let again = others(&tick(&mut model, 5));
    assert_eq!(again, working);
    assert!(resting.iter().all(|agent| !again.contains(agent)));
    // A request under way is not doubled.
    assert_eq!(others(&tick(&mut model, 11)), []);
    // Five seconds after the answer, again.
    for agent in &working {
        screen(&mut model, *agent, "\x1b[2J\x1b[Hread again");
    }
    assert_eq!(others(&tick(&mut model, 14)), []);
    // An agent at rest is read again too, fifteen seconds after its last
    // reading: its screen still changes, when the last lines of a turn
    // land late or a person types in its pane.
    assert_eq!(others(&tick(&mut model, 15)), resting);
    assert_eq!(others(&tick(&mut model, 16)), working);
    for agent in &resting {
        screen(&mut model, *agent, "\x1b[2J\x1b[Hread at rest");
    }

    // A change of state asks again at once, without waiting for the pace.
    let rested = resting[0];
    let effects = state(&mut model, rested, AgentState::Working);
    assert!(loads(&effects).contains(&rested));
}

#[test]
fn a_screen_the_source_cannot_read_is_not_asked_for_again_until_things_change() {
    let mut model = model(160, 48);
    let all = running(&model);
    press(&mut model, "5");
    let busy = all[1..PAGE]
        .iter()
        .copied()
        .find(|agent| model.registry.agent(*agent).unwrap().state.is_busy())
        .unwrap();
    screen(&mut model, busy, "\x1b[2J\x1b[Hin the background");
    let place = |model: &mut Model, location: Location| {
        let at = model.now;
        update(
            model,
            Msg::Events(vec![Event::new(
                at,
                EventKind::LocationChanged {
                    agent: busy,
                    location,
                },
            )]),
        )
    };
    // The session moves to a terminal volee cannot reach: its screen is
    // asked for where it now is, and the source says it cannot read it.
    let effects = place(&mut model, Location::Terminal);
    assert!(loads(&effects).contains(&busy));
    update(
        &mut model,
        Msg::PreviewLoaded {
            agent: busy,
            result: Err(SourceError::Unsupported(
                "it runs in a terminal of its own".into(),
            )),
        },
    );
    let said = reading(&model, busy).error.clone().unwrap();
    assert!(said.contains("a terminal of its own"), "{said}");
    // Though the agent works, nothing asks again, tick after tick.
    for second in [6, 12, 18] {
        assert!(!loads(&tick(&mut model, second)).contains(&busy));
    }
    // Back in the background, where its harness replays it: asked again.
    let effects = place(&mut model, Location::Background);
    assert!(loads(&effects).contains(&busy));
    screen(&mut model, busy, "\x1b[2J\x1b[Hnow it shows");
    assert!(reading(&model, busy).error.is_none());
}

#[test]
fn the_wall_follows_the_agents_that_stop_and_start() {
    let mut model = model(160, 48);
    let all = running(&model);
    press(&mut model, "gg5l");
    assert_eq!(model.selected, Some(all[1]));
    // The selected agent stops: its tile leaves, the next one takes its
    // place and the selection.
    state(&mut model, all[1], AgentState::Stopped);
    let now = running(&model);
    assert!(!now.contains(&all[1]));
    assert_eq!(shown(&model), now[..PAGE.min(now.len())]);
    assert_eq!(model.selected, Some(now[1]));
    assert_eq!(model.overlay, Some(Overlay::Wall));

    // Nothing runs any more: the wall closes and says why.
    for agent in now {
        state(&mut model, agent, AgentState::Stopped);
    }
    assert_eq!(model.overlay, None);
    assert!(model.wall.is_none());
    assert!(
        model
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("the wall closed")
    );
    // And does not open on nothing.
    press(&mut model, "5");
    assert_eq!(model.overlay, None);
    assert!(
        model
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("No agent of the list runs")
    );
}

#[test]
fn a_click_selects_a_tile_and_a_second_one_opens_it() {
    let mut model = model(160, 48);
    let all = running(&model);
    press(&mut model, "5");
    let (_, areas) = areas(model.size, PAGE);
    // The middle of the fifth tile: second row, second column.
    let target = areas[4];
    let (column, row) = (target.x + target.width / 2, target.y + target.height / 2);
    let effects = click(&mut model, column, row);
    assert_eq!(model.selected, Some(all[4]));
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Open { .. }))
    );
    let effects = click(&mut model, column, row);
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::Open { agent, .. } if *agent == all[4]))
    );
    // The header row is no tile.
    assert!(!model.dirty || click(&mut model, 2, 0).is_empty());
}

#[test]
fn a_page_holds_what_the_terminal_shows_at_a_readable_size() {
    // Two in a small terminal, nine in a large one, more only where each
    // tile stays roomy.
    assert_eq!(capacity((80, 24)), (2, 1));
    assert_eq!(capacity((100, 30)), (2, 2));
    assert_eq!(capacity((120, 40)), (3, 2));
    assert_eq!(capacity((200, 50)), (3, 3));
    assert_eq!(capacity((308, 80)), (4, 3));
    assert_eq!(capacity((320, 100)), (4, 4));
    assert_eq!(capacity((30, 8)), (1, 1));
    // The grid of a page that is not full stays as square as it can.
    let grids: Vec<(usize, usize)> = (0..=9).map(|count| grid((200, 50), count)).collect();
    assert_eq!(
        grids,
        [
            (0, 0),
            (1, 1),
            (2, 1),
            (2, 2),
            (2, 2),
            (3, 2),
            (3, 2),
            (3, 3),
            (3, 3),
            (3, 3)
        ]
    );
    assert_eq!(grid((308, 80), 10), (4, 3));
    // A wide and short terminal lines its tiles up.
    assert_eq!((capacity((200, 14)), grid((200, 14), 3)), ((3, 1), (3, 1)));
    for size in [(80, 24), (120, 40), (308, 80), (200, 14), (30, 8)] {
        for count in 1..=page_size(size) {
            let (columns, rows) = grid(size, count);
            let (most_columns, most_rows) = capacity(size);
            assert!(columns * rows >= count, "{size:?} {count}");
            assert!(
                columns <= most_columns && rows <= most_rows,
                "{size:?} {count}"
            );
            assert_eq!(areas(size, count).1.len(), count);
        }
    }

    // The page follows the terminal: resized, the wall shows fewer tiles,
    // around the selected one.
    let mut model = model(160, 48);
    let all = running(&model);
    press(&mut model, "gg5llllll");
    assert_eq!(model.selected, Some(all[6]));
    assert_eq!(shown(&model).len(), 9);
    update(
        &mut model,
        Msg::Resize {
            width: 80,
            height: 24,
        },
    );
    // A narrow terminal lists its agents as a tree: the wall follows the
    // list's order, as it is now.
    let listed = running(&model);
    let position = listed.iter().position(|agent| *agent == all[6]).unwrap();
    assert_eq!(model.selected, Some(all[6]));
    assert_eq!(
        shown(&model),
        listed[position / 2 * 2..position / 2 * 2 + 2]
    );
    assert_eq!(
        model.wall.as_ref().unwrap().pages(),
        (position / 2, listed.len().div_ceil(2))
    );
}
