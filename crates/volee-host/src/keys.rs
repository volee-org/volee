//! Find the leave keys, Ctrl+Space then q unless configured otherwise, in
//! the person's keys, in every encoding (ADR 0021).

/// What a chunk of keys holds once the leave keys are taken out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filtered {
    /// The keys to send to the program, the leave keys removed.
    pub forward: Vec<u8>,
    /// Whether the person left.
    pub detach: bool,
}

/// The keys that leave a program taken over from a terminal (ADR 0021).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leave {
    /// Control and a letter leaves at once.
    Key(char),
    /// Control and `prefix` (a letter, or ' ' for Control and Space) opens a
    /// menu; `then`, a plain letter, leaves.
    Menu {
        /// The letter, or ' ', that Control presses to open the menu.
        prefix: char,
        /// The plain letter that leaves once the menu is open.
        then: char,
    },
}

impl Default for Leave {
    /// Control and Space, then q, like tmux's prefix.
    fn default() -> Self {
        Self::Menu {
            prefix: ' ',
            then: 'q',
        }
    }
}

impl Leave {
    /// The configuration itself when it is valid, the default otherwise.
    fn normalized(self) -> Self {
        match self {
            Self::Key(letter) if valid_leave(letter) => Self::Key(letter.to_ascii_lowercase()),
            Self::Menu { prefix, then }
                if (prefix == ' ' || valid_leave(prefix)) && then.is_ascii_alphabetic() =>
            {
                Self::Menu {
                    prefix: prefix.to_ascii_lowercase(),
                    then: then.to_ascii_lowercase(),
                }
            }
            _ => Self::default(),
        }
    }
}

/// Removes the leave keys from keys read from a terminal, one read at a time.
///
/// With [`Leave::Key`], Control and the letter leave at once. With
/// [`Leave::Menu`], Control and the prefix arm a menu that swallows its
/// key: the `then` letter leaves, the prefix again sends the prefix to the
/// program, Escape cancels, and any other key cancels and is dropped, as
/// tmux drops an unbound key after its prefix.
///
/// Recognized in every encoding: the control byte (`0x00` for Space), the
/// kitty keyboard protocol (`CSI code ; mods [: event] u`, with any alternate
/// keys, text and lock modifiers) and xterm's `modifyOtherKeys`
/// (`CSI 27 ; mods ; code ~`). A key repeat or release is swallowed without
/// leaving. A control sequence cut between two reads is held until it is
/// complete, and anything inside a bracketed paste is pasted, not obeyed.
#[derive(Clone, Debug)]
pub struct KeyFilter {
    /// The configuration, normalized.
    leave: Leave,
    /// The start of a control sequence (`ESC [` and parameters) that the
    /// last read cut, or an incomplete character.
    pending: Vec<u8>,
    /// Inside a bracketed paste (`CSI 200 ~` to `CSI 201 ~`).
    pasting: bool,
    /// Menu only: the bytes of the prefix press, while the menu is open.
    armed: Option<Vec<u8>>,
}

impl Default for KeyFilter {
    fn default() -> Self {
        Self::new(Leave::default())
    }
}

/// The letter of the single-key form when nothing else is configured: Ctrl+Q.
pub const DEFAULT_LEAVE: char = 'q';

/// Whether Control and `letter` can leave: a letter whose Control byte is
/// not also Backspace, Tab, Line Feed or Enter.
pub fn valid_leave(letter: char) -> bool {
    letter.is_ascii_alphabetic() && !matches!(letter.to_ascii_lowercase(), 'h' | 'i' | 'j' | 'm')
}

/// The longest incomplete control sequence held for the next read.
const MAX_PENDING: usize = 32;

/// The modifier bits of Control in a kitty or xterm modifier value.
const CTRL: u16 = 0b100;
/// No modifier bits (Caps Lock and Num Lock do not count).
const NO_MODS: u16 = 0;

/// What the filter does with one key or sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// This many bytes were taken; what they forward is already in the output.
    Took(usize),
    /// The bytes are incomplete: hold them for the next read.
    Wait,
    /// The person left.
    Detach,
}

