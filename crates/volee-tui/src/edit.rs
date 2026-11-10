//! The full-screen view of an agent in a volee host, live, and its edit
//! mode (ADR 0026): `i` or `a` sends your keys to the agent inside volee's
//! frame, the leave key (Ctrl+Q) gives them back to volee.

use std::path::PathBuf;

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use volee_core::AgentId;
use volee_host::{Size, Update, Welcome};

use crate::app::{Effect, Model, Msg, Overlay};
use crate::keymap::KeyCombo;
use crate::live::{Live, LiveInput, Outcome, Phase, key_press};
use crate::text;

/// The cells an agent's screen takes in the full-screen view of a terminal
/// of `size`: all of it but the hint line, the frame and a margin column.
pub(crate) fn screen_area(size: (u16, u16)) -> Rect {
    let (width, height) = size;
    Rect::new(2, 1, width.saturating_sub(3), height.saturating_sub(3))
}

/// The size edit mode gives the agent's terminal.
fn edit_size(model: &Model) -> Size {
    let area = screen_area(model.size);
    Size {
        cols: area.width.max(10),
        rows: area.height.max(3),
    }
}

/// The socket of the volee host an agent runs in, when the interface knows
/// where hosts live.
pub(crate) fn host_socket(model: &Model, agent: AgentId) -> Option<PathBuf> {
    let hosts = model.hosts.as_ref()?;
    let host = model.registry.agent(agent)?.location.host()?;
    Some(hosts.socket(host))
}

/// Whether the keys go to an agent.
pub(crate) fn editing(model: &Model) -> bool {
    model.overlay == Some(Overlay::Screen) && model.live.as_ref().is_some_and(|live| live.editing)
}

/// Whether the full-screen view shows a live screen of `agent`.
pub(crate) fn live_for(model: &Model, agent: AgentId) -> bool {
    model
        .live
        .as_ref()
        .is_some_and(|live| live.agent == agent && live.phase == Phase::Live)
}

/// Connects to the agent's host, to watch or to edit. A connection to the
/// same agent keeps its screen until the new snapshot arrives.
fn connect(model: &mut Model, agent: AgentId, edit: bool, effects: &mut Vec<Effect>) -> bool {
    let Some(socket) = host_socket(model, agent) else {
        return false;
    };
    model.next_link += 1;
    let link = model.next_link;
    match &mut model.live {
        Some(live) if live.agent == agent => {
            effects.push(Effect::LiveClose { link: live.link });
            live.reconnect(link, edit);
        }
        other => {
            if let Some(old) = other.as_ref() {
                effects.push(Effect::LiveClose { link: old.link });
            }
            *other = Some(Live::new(agent, link, edit));
        }
    }
    effects.push(Effect::LiveOpen {
        link,
        socket,
        edit,
        size: edit_size(model),
    });
    true
}

/// Shows the agent's screen live in the full-screen view, when it runs in
/// a volee host; returns whether it does.
pub(crate) fn watch(model: &mut Model, agent: AgentId, effects: &mut Vec<Effect>) -> bool {
    connect(model, agent, false, effects)
}

/// Enters edit mode on the agent: its screen full size inside volee, your
/// keys going to it. Returns false when it does not run in a volee host.
pub(crate) fn edit(model: &mut Model, agent: AgentId, effects: &mut Vec<Effect>) -> bool {
    if host_socket(model, agent).is_none() {
        return false;
    }
    model.overlay = Some(Overlay::Screen);
    model.preview.scroll = 0;
    if model
        .live
        .as_ref()
        .is_some_and(|live| live.agent == agent && live.editing)
    {
        return true;
    }
    connect(model, agent, true, effects)
}

/// Leaves edit mode: the keys come back to volee, the screen stays, live.
pub(crate) fn leave(model: &mut Model, effects: &mut Vec<Effect>) {
    if let Some(agent) = model.live.as_ref().map(|live| live.agent) {
        connect(model, agent, false, effects);
    }
}

/// Closes the live screen, when the full-screen view closes.
pub(crate) fn close(model: &mut Model, effects: &mut Vec<Effect>) {
    if let Some(live) = model.live.take() {
        effects.push(Effect::LiveClose { link: live.link });
    }
}

/// A key in edit mode: the leave key comes back, every other key goes to
/// the agent.
pub(crate) fn on_key(model: &mut Model, key: &KeyEvent, effects: &mut Vec<Effect>) {
    if KeyCombo::from_event(key) == model.keymap.leave_key() {
        leave(model, effects);
        return;
    }
    let Some(live) = &mut model.live else {
        return;
    };
    // Keys typed before the host answered are dropped: a few milliseconds.
    if live.phase != Phase::Live {
        return;
    }
    // Typing brings the newest lines back into view.
    live.scroll_to_bottom();
    let input = if live.keys {
        match key_press(key) {
            Some(press) => LiveInput::Key(press),
            None => return,
        }
    } else {
        let bytes = live.legacy_bytes(key);
        if bytes.is_empty() {
            return;
        }
        LiveInput::Bytes(bytes)
    };
    effects.push(Effect::LiveSend {
        link: live.link,
        input,
    });
}

