//! Find Ctrl+Q in the person's keys, in every encoding (ADR 0021).

/// What a chunk of keys holds once Ctrl+Q is taken out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filtered {
    /// The keys to send to the program, Ctrl+Q removed.
    pub forward: Vec<u8>,
    /// Whether the person pressed Ctrl+Q.
    pub detach: bool,
}

/// Removes Ctrl+Q from keys read from a terminal, one read at a time.
///
/// Recognized: the byte `0x11`, the kitty keyboard protocol (`CSI 113 ; 5 u`
/// with any event type, alternate keys, text and lock modifiers), and xterm's
/// `modifyOtherKeys` (`CSI 27 ; 5 ; 113 ~`). A key repeat or release is
/// swallowed without detaching. Keys after a Ctrl+Q are dropped: the person
/// is leaving. A control sequence cut between two reads is held until it is
/// complete, and a `0x11` inside a bracketed paste is pasted, not obeyed.
#[derive(Clone, Debug, Default)]
pub struct KeyFilter {
    /// The start of a control sequence (`ESC [` and parameters) that the
    /// last read cut.
    pending: Vec<u8>,
    /// Inside a bracketed paste (`CSI 200 ~` to `CSI 201 ~`).
    pasting: bool,
}

/// The longest incomplete control sequence held for the next read.
const MAX_PENDING: usize = 32;

impl KeyFilter {
    /// Filters one read of keys.
    pub fn filter(&mut self, read: &[u8]) -> Filtered {
        let mut keys = std::mem::take(&mut self.pending);
        keys.extend_from_slice(read);
        let mut out = Filtered::default();
        let mut i = 0;
        while i < keys.len() {
            if keys[i] == 0x11 && !self.pasting {
                out.detach = true;
                return out;
            }
            if keys[i] == 0x1b && keys.get(i + 1) == Some(&b'[') {
                let rest = &keys[i + 2..];
                let Some(end) = rest.iter().position(|byte| (0x40..=0x7e).contains(byte)) else {
                    // Cut by the read: wait for the rest.
                    if keys.len() - i <= MAX_PENDING {
                        self.pending = keys[i..].to_vec();
                        return out;
                    }
                    out.forward.extend_from_slice(&keys[i..]);
                    return out;
                };
                match &rest[..=end] {
                    b"200~" => self.pasting = true,
                    b"201~" => self.pasting = false,
                    _ => {}
                }
                if !self.pasting
                    && let Some((len, action)) = csi_ctrl_q(rest)
                {
                    match action {
                        Action::Detach => {
                            out.detach = true;
                            return out;
                        }
                        Action::Swallow => {
                            i += 2 + len;
                            continue;
                        }
                    }
                }
                out.forward.extend_from_slice(&keys[i..i + 3 + end]);
                i += 3 + end;
                continue;
            }
            out.forward.push(keys[i]);
            i += 1;
        }
        out
    }
}

/// Filters keys that arrived in one read; see [`KeyFilter`].
pub fn filter(keys: &[u8]) -> Filtered {
    KeyFilter::default().filter(keys)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Detach,
    Swallow,
}

/// If `rest` (after `ESC [`) starts with a Ctrl+Q sequence, its length and
/// what to do with it.
fn csi_ctrl_q(rest: &[u8]) -> Option<(usize, Action)> {
    let end = rest.iter().position(|byte| (0x40..=0x7e).contains(byte))?;
    let params = std::str::from_utf8(&rest[..end]).ok()?;
    let len = end + 1;
    match rest[end] {
        b'u' => {
            // key[:alternates];modifiers[:event][;text]
            let mut fields = params.split(';');
            let key = fields.next()?.split(':').next()?;
            if key != "113" {
                return None;
            }
            let mut modifiers = fields.next().unwrap_or("1").split(':');
            let mods: u16 = modifiers.next()?.parse().ok()?;
            let event: u8 = modifiers.next().map_or(Some(1), |e| e.parse().ok())?;
            if !is_ctrl_only(mods) {
                return None;
            }
            Some((
                len,
                if event == 1 {
                    Action::Detach
                } else {
                    Action::Swallow
                },
            ))
        }
        b'~' => {
            let mut fields = params.split(';');
            let (first, mods, key) = (fields.next()?, fields.next()?, fields.next()?);
            let mods: u16 = mods.parse().ok()?;
            (first == "27" && key == "113" && is_ctrl_only(mods)).then_some((len, Action::Detach))
        }
        _ => None,
    }
}

/// Whether an encoded modifier value means Ctrl alone. Caps Lock and Num Lock
/// (64 and 128 in the kitty encoding) do not count.
fn is_ctrl_only(encoded: u16) -> bool {
    encoded
        .checked_sub(1)
        .is_some_and(|bits| bits & 0b11_1111 == 0b100)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detaches(keys: &[u8]) -> bool {
        filter(keys).detach
    }

    #[test]
    fn plain_keys_pass() {
        let out = filter(b"hello\x1b[A\x1b");
        assert_eq!(out.forward, b"hello\x1b[A\x1b");
        assert!(!out.detach);
    }

    #[test]
    fn the_plain_byte_detaches_and_drops_what_follows() {
        let out = filter(b"ab\x11cd");
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
        let out = filter(b"x\x1b[113;5:3uy");
        assert_eq!(out.forward, b"xy");
        assert!(!out.detach);
        assert!(!detaches(b"\x1b[113;5:2u"));
    }

    #[test]
    fn a_sequence_cut_between_reads_is_still_seen() {
        let mut filter = KeyFilter::default();
        let first = filter.filter(b"ab\x1b[113;5");
        assert_eq!(first.forward, b"ab");
        assert!(!first.detach);
        assert!(filter.filter(b"u").detach);
        let mut other = KeyFilter::default();
        assert_eq!(other.filter(b"\x1b[1").forward, b"");
        assert_eq!(other.filter(b"A").forward, b"\x1b[1A");
    }

    #[test]
    fn ctrl_q_inside_a_paste_is_pasted() {
        let mut filter = KeyFilter::default();
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
            let out = filter(keys);
            assert_eq!(out.forward, keys, "{keys:?}");
            assert!(!out.detach);
        }
    }
}
