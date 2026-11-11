//! Mouse events turned into the bytes a program expects, following the
//! tracking mode and the format it turned on. A program that did not ask
//! for the mouse gets nothing.

use libghostty_vt::Terminal;
use libghostty_vt::key::Mods;
use libghostty_vt::mouse::{Action, Button, Encoder, EncoderSize, Event, Position};

use crate::Error;

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    /// The main button.
    Left,
    /// The wheel's button.
    Middle,
    /// The secondary button.
    Right,
}

/// What the mouse did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    /// A button went down.
    Press(MouseButton),
    /// A button came up.
    Release(MouseButton),
    /// The mouse moved with a button held.
    Drag(MouseButton),
    /// The mouse moved with no button held.
    Move,
    /// The wheel turned away from the person.
    WheelUp,
    /// The wheel turned towards the person.
    WheelDown,
    /// The wheel turned left.
    WheelLeft,
    /// The wheel turned right.
    WheelRight,
}

/// A mouse event on a cell of the screen, with its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseInput {
    /// What happened.
    pub kind: MouseKind,
    /// The cell's column, from 0.
    pub column: u16,
    /// The cell's row, from 0.
    pub row: u16,
    /// Shift was held.
    pub shift: bool,
    /// Alt (Option) was held.
    pub alt: bool,
    /// Control was held.
    pub ctrl: bool,
}

/// A cell, in the pixels the encoder counts in. Any size does: only cells
/// are reported, and the middle of a cell is never on an edge.
const CELL: (u32, u32) = (10, 20);

/// The bytes for `input` on a screen of `cols` by `rows` cells, encoded the
/// way the program on `terminal` asked; empty when it asked for no mouse
/// event of that kind.
pub(crate) fn encode_mouse(
    terminal: &Terminal<'_, '_>,
    input: &MouseInput,
    (cols, rows): (u16, u16),
) -> Result<Vec<u8>, Error> {
    let (action, button, held) = match input.kind {
        MouseKind::Press(button) => (Action::Press, Some(vt(button)), true),
        MouseKind::Release(button) => (Action::Release, Some(vt(button)), false),
        MouseKind::Drag(button) => (Action::Motion, Some(vt(button)), true),
        MouseKind::Move => (Action::Motion, None, false),
        // A wheel is two buttons that are only ever pressed.
        MouseKind::WheelUp => (Action::Press, Some(Button::Four), false),
        MouseKind::WheelDown => (Action::Press, Some(Button::Five), false),
        MouseKind::WheelLeft => (Action::Press, Some(Button::Six), false),
        MouseKind::WheelRight => (Action::Press, Some(Button::Seven), false),
    };
    let mut mods = Mods::empty();
    if input.shift {
        mods |= Mods::SHIFT;
    }
    if input.alt {
        mods |= Mods::ALT;
    }
    if input.ctrl {
        mods |= Mods::CTRL;
    }
    let mut event = Event::new()?;
    event
        .set_action(action)
        .set_button(button)
        .set_mods(mods)
        .set_position(Position {
            x: middle(input.column, CELL.0),
            y: middle(input.row, CELL.1),
        });
    let mut encoder = Encoder::new()?;
    encoder
        .set_options_from_terminal(terminal)
        .set_size(EncoderSize {
            screen_width: u32::from(cols.max(1)) * CELL.0,
            screen_height: u32::from(rows.max(1)) * CELL.1,
            cell_width: CELL.0,
            cell_height: CELL.1,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        })
        .set_any_button_pressed(held);
    let mut bytes = Vec::new();
    encoder.encode_to_vec(&event, &mut bytes)?;
    Ok(bytes)
}

fn vt(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Middle => Button::Middle,
        MouseButton::Right => Button::Right,
    }
}

/// The middle of cell number `index`, in pixels, along an axis where a cell
/// is `size` pixels.
fn middle(index: u16, size: u32) -> f32 {
    // Screens are at most a thousand cells: the product fits a float exactly.
    let pixels = u32::from(index) * size + size / 2;
    f32::from(u16::try_from(pixels).unwrap_or(u16::MAX))
}
