//! Keys and pasted text turned into the bytes a program expects, following
//! the modes it turned on (keyboard protocol, cursor keys, bracketed paste).

use libghostty_vt::Terminal;
use libghostty_vt::key::{Action, Encoder, Event, Key, Mods, OptionAsAlt};
use libghostty_vt::terminal::Mode;

use crate::Error;

/// A key pressed, by name, with its modifiers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyInput {
    /// One character (`a`, `A`, `/`, `é`), or a key's name: `enter`, `tab`,
    /// `backspace`, `escape`, `space`, `up`, `down`, `left`, `right`,
    /// `home`, `end`, `pageup`, `pagedown`, `insert`, `delete`, `f1` to
    /// `f12`. Anything else encodes to nothing.
    pub key: String,
    /// Shift was held.
    pub shift: bool,
    /// Alt (Option) was held.
    pub alt: bool,
    /// Control was held.
    pub ctrl: bool,
}

/// The bytes for `input`, encoded the way the program on `terminal` asked.
pub(crate) fn encode_key(terminal: &Terminal<'_, '_>, input: &KeyInput) -> Result<Vec<u8>, Error> {
    let Some((key, text, unshifted)) = describe(&input.key, input.shift) else {
        return Ok(Vec::new());
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
    event.set_action(Action::Press).set_key(key).set_mods(mods);
    if let Some(unshifted) = unshifted {
        event.set_unshifted_codepoint(unshifted);
    }
    if let Some(text) = text {
        // The text already carries Shift (`A`, `!`): the encoder must not
        // apply it again. Shift+Space stays a modified key.
        if input.shift && key != Key::Space {
            event.set_consumed_mods(Mods::SHIFT);
        }
        event.set_utf8(Some(text));
    }
    let mut encoder = Encoder::new()?;
    encoder
        .set_options_from_terminal(terminal)
        // The person's terminal already decided what Option means: a key
        // that arrives with Alt is meant as Alt.
        .set_macos_option_as_alt(OptionAsAlt::True);
    let mut bytes = Vec::new();
    encoder.encode_to_vec(&event, &mut bytes)?;
    Ok(bytes)
}

/// The bytes for pasted `text`: wrapped in bracketed paste when the program
/// turned it on, control characters made harmless.
pub(crate) fn encode_paste(terminal: &Terminal<'_, '_>, text: &str) -> Result<Vec<u8>, Error> {
    let bracketed = terminal.mode(Mode::BRACKETED_PASTE).unwrap_or(false);
    let mut data = text.as_bytes().to_vec();
    // Room for the two bracketed paste markers, six bytes each.
    let mut buf = vec![0; data.len() + 16];
    let written = libghostty_vt::paste::encode(&mut data, bracketed, &mut buf)?;
    buf.truncate(written);
    Ok(buf)
}

/// The key, its text and its unshifted character for a [`KeyInput::key`].
fn describe(name: &str, shift: bool) -> Option<(Key, Option<String>, Option<char>)> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        return Some(character(c, shift));
    }
    let key = match name {
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "escape" => Key::Escape,
        "space" => return Some((Key::Space, Some(" ".to_owned()), Some(' '))),
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        _ => function_key(name)?,
    };
    Some((key, None, None))
}

/// A printable character: its physical key when the US layout says so,
/// else an unidentified key that carries the text. With Shift, a letter is
/// a capital whichever way it came.
fn character(c: char, shift: bool) -> (Key, Option<String>, Option<char>) {
    if c == ' ' {
        return (Key::Space, Some(" ".to_owned()), Some(' '));
    }
    let lower = c.to_ascii_lowercase();
    let c = if shift { c.to_ascii_uppercase() } else { c };
    let key = match lower {
        'a'..='z' => letter(lower),
        '0'..='9' => digit(lower),
        '-' => Key::Minus,
        '=' => Key::Equal,
        '[' => Key::BracketLeft,
        ']' => Key::BracketRight,
        '\\' => Key::Backslash,
        ';' => Key::Semicolon,
        '\'' => Key::Quote,
        ',' => Key::Comma,
        '.' => Key::Period,
        '/' => Key::Slash,
        '`' => Key::Backquote,
        _ => Key::Unidentified,
    };
    let unshifted = if c.is_ascii_alphabetic() { lower } else { c };
    // A control character has no text: the encoder works from the key.
    let text = (!c.is_control()).then(|| c.to_string());
    (key, text, Some(unshifted))
}

fn letter(c: char) -> Key {
    const LETTERS: [Key; 26] = [
        Key::A,
        Key::B,
        Key::C,
        Key::D,
        Key::E,
        Key::F,
        Key::G,
        Key::H,
        Key::I,
        Key::J,
        Key::K,
        Key::L,
        Key::M,
        Key::N,
        Key::O,
        Key::P,
        Key::Q,
        Key::R,
        Key::S,
        Key::T,
        Key::U,
        Key::V,
        Key::W,
        Key::X,
        Key::Y,
        Key::Z,
    ];
    LETTERS
        .get((u32::from(c) - u32::from('a')) as usize)
        .copied()
        .unwrap_or(Key::Unidentified)
}

