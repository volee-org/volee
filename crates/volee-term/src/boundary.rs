//! Cut a byte stream between complete escape sequences and characters.
//!
//! A host takes snapshots between two chunks of output (ADR 0019). If a chunk
//! ended in the middle of an escape sequence or of a UTF-8 character, a
//! client that starts from that snapshot would receive the rest of the
//! sequence without its beginning and print it as text. [`Boundary`] holds
//! such an incomplete tail back until the next chunk completes it.

/// The longest tail held back. A program that writes a huge string (an image
/// in an APC sequence) is released in pieces rather than held forever.
const MAX_PENDING: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    /// Inside a UTF-8 character, with this many continuation bytes to come.
    Utf8(u8),
    Escape,
    EscapeIntermediate,
    Csi,
    /// Inside OSC, DCS, SOS, PM or APC: ends with BEL or ST (`ESC \`).
    String,
    StringEscape,
}

/// Splits a stream into complete pieces. See the module documentation.
#[derive(Debug)]
pub struct Boundary {
    state: State,
    pending: Vec<u8>,
}

impl Default for Boundary {
    fn default() -> Self {
        Self::new()
    }
}

impl Boundary {
    /// A splitter at the start of a stream.
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            pending: Vec::new(),
        }
    }

    /// Adds `data` and returns the bytes that end on a complete boundary,
    /// keeping an incomplete tail for the next call.
    pub fn push(&mut self, data: &[u8]) -> Vec<u8> {
        let start = self.pending.len();
        self.pending.extend_from_slice(data);
        let mut complete = if self.state == State::Ground {
            Some(start)
        } else {
            None
        };
        for (offset, &byte) in data.iter().enumerate() {
            self.state = step(self.state, byte);
            if self.state == State::Ground {
                complete = Some(start + offset + 1);
            }
        }
        let cut = complete.unwrap_or(0);
        let cut = if self.pending.len() - cut > MAX_PENDING {
            self.pending.len()
        } else {
            cut
        };
        let rest = self.pending.split_off(cut);
        std::mem::replace(&mut self.pending, rest)
    }

    /// Whether some bytes are held back.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

fn step(state: State, byte: u8) -> State {
    // ESC, CAN and SUB interrupt any sequence (ECMA-48), except inside a
    // string where ESC may start the terminator.
    match state {
        State::String => match byte {
            BEL => State::Ground,
            ESC => State::StringEscape,
            _ => State::String,
        },
        State::StringEscape => match byte {
            b'\\' => State::Ground,
            ESC => State::StringEscape,
            _ => State::String,
        },
        _ if byte == ESC => State::Escape,
        _ if byte == 0x18 || byte == 0x1a => State::Ground,
        State::Ground => utf8_start(byte),
        State::Utf8(left) => {
            if (0x80..0xc0).contains(&byte) {
                if left <= 1 {
                    State::Ground
                } else {
                    State::Utf8(left - 1)
                }
            } else {
                // A broken character: start over from this byte.
                utf8_start(byte)
            }
        }
        State::Escape => match byte {
            b'[' => State::Csi,
            b']' | b'P' | b'X' | b'^' | b'_' => State::String,
            0x20..=0x2f => State::EscapeIntermediate,
            _ => State::Ground,
        },
        State::EscapeIntermediate => match byte {
            0x20..=0x2f => State::EscapeIntermediate,
            _ => State::Ground,
        },
        State::Csi => match byte {
            0x40..=0x7e => State::Ground,
            _ => State::Csi,
        },
    }
}

fn utf8_start(byte: u8) -> State {
    match byte {
        0xc0..=0xdf => State::Utf8(1),
        0xe0..=0xef => State::Utf8(2),
        0xf0..=0xf7 => State::Utf8(3),
        _ => State::Ground,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(chunks: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut boundary = Boundary::new();
        chunks.iter().map(|chunk| boundary.push(chunk)).collect()
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(split(&[b"hello", b" world"]), [b"hello".to_vec(), b" world".to_vec()]);
    }

    #[test]
    fn a_csi_cut_in_two_waits_for_its_end() {
        assert_eq!(
            split(&[b"ab\x1b[3", b"1mred"]),
            [b"ab".to_vec(), b"\x1b[31mred".to_vec()]
        );
    }

    #[test]
    fn an_osc_waits_for_bel_or_st() {
        assert_eq!(
            split(&[b"\x1b]0;tit", b"le\x07x", b"\x1b]2;a\x1b", b"\\y"]),
            [
                Vec::new(),
                b"\x1b]0;title\x07x".to_vec(),
                Vec::new(),
                b"\x1b]2;a\x1b\\y".to_vec(),
            ]
        );
    }

    #[test]
    fn a_utf8_character_is_never_cut() {
        let text = "é✳".as_bytes();
        assert_eq!(
            split(&[&text[..1], &text[1..3], &text[3..]]),
            [Vec::new(), "é".as_bytes().to_vec(), "✳".as_bytes().to_vec()]
        );
    }

    #[test]
    fn a_lone_escape_at_the_end_is_held() {
        let mut boundary = Boundary::new();
        assert_eq!(boundary.push(b"x\x1b"), b"x");
        assert!(boundary.has_pending());
        assert_eq!(boundary.push(b"7"), b"\x1b7");
        assert!(!boundary.has_pending());
    }

    #[test]
    fn a_huge_string_is_released_past_the_limit() {
        let mut boundary = Boundary::new();
        let mut data = b"\x1b_G".to_vec();
        data.resize(MAX_PENDING + 10, b'a');
        assert_eq!(boundary.push(&data).len(), data.len());
    }
}
