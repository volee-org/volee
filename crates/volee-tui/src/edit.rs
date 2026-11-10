//! The full-screen view of an agent in a volee host, live, and its edit
//! mode (ADR 0026): your keys go to the agent inside volee's frame, the
//! leave key (Ctrl+Q) gives them back to volee.
//!
//! An agent that runs in no volee host is opened the same way, through a
//! view: a host the source starts on the session's own door (ADR 0027).
//! Leaving a view closes it; the agent keeps running where it runs.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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
pub(crate) fn edit_size(model: &Model) -> Size {
    let area = screen_area(model.size);
    Size {
        cols: area.width.max(10),
        rows: area.height.max(3),
    }
}

/// The socket of an agent's terminal: the one the source opened on it, or
/// the volee host it runs in, when the interface knows where hosts live.
pub(crate) fn host_socket(model: &Model, agent: AgentId) -> Option<PathBuf> {
    if let Some((opened, socket, _)) = &model.door
        && *opened == agent
    {
        return Some(socket.clone());
    }
    let hosts = model.hosts.as_ref()?;
    let host = model.registry.agent(agent)?.location.host()?;
    Some(hosts.socket(host))
}

/// Whether the terminal open on `agent` is a view on a session that runs
/// elsewhere (ADR 0027).
pub(crate) fn is_view(model: &Model, agent: AgentId) -> bool {
    matches!(&model.door, Some((opened, _, true)) if *opened == agent)
}

/// Closes the view open on an agent and comes back to the interface.
fn close_view(model: &mut Model, effects: &mut Vec<Effect>) {
    close(model, effects);
    if model.overlay == Some(Overlay::Screen) {
        model.overlay = None;
    }
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

/// Leaves edit mode: the keys come back to volee. The screen of an agent in
/// a volee host stays, live; a view closes, and its agent keeps running.
pub(crate) fn leave(model: &mut Model, effects: &mut Vec<Effect>) {
    let Some(agent) = model.live.as_ref().map(|live| live.agent) else {
        return;
    };
    if is_view(model, agent) {
        let name = model.name_of(agent);
        close_view(model, effects);
        model.notify(format!("Left {name}: it keeps running."), false);
    } else {
        connect(model, agent, false, effects);
    }
}

/// Closes the live screen, when the full-screen view closes, and forgets
/// the terminal the source opened for it.
pub(crate) fn close(model: &mut Model, effects: &mut Vec<Effect>) {
    if let Some(live) = model.live.take() {
        effects.push(Effect::LiveClose { link: live.link });
    }
    model.door = None;
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
        // A host from before 0.10 does not resume a program that suspends
        // itself: Claude Code would stay frozen.
        if bytes == [0x1a] {
            model.notify(
                "Ctrl+Z is not sent to an agent in a volee host from before 0.10: nothing there \
                 would resume Claude Code. It gets a new host when it is stopped and resumed.",
                true,
            );
            return;
        }
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

/// How many lines one notch of the wheel scrolls back.
const WHEEL_LINES: isize = 3;

/// The mouse in edit mode. A program that asked for the mouse gets it, on
/// the cell under the pointer, encoded the way it asked: clicks, drags and
/// the wheel. For any other program the wheel reads back through what it
/// wrote, as in a terminal. Returns whether the screen changed.
pub(crate) fn on_mouse(model: &mut Model, mouse: MouseEvent, effects: &mut Vec<Effect>) -> bool {
    let area = screen_area(model.size);
    let Some(live) = model.live.as_mut().filter(|live| live.phase == Phase::Live) else {
        return false;
    };
    if live.wants_mouse() {
        // Only on the agent's own cells: the frame around them is volee's.
        let inside = mouse.column >= area.x
            && mouse.row >= area.y
            && mouse.column - area.x < live.size.cols.min(area.width)
            && mouse.row - area.y < live.size.rows.min(area.height);
        if !inside || live.scroll > 0 {
            return false;
        }
        let bytes = live.mouse_bytes(&volee_term::MouseInput {
            kind: mouse_kind(mouse.kind),
            column: mouse.column - area.x,
            row: mouse.row - area.y,
            shift: mouse.modifiers.contains(KeyModifiers::SHIFT),
            alt: mouse.modifiers.contains(KeyModifiers::ALT),
            ctrl: mouse.modifiers.contains(KeyModifiers::CONTROL),
        });
        if !bytes.is_empty() {
            effects.push(Effect::LiveSend {
                link: live.link,
                input: LiveInput::Bytes(bytes),
            });
        }
        return false;
    }
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            live.scroll_by(WHEEL_LINES);
            true
        }
        MouseEventKind::ScrollDown => {
            live.scroll_by(-WHEEL_LINES);
            true
        }
        _ => false,
    }
}

/// A mouse event of the person's terminal, as the emulator names it.
fn mouse_kind(kind: MouseEventKind) -> volee_term::MouseKind {
    use volee_term::MouseKind;
    let button = |button| match button {
        MouseButton::Left => volee_term::MouseButton::Left,
        MouseButton::Middle => volee_term::MouseButton::Middle,
        MouseButton::Right => volee_term::MouseButton::Right,
    };
    match kind {
        MouseEventKind::Down(pressed) => MouseKind::Press(button(pressed)),
        MouseEventKind::Up(released) => MouseKind::Release(button(released)),
        MouseEventKind::Drag(held) => MouseKind::Drag(button(held)),
        MouseEventKind::Moved => MouseKind::Move,
        MouseEventKind::ScrollUp => MouseKind::WheelUp,
        MouseEventKind::ScrollDown => MouseKind::WheelDown,
        MouseEventKind::ScrollLeft => MouseKind::WheelLeft,
        MouseEventKind::ScrollRight => MouseKind::WheelRight,
    }
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
        Msg::LiveUpdate { link, update } => updated(model, link, update, effects),
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
        if is_view(model, agent) {
            // The door's program ended before anyone was there: what it
            // printed says why.
            model.notify(
                format!(
                    "The terminal on {name} closed at once: its last lines are shown. Esc \
                     comes back."
                ),
                true,
            );
        } else {
            model.notify(
                format!("{name} has ended: there is nothing to type into."),
                true,
            );
        }
    }
    true
}