fn digit(c: char) -> Key {
    const DIGITS: [Key; 10] = [
        Key::Digit0,
        Key::Digit1,
        Key::Digit2,
        Key::Digit3,
        Key::Digit4,
        Key::Digit5,
        Key::Digit6,
        Key::Digit7,
        Key::Digit8,
        Key::Digit9,
    ];
    DIGITS
        .get((u32::from(c) - u32::from('0')) as usize)
        .copied()
        .unwrap_or(Key::Unidentified)
}

fn function_key(name: &str) -> Option<Key> {
    const KEYS: [Key; 12] = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ];
    let number: usize = name.strip_prefix('f')?.parse().ok()?;
    KEYS.get(number.checked_sub(1)?).copied()
}

#[cfg(test)]
mod tests {
    use crate::{KeyInput, Screen};

    fn key(name: &str) -> KeyInput {
        KeyInput {
            key: name.to_owned(),
            ..KeyInput::default()
        }
    }

    fn ctrl(name: &str) -> KeyInput {
        KeyInput {
            ctrl: true,
            ..key(name)
        }
    }

    fn shift(name: &str) -> KeyInput {
        KeyInput {
            shift: true,
            ..key(name)
        }
    }

    fn alt(name: &str) -> KeyInput {
        KeyInput {
            alt: true,
            ..key(name)
        }
    }

    fn encode(screen: &Screen, input: &KeyInput) -> Vec<u8> {
        screen.encode_key(input).expect("encode")
    }

    #[test]
    fn plain_keys_follow_the_legacy_encoding() {
        let screen = Screen::new(80, 24, 0).expect("screen");
        assert_eq!(encode(&screen, &key("a")), b"a");
        assert_eq!(encode(&screen, &shift("A")), b"A");
        assert_eq!(encode(&screen, &shift("a")), b"A");
        assert_eq!(encode(&screen, &key("é")), "é".as_bytes());
        assert_eq!(encode(&screen, &key("/")), b"/");
        assert_eq!(encode(&screen, &key(" ")), b" ");
        assert_eq!(encode(&screen, &key("space")), b" ");
        assert_eq!(encode(&screen, &key("enter")), b"\r");
        assert_eq!(encode(&screen, &key("tab")), b"\t");
        assert_eq!(encode(&screen, &key("backspace")), b"\x7f");
        assert_eq!(encode(&screen, &key("escape")), b"\x1b");
        assert_eq!(encode(&screen, &ctrl("c")), b"\x03");
        assert_eq!(encode(&screen, &ctrl("z")), b"\x1a");
        assert_eq!(encode(&screen, &alt("b")), b"\x1bb");
        assert_eq!(encode(&screen, &key("up")), b"\x1b[A");
        assert_eq!(encode(&screen, &key("f1")), b"\x1bOP");
        assert_eq!(encode(&screen, &key("nonsense")), b"");
    }

    #[test]
    fn cursor_keys_follow_the_application_mode() {
        let mut screen = Screen::new(80, 24, 0).expect("screen");
        screen.feed(b"\x1b[?1h");
        assert_eq!(encode(&screen, &key("up")), b"\x1bOA");
        assert_eq!(encode(&screen, &key("left")), b"\x1bOD");
    }

    #[test]
    fn keys_follow_the_kitty_protocol_once_the_program_asks() {
        let mut screen = Screen::new(80, 24, 0).expect("screen");
        // Disambiguate escape codes, as Claude Code asks.
        screen.feed(b"\x1b[>1u");
        assert_eq!(encode(&screen, &key("a")), b"a");
        assert_eq!(encode(&screen, &key("escape")), b"\x1b[27u");
        assert_eq!(encode(&screen, &shift("enter")), b"\x1b[13;2u");
        assert_eq!(encode(&screen, &ctrl("c")), b"\x1b[99;5u");
        assert_eq!(encode(&screen, &key("enter")), b"\r");
    }

    #[test]
    fn a_paste_is_bracketed_only_when_the_program_asks() {
        let mut screen = Screen::new(80, 24, 0).expect("screen");
        assert_eq!(screen.encode_paste("one\ntwo").expect("paste"), b"one\rtwo");
        screen.feed(b"\x1b[?2004h");
        assert_eq!(
            screen.encode_paste("one\ntwo").expect("paste"),
            b"\x1b[200~one\ntwo\x1b[201~"
        );
        let pasted = screen.encode_paste("x\x1b[201~y").expect("paste");
        assert!(!pasted[6..pasted.len() - 6].contains(&0x1b), "{pasted:?}");
    }
}