/// Text pasted in edit mode goes to the agent.
pub(crate) fn on_paste(model: &mut Model, text: &str, effects: &mut Vec<Effect>) {
    let Some(live) = model.live.as_ref().filter(|live| live.phase == Phase::Live) else {
        return;
    };
    let input = if live.keys {
        LiveInput::Paste(text.to_owned())
    } else {
        LiveInput::Bytes(live.legacy_paste(text))
    };
    effects.push(Effect::LiveSend {
        link: live.link,
        input,
    });
}

/// The terminal changed size: in edit mode, the agent's follows.
pub(crate) fn on_resize(model: &mut Model, effects: &mut Vec<Effect>) {
    if !editing(model) {
        return;
    }
    let size = edit_size(model);
    if let Some(live) = &model.live
        && live.size != size
    {
        effects.push(Effect::LiveSend {
            link: live.link,
            input: LiveInput::Resize(size),
        });
    }
}

/// A message of a live screen's connection; returns whether anything
/// changed.
pub(crate) fn on_msg(model: &mut Model, msg: Msg, effects: &mut Vec<Effect>) -> bool {
    match msg {
        Msg::LiveOpened { link, welcome } => opened(model, link, &welcome),
        Msg::LiveUpdate { link, update } => updated(model, link, update),
        Msg::LiveClosed { link, reason } => closed(model, link, &reason, effects),
        _ => false,
    }
}

/// The host welcomed a connection.
pub(crate) fn opened(model: &mut Model, link: u64, welcome: &Welcome) -> bool {
    let Some(live) = model.live.as_mut().filter(|live| live.link == link) else {
        return false;
    };
    let wanted = live.editing;
    live.opened(welcome);
    let (agent, editing) = (live.agent, live.editing);
    if wanted && !editing {
        let name = model.name_of(agent);
        model.notify(
            format!("{name} has ended: there is nothing to type into."),
            true,
        );
    }
    true
}

/// An update from a host.
pub(crate) fn updated(model: &mut Model, link: u64, update: Update) -> bool {
    let Some(live) = model.live.as_mut().filter(|live| live.link == link) else {
        return false;
    };
    let was_editing = live.editing;
    let agent = live.agent;
    match live.apply(update) {
        Outcome::Drawn => {}
        Outcome::Exited => {
            let name = model.name_of(agent);
            model.notify(format!("{name} has ended."), !was_editing);
        }
        Outcome::InputTaken => {
            let name = model.name_of(agent);
            model.notify(
                format!("{name} was taken over from another terminal: volee watches it again."),
                false,
            );
        }
    }
    true
}

/// A connection ended, or could not open.
pub(crate) fn closed(
    model: &mut Model,
    link: u64,
    reason: &str,
    effects: &mut Vec<Effect>,
) -> bool {
    let Some(live) = model.live.as_mut().filter(|live| live.link == link) else {
        return false;
    };
    let (agent, was_editing, never_opened) =
        (live.agent, live.editing, live.phase == Phase::Connecting);
    let ended = live.exited;
    live.closed(reason.to_owned());
    let name = model.name_of(agent);
    if never_opened {
        let reason = text::clause(reason).to_owned();
        if was_editing {
            model.notify(
                format!(
                    "Cannot type into {name}: its volee host does not answer ({reason}). Send it \
                     a message to resume it in a new host."
                ),
                true,
            );
        }
        // The screen the source keeps, as before the live view.
        if model.overlay == Some(Overlay::Screen) && model.preview.agent == Some(agent) {
            model.request_preview(agent, effects);
        }
    } else if was_editing && !ended {
        model.notify(format!("The volee host of {name} went away."), true);
    }
    true
}