impl KeyFilter {
    /// A filter for the leave keys. A configuration that is not valid (see
    /// [`valid_leave`]) falls back to the default, Ctrl+Space then q.
    pub fn new(leave: Leave) -> Self {
        Self {
            leave: leave.normalized(),
            pending: Vec::new(),
            pasting: false,
            armed: None,
        }
    }

    /// Filters one read of keys.
    pub fn filter(&mut self, read: &[u8]) -> Filtered {
        let mut keys = std::mem::take(&mut self.pending);
        keys.extend_from_slice(read);
        let mut out = Filtered::default();
        let mut i = 0;
        while i < keys.len() {
            let rest = &keys[i..];
            let step = if rest[0] == 0x1b && rest.get(1) == Some(&b'[') {
                self.control_sequence(rest, &mut out.forward)
            } else {
                self.key_byte(rest, &mut out.forward)
            };
            match step {
                Step::Took(len) => i += len,
                Step::Wait => {
                    self.pending = rest.to_vec();
                    return out;
                }
                Step::Detach => {
                    out.detach = true;
                    return out;
                }
            }
        }
        out
    }

    /// Handles the control sequence at the start of `keys` (which starts
    /// with `ESC [`).
    fn control_sequence(&mut self, keys: &[u8], out: &mut Vec<u8>) -> Step {
        let Some(end) = keys[2..]
            .iter()
            .position(|byte| (0x40..=0x7e).contains(byte))
        else {
            // Cut by the read: wait for the rest, unless it is too long to
            // be a key.
            if keys.len() <= MAX_PENDING {
                return Step::Wait;
            }
            out.extend_from_slice(keys);
            return Step::Took(keys.len());
        };
        let seq = &keys[..3 + end];
        if self.sequence(seq, out) {
            Step::Detach
        } else {
            Step::Took(seq.len())
        }
    }

    /// Handles one complete control sequence (`ESC [` to its final byte);
    /// returns whether the person left.
    fn sequence(&mut self, seq: &[u8], out: &mut Vec<u8>) -> bool {
        let body = &seq[2..];
        if self.pasting {
            self.pasting = body != b"201~";
            out.extend_from_slice(seq);
            return false;
        }
        if body == b"200~" {
            // Not a key: it ends any menu and starts the paste.
            self.armed = None;
            self.pasting = true;
            out.extend_from_slice(seq);
            return false;
        }
        let key = seq_key(body);
        match self.leave {
            Leave::Key(letter) => {
                if let Some(key) = key
                    && key.is(key_code(letter), CTRL)
                {
                    // A repeat or a release of the leave key is swallowed.
                    return key.press();
                }
                out.extend_from_slice(seq);
                false
            }
            Leave::Menu { prefix, then } => self.menu_sequence(seq, key, prefix, then, out),
        }
    }

    /// The menu's reaction to one control sequence; returns whether the
    /// person left.
    fn menu_sequence(
        &mut self,
        seq: &[u8],
        key: Option<SeqKey>,
        prefix: char,
        then: char,
        out: &mut Vec<u8>,
    ) -> bool {
        let Some(key) = key else {
            // Not a key the menu knows (an arrow, a function key): it ends
            // an open menu, and is forwarded otherwise.
            if self.armed.take().is_none() {
                out.extend_from_slice(seq);
            }
            return false;
        };
        if let Some(first) = self.armed.take() {
            if !key.press() {
                // A repeat or a release of any key: swallowed, the menu
                // stays open.
                self.armed = Some(first);
            } else if key.is(key_code(prefix), CTRL) {
                // The prefix again: the first press goes to the program.
                out.extend_from_slice(&first);
            } else {
                return key.is(key_code(then), NO_MODS);
            }
            return false;
        }
        if key.is(key_code(prefix), CTRL) {
            if key.press() {
                self.armed = Some(seq.to_vec());
            }
            return false;
        }
        out.extend_from_slice(seq);
        false
    }

