//! Text written by agents, or read from the repositories they work in, made
//! safe to print in the person's terminal.
//!
//! Agents choose their own words: a worker's name (through `spawn_worker`), a
//! message, a report, an evidence line. Printed as they are, the control
//! characters in them would reach the terminal as commands: an escape
//! sequence can move the cursor, rewrite what is on screen, change the window
//! title or write to the clipboard. The functions here drop them. (The
//! interface needs nothing of the kind: ratatui drops control characters, and
//! an agent's own output is rendered by a terminal emulator.)

use std::borrow::Cow;

/// `text` without control characters, except line breaks and tabs.
pub(crate) fn printable(text: &str) -> Cow<'_, str> {
    map_chars(text, |c| match c {
        '\n' | '\t' => Some(c),
        c if c.is_control() => None,
        c => Some(c),
    })
}

/// `text` on one line, for a table cell or a field: control characters
/// dropped, line breaks and tabs turned into spaces, so that a name cannot
/// draw lines of its own.
pub(crate) fn one_line(text: &str) -> Cow<'_, str> {
    map_chars(text, |c| match c {
        '\n' | '\t' => Some(' '),
        c if c.is_control() => None,
        c => Some(c),
    })
}

/// `text` with each character replaced or dropped by `map`, borrowed when
/// nothing changes.
fn map_chars(text: &str, map: impl Fn(char) -> Option<char>) -> Cow<'_, str> {
    if text.chars().all(|c| map(c) == Some(c)) {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.chars().filter_map(map).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_lose_their_escape_character() {
        // A clipboard write (OSC 52) ended by a bell, then a screen clear.
        assert_eq!(printable("a\x1b]52;c;aGk=\x07b"), "a]52;c;aGk=b");
        assert_eq!(printable("\x1b[2Jdone"), "[2Jdone");
    }

    #[test]
    fn printable_text_keeps_its_line_breaks_and_tabs() {
        assert_eq!(printable("one\n\ttwo\r\n"), "one\n\ttwo\n");
    }

    #[test]
    fn one_line_text_cannot_draw_lines() {
        assert_eq!(one_line("fake\nreport\tdone\r"), "fake report done");
    }

    #[test]
    fn c1_controls_go_and_other_text_stays_borrowed() {
        // U+009B starts a control sequence on some terminals.
        assert_eq!(one_line("x\u{9b}31my"), "x31my");
        assert!(matches!(
            one_line("\u{2713} d\u{e9}j\u{e0} \u{b7} 3 min"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(printable("two\nlines"), Cow::Borrowed(_)));
    }
}