/// After an update: an agent adopted to be edited is edited as soon as the
/// interface sees it in its host.
pub(crate) fn settle(model: &mut Model, effects: &mut Vec<Effect>) {
    if let Some(agent) = model.edit_when_hosted
        && host_socket(model, agent).is_some()
    {
        model.edit_when_hosted = None;
        model.select(agent, effects);
        edit(model, agent, effects);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;
    use volee_host::KeyPress;

    use super::*;
    use crate::app::update;
    use crate::keymap::Context;
    use crate::test_support::{ctrl, hosted_model, key, press, welcome};

    const SIZE: Size = Size {
        cols: 117,
        rows: 37,
    };

    fn socket() -> PathBuf {
        PathBuf::from("/tmp/volee-test-hosts/h1.sock")
    }

    /// The host of link `link` answers, with a first screen.
    fn answer(model: &mut Model, link: u64, keys: bool) {
        update(
            model,
            Msg::LiveOpened {
                link,
                welcome: welcome(SIZE, keys),
            },
        );
        update(
            model,
            Msg::LiveUpdate {
                link,
                update: Update::Snapshot(b"\x1b[2J\x1b[H> fix the tests".to_vec()),
            },
        );
    }

    fn sent(effects: &[Effect]) -> Vec<LiveInput> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::LiveSend { input, .. } => Some(input.clone()),
                _ => None,
            })
            .collect()
    }

    fn press_of(name: &str, ctrl: bool) -> LiveInput {
        LiveInput::Key(KeyPress {
            key: name.to_owned(),
            shift: false,
            alt: false,
            ctrl,
        })
    }

    #[test]
    fn i_enters_edit_mode_with_the_panel_s_size() {
        let (mut model, _) = hosted_model(120, 40);
        let effects = press(&mut model, "i");
        assert!(effects.contains(&Effect::LiveOpen {
            link: 1,
            socket: socket(),
            edit: true,
            size: SIZE,
        }));
        assert_eq!(model.overlay, Some(Overlay::Screen));
        assert_eq!(model.contexts(), [Context::Attached]);
        assert_eq!(screen_area(model.size), Rect::new(2, 1, 117, 37));
    }

    #[test]
    fn every_key_goes_to_the_agent_but_the_leave_key() {
        let (mut model, _) = hosted_model(120, 40);
        press(&mut model, "a");
        answer(&mut model, 1, true);
        // Escape, q and Ctrl+C are the agent's, not volee's.
        assert_eq!(
            sent(&key(&mut model, KeyCode::Esc)),
            [press_of("escape", false)]
        );
        assert_eq!(sent(&press(&mut model, "q")), [press_of("q", false)]);
        assert_eq!(sent(&ctrl(&mut model, 'c')), [press_of("c", true)]);
        assert!(editing(&model));

        let effects = ctrl(&mut model, 'q');
        assert_eq!(
            effects,
            [
                Effect::LiveClose { link: 1 },
                Effect::LiveOpen {
                    link: 2,
                    socket: socket(),
                    edit: false,
                    size: SIZE,
                },
            ]
        );
        assert!(!editing(&model));
        assert_eq!(model.contexts(), [Context::Screen]);
        // The screen stays while the watcher connects.
        assert!(model.live.as_ref().is_some_and(Live::has_screen));

        let effects = press(&mut model, "z");
        assert!(effects.contains(&Effect::LiveClose { link: 2 }));
        assert_eq!(model.overlay, None);
        assert!(model.live.is_none());
    }

    #[test]
    fn pastes_and_sizes_follow_in_edit_mode() {
        let (mut model, _) = hosted_model(120, 40);
        press(&mut model, "i");
        answer(&mut model, 1, true);
        let effects = update(&mut model, Msg::Paste("two\nlines".into()));
        assert_eq!(sent(&effects), [LiveInput::Paste("two\nlines".into())]);
        let effects = update(
            &mut model,
            Msg::Resize {
                width: 100,
                height: 30,
            },
        );
        assert_eq!(
            sent(&effects),
            [LiveInput::Resize(Size { cols: 97, rows: 27 })]
        );
    }

    #[test]
    fn an_older_host_gets_the_bytes_of_a_terminal() {
        let (mut model, _) = hosted_model(120, 40);
        press(&mut model, "i");
        answer(&mut model, 1, false);
        assert_eq!(
            sent(&key(&mut model, KeyCode::Enter)),
            [LiveInput::Bytes(b"\r".to_vec())]
        );
        let effects = update(&mut model, Msg::Paste("a\nb".into()));
        assert_eq!(sent(&effects), [LiveInput::Bytes(b"a\rb".to_vec())]);
    }

    #[test]
    fn z_watches_an_agent_in_a_host_live() {
        let (mut model, _) = hosted_model(120, 40);
        let effects = press(&mut model, "z");
        assert!(effects.contains(&Effect::LiveOpen {
            link: 1,
            socket: socket(),
            edit: false,
            size: SIZE,
        }));
        answer(&mut model, 1, true);
        assert!(model.live.as_ref().is_some_and(|live| !live.editing));
        // Keys are volee's: `i` enters edit mode on the same screen.
        let effects = press(&mut model, "i");
        assert!(effects.contains(&Effect::LiveClose { link: 1 }));
        assert!(effects.contains(&Effect::LiveOpen {
            link: 2,
            socket: socket(),
            edit: true,
            size: SIZE,
        }));
    }

    #[test]
    fn a_host_that_does_not_answer_is_said_plainly() {
        let (mut model, agent) = hosted_model(120, 40);
        press(&mut model, "i");
        update(
            &mut model,
            Msg::LiveClosed {
                link: 1,
                reason: "No such file or directory (os error 2)".into(),
            },
        );
        assert!(!editing(&model));
        let notice = model.notice.as_ref().expect("notice");
        assert!(notice.error);
        assert!(
            notice.text.starts_with("Cannot type into"),
            "{}",
            notice.text
        );
        assert_eq!(model.preview.agent, Some(agent));
    }

    #[test]
    fn input_taken_elsewhere_ends_edit_mode() {
        let (mut model, _) = hosted_model(120, 40);
        press(&mut model, "i");
        answer(&mut model, 1, true);
        update(
            &mut model,
            Msg::LiveUpdate {
                link: 1,
                update: Update::Event(volee_host::Event::InputTaken),
            },
        );
        assert!(!editing(&model));
        assert_eq!(model.contexts(), [Context::Screen]);
        // Updates of an old connection are dropped.
        assert!(!updated(&mut model, 7, Update::Output(b"x".to_vec())));
    }
}