/// An update from a host.
pub(crate) fn updated(
    model: &mut Model,
    link: u64,
    update: Update,
    effects: &mut Vec<Effect>,
) -> bool {
    let Some(live) = model.live.as_mut().filter(|live| live.link == link) else {
        return false;
    };
    let was_editing = live.editing;
    let agent = live.agent;
    match live.apply(update) {
        Outcome::Drawn => {}
        // The program of a view is the door, not the agent: it ends when
        // the person leaves through it (Ctrl+Z in `claude attach`), and the
        // agent keeps running.
        Outcome::Exited { failed } if is_view(model, agent) => {
            let name = model.name_of(agent);
            if failed {
                model.notify(
                    format!(
                        "The terminal on {name} closed with an error: its last lines are \
                         shown. Esc comes back."
                    ),
                    true,
                );
            } else {
                close_view(model, effects);
                model.notify(format!("Left {name}: it keeps running."), false);
            }
        }
        Outcome::Exited { .. } => {
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
    if is_view(model, agent) {
        // A view that ended with an error stays, to be read.
        if !ended {
            close_view(model, effects);
            let reason = text::clause(reason).to_owned();
            if never_opened {
                model.notify(
                    format!("Cannot open {name}: its terminal does not answer ({reason})."),
                    true,
                );
            } else {
                model.notify(
                    format!("The terminal on {name} closed: it keeps running."),
                    false,
                );
            }
        }
        return true;
    }
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
        // Nothing in an older host would resume Claude Code after Ctrl+Z.
        assert_eq!(sent(&ctrl(&mut model, 'z')), []);
        assert!(
            model
                .notice
                .as_ref()
                .is_some_and(|notice| notice.text.starts_with("Ctrl+Z"))
        );
    }

    fn mouse(model: &mut Model, kind: MouseEventKind, column: u16, row: u16) -> Vec<Effect> {
        update(
            model,
            Msg::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }),
        )
    }

    fn output(model: &mut Model, link: u64, bytes: &[u8]) {
        update(
            model,
            Msg::LiveUpdate {
                link,
                update: Update::Output(bytes.to_vec()),
            },
        );
    }

    #[test]
    fn a_program_that_asked_for_the_mouse_gets_it_on_its_own_cells() {
        let (mut model, _) = hosted_model(120, 40);
        press(&mut model, "i");
        answer(&mut model, 1, true);
        // The program turns mouse reporting on, in the SGR format.
        output(&mut model, 1, b"\x1b[?1000h\x1b[?1006h");
        let left = MouseEventKind::Down(MouseButton::Left);
        // The agent's screen starts at column 2, row 1 of the terminal.
        assert_eq!(
            sent(&mouse(&mut model, left, 6, 3)),
            [LiveInput::Bytes(b"\x1b[<0;5;3M".to_vec())]
        );
        assert_eq!(
            sent(&mouse(
                &mut model,
                MouseEventKind::Up(MouseButton::Left),
                6,
                3
            )),
            [LiveInput::Bytes(b"\x1b[<0;5;3m".to_vec())]
        );
        assert_eq!(
            sent(&mouse(&mut model, MouseEventKind::ScrollUp, 2, 1)),
            [LiveInput::Bytes(b"\x1b[<64;1;1M".to_vec())]
        );
        // The frame and the bottom line are volee's: nothing is sent.
        assert_eq!(sent(&mouse(&mut model, left, 0, 0)), []);
        assert_eq!(sent(&mouse(&mut model, left, 6, 39)), []);
        // It asked for clicks, not for moves.
        assert_eq!(sent(&mouse(&mut model, MouseEventKind::Moved, 6, 3)), []);
        assert!(editing(&model), "the mouse never leaves edit mode");
    }

    #[test]
    fn for_any_other_program_the_wheel_reads_back() {
        let (mut model, _) = hosted_model(120, 40);
        press(&mut model, "i");
        answer(&mut model, 1, true);
        // More lines than the screen holds.
        for line in 1..=80 {
            output(&mut model, 1, format!("line {line}\r\n").as_bytes());
        }
        // Nothing goes to a program that did not ask for the mouse.
        let click = MouseEventKind::Down(MouseButton::Left);
        assert_eq!(sent(&mouse(&mut model, click, 6, 3)), []);
        assert_eq!(sent(&mouse(&mut model, MouseEventKind::ScrollUp, 6, 3)), []);
        assert_eq!(model.live.as_ref().unwrap().scroll, 3);
        mouse(&mut model, MouseEventKind::ScrollUp, 6, 3);
        assert_eq!(model.live.as_ref().unwrap().scroll, 6);
        mouse(&mut model, MouseEventKind::ScrollDown, 6, 3);
        assert_eq!(model.live.as_ref().unwrap().scroll, 3);
        // What the agent writes meanwhile does not move the view: it is
        // further back.
        output(&mut model, 1, b"line 81\r\nline 82\r\n");
        assert_eq!(model.live.as_ref().unwrap().scroll, 5);
        // Typing comes back to the newest lines.
        press(&mut model, "x");
        assert_eq!(model.live.as_ref().unwrap().scroll, 0);
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
        let update = Update::Output(b"x".to_vec());
        assert!(!updated(&mut model, 7, update, &mut Vec::new()));
    }
}