    /// Handles one key that is not a control sequence, at the start of
    /// `keys`.
    fn key_byte(&mut self, keys: &[u8], out: &mut Vec<u8>) -> Step {
        let byte = keys[0];
        match self.leave {
            Leave::Key(letter) => {
                if byte == control_byte(letter) && !self.pasting {
                    return Step::Detach;
                }
                out.push(byte);
                Step::Took(1)
            }
            Leave::Menu { prefix, then } => self.menu_byte(keys, prefix, then, out),
        }
    }

    /// The menu's reaction to one byte that starts a key (not `ESC [`).
    fn menu_byte(&mut self, keys: &[u8], prefix: char, then: char, out: &mut Vec<u8>) -> Step {
        let byte = keys[0];
        if self.pasting {
            out.push(byte);
            return Step::Took(1);
        }
        let Some(first) = self.armed.take() else {
            if byte == control_byte(prefix) {
                self.armed = Some(vec![byte]);
            } else {
                out.push(byte);
            }
            return Step::Took(1);
        };
        // The menu is open: this key decides, and closes the menu.
        if byte == control_byte(prefix) {
            out.extend_from_slice(&first);
            return Step::Took(1);
        }
        if byte == ascii_byte(then) {
            return Step::Detach;
        }
        if byte >= 0x80 {
            // A whole character is one key; wait for its last bytes if the
            // read cut it.
            let len = utf8_len(byte);
            if len > keys.len() {
                self.armed = Some(first);
                return Step::Wait;
            }
            return Step::Took(len);
        }
        // A lone Escape cancels the menu and forwards nothing; any other
        // key is dropped.
        Step::Took(1)
    }
}

/// Filters keys that arrived in one read, with the default leave keys; see
/// [`KeyFilter`].
pub fn filter(keys: &[u8]) -> Filtered {
    KeyFilter::default().filter(keys)
}

/// The byte of an ASCII character (0 for anything else, which the callers
/// never pass).
fn ascii_byte(c: char) -> u8 {
    u8::try_from(u32::from(c)).unwrap_or(0)
}

/// The byte Control and `c` sends without a keyboard protocol: Control
/// clears the bits above `0x1f`, which maps Space to NUL.
fn control_byte(c: char) -> u8 {
    ascii_byte(c) & 0x1f
}

/// The key code of `c` in the kitty protocol and in xterm: its lowercase
/// character, Space being 32.
fn key_code(c: char) -> u32 {
    u32::from(c.to_ascii_lowercase())
}

/// The length of the character that starts with `lead`.
fn utf8_len(lead: u8) -> usize {
    match lead {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

/// A key that a control sequence carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SeqKey {
    /// The key code.
    code: u32,
    /// The encoded modifiers: 1 plus the modifier bits, absent meaning 1.
    mods: u16,
    /// The kitty event: 1 press, 2 repeat, 3 release.
    event: u8,
}

impl SeqKey {
    /// Whether this is key `code` with exactly the modifier bits `bits`.
    fn is(self, code: u32, bits: u16) -> bool {
        self.code == code
            && self
                .mods
                .checked_sub(1)
                .is_some_and(|mods| mods & 0b11_1111 == bits)
    }

    /// Whether this is a press, not a repeat or a release.
    fn press(self) -> bool {
        self.event == 1
    }
}

/// The key of a control sequence body (after `ESC [`, final byte included):
/// kitty `code[:alternates] ; mods[:event] ; text u`, or xterm
/// `27 ; mods ; code ~`.
fn seq_key(body: &[u8]) -> Option<SeqKey> {
    let end = body.iter().position(|byte| (0x40..=0x7e).contains(byte))?;
    let params = std::str::from_utf8(&body[..end]).ok()?;
    let mut fields = params.split(';');
    match body[end] {
        b'u' => {
            let code: u32 = fields.next()?.split(':').next()?.parse().ok()?;
            let mut modifiers = fields.next().unwrap_or("1").split(':');
            let mods: u16 = modifiers.next()?.parse().ok()?;
            let event: u8 = modifiers
                .next()
                .map_or(Some(1), |event| event.parse().ok())?;
            Some(SeqKey { code, mods, event })
        }
        b'~' => {
            let (first, mods, code) = (fields.next()?, fields.next()?, fields.next()?);
            if first != "27" {
                return None;
            }
            Some(SeqKey {
                code: code.parse().ok()?,
                mods: mods.parse().ok()?,
                event: 1,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The single-key form, Control and q.
    fn single_q() -> KeyFilter {
        KeyFilter::new(Leave::Key('q'))
    }

    fn detaches(keys: &[u8]) -> bool {
        single_q().filter(keys).detach
    }

    #[test]
    fn plain_keys_pass() {
        let out = filter(b"hello\x1b[A\x1b");
        assert_eq!(out.forward, b"hello\x1b[A\x1b");
        assert!(!out.detach);
    }

    #[test]
    fn the_plain_byte_detaches_and_drops_what_follows() {
        let out = single_q().filter(b"ab\x11cd");
        assert_eq!(out.forward, b"ab");
        assert!(out.detach);
    }

    #[test]
    fn kitty_encodings_detach() {
        assert!(detaches(b"\x1b[113;5u"));
        assert!(detaches(b"\x1b[113;5:1u"));
        assert!(detaches(b"\x1b[113:81;5u"));
        assert!(detaches(b"\x1b[113;69u"), "with caps lock");
        assert!(detaches(b"\x1b[113;133u"), "with num lock");
        assert!(detaches(b"\x1b[27;5;113~"), "modifyOtherKeys");
    }

    #[test]
    fn repeats_and_releases_are_swallowed() {
        let out = single_q().filter(b"x\x1b[113;5:3uy");
        assert_eq!(out.forward, b"xy");
        assert!(!out.detach);
        assert!(!detaches(b"\x1b[113;5:2u"));
    }

    #[test]
    fn a_sequence_cut_between_reads_is_still_seen() {
        let mut filter = single_q();
        let first = filter.filter(b"ab\x1b[113;5");
        assert_eq!(first.forward, b"ab");
        assert!(!first.detach);
        assert!(filter.filter(b"u").detach);
        let mut other = single_q();
        assert_eq!(other.filter(b"\x1b[1").forward, b"");
        assert_eq!(other.filter(b"A").forward, b"\x1b[1A");
    }

    #[test]
    fn another_letter_leaves_in_every_encoding() {
        for keys in [&b"\x07"[..], b"\x1b[103;5u", b"\x1b[27;5;103~"] {
            assert!(
                KeyFilter::new(Leave::Key('g')).filter(keys).detach,
                "{keys:?}"
            );
        }
        // Ctrl+Q then reaches the agent.
        let out = KeyFilter::new(Leave::Key('G')).filter(b"\x11");
        assert_eq!(out.forward, b"\x11");
        assert!(!out.detach);
        // Letters that are also Backspace, Tab, Line Feed or Enter are not
        // valid single keys.
        assert!(!valid_leave('m') && !valid_leave('1') && valid_leave('q'));
    }

    #[test]
    fn ctrl_q_inside_a_paste_is_pasted() {
        let mut filter = single_q();
        let out = filter.filter(b"\x1b[200~a\x11b\x1b[201~");
        assert_eq!(out.forward, b"\x1b[200~a\x11b\x1b[201~");
        assert!(!out.detach);
        assert!(filter.filter(b"\x11").detach, "after the paste it detaches");
    }

    #[test]
    fn other_q_keys_pass() {
        for keys in [
            &b"\x1b[113u"[..],
            b"\x1b[113;3u",
            b"\x1b[113;7u",
            b"\x1b[113;6u",
            b"\x1b[27;3;113~",
        ] {
            let out = single_q().filter(keys);
            assert_eq!(out.forward, keys, "{keys:?}");
            assert!(!out.detach);
        }
    }

    #[test]
    fn ctrl_space_then_q_detaches() {
        let out = KeyFilter::default().filter(b"ab\x00q");
        assert_eq!(out.forward, b"ab");
        assert!(out.detach);
        assert!(KeyFilter::default().filter(b"\x1b[32;5u\x1b[113u").detach);
        assert!(
            KeyFilter::default()
                .filter(b"\x1b[32;5u\x1b[113;1:1u")
                .detach
        );
    }

    #[test]
    fn the_prefix_twice_sends_the_prefix_to_the_program() {
        let out = KeyFilter::default().filter(b"\x00\x00");
        assert_eq!(out.forward, b"\x00");
        assert!(!out.detach);
        let out = KeyFilter::default().filter(b"\x1b[32;5u\x1b[32;5u");
        assert_eq!(out.forward, b"\x1b[32;5u");
        assert!(!out.detach);
        // The menu is closed afterwards: q is an ordinary key.
        assert_eq!(KeyFilter::default().filter(b"\x00\x00q").forward, b"\x00q");
    }

    #[test]
    fn the_prefix_then_another_key_drops_both() {
        let mut filter = KeyFilter::default();
        let out = filter.filter(b"\x00x");
        assert_eq!(out.forward, b"");
        assert!(!out.detach);
        assert_eq!(filter.filter(b"y").forward, b"y");
        let mut filter = KeyFilter::default();
        assert_eq!(filter.filter(b"\x00\x1b[97uy").forward, b"y");
        assert!(!filter.filter(b"").detach);
    }

    #[test]
    fn the_prefix_and_q_in_two_reads_detach() {
        let mut filter = KeyFilter::default();
        let first = filter.filter(b"\x00");
        assert_eq!(first.forward, b"");
        assert!(!first.detach);
        assert!(filter.filter(b"q").detach);
    }

    #[test]
    fn a_prefix_release_while_armed_is_swallowed() {
        let out = KeyFilter::default().filter(b"\x1b[32;5u\x1b[32;5:3uq");
        assert_eq!(out.forward, b"");
        assert!(out.detach);
        let mut filter = KeyFilter::default();
        assert!(!filter.filter(b"\x1b[32;5u").detach);
        assert_eq!(filter.filter(b"\x1b[113;1:3u").forward, b"");
        assert!(filter.filter(b"q").detach, "a release does not disarm");
    }

    #[test]
    fn escape_after_the_prefix_cancels() {
        let mut filter = KeyFilter::default();
        assert_eq!(filter.filter(b"\x00").forward, b"");
        let out = filter.filter(b"\x1b");
        assert_eq!(out.forward, b"");
        assert!(!out.detach);
        assert_eq!(filter.filter(b"q").forward, b"q", "the menu is closed");
    }

    #[test]
    fn the_prefix_inside_a_paste_is_pasted() {
        let mut filter = KeyFilter::default();
        let out = filter.filter(b"\x1b[200~a\x00q\x1b[201~");
        assert_eq!(out.forward, b"\x1b[200~a\x00q\x1b[201~");
        assert!(!out.detach);
        assert!(
            filter.filter(b"\x00q").detach,
            "after the paste the menu works"
        );
    }

    #[test]
    fn a_custom_menu() {
        let leave = Leave::Menu {
            prefix: 'a',
            then: 'x',
        };
        assert!(KeyFilter::new(leave).filter(b"\x01x").detach);
        assert!(KeyFilter::new(leave).filter(b"\x1b[97;5ux").detach);
        assert_eq!(KeyFilter::new(leave).filter(b"\x01\x01").forward, b"\x01");
        // Space is no longer the prefix, and q is no longer the letter.
        assert_eq!(KeyFilter::new(leave).filter(b"\x00q").forward, b"\x00q");
        let out = KeyFilter::new(leave).filter(b"\x01q");
        assert_eq!(out.forward, b"");
        assert!(!out.detach);
    }

    #[test]
    fn invalid_configurations_fall_back_to_the_default() {
        for leave in [
            Leave::Key('m'),
            Leave::Key('1'),
            Leave::Menu {
                prefix: 'm',
                then: 'q',
            },
            Leave::Menu {
                prefix: ' ',
                then: '1',
            },
        ] {
            assert!(KeyFilter::new(leave).filter(b"\x00q").detach, "{leave:?}");
        }
    }
}
