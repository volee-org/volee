//! A small multi-line text field: the text, a cursor, and the edits a
//! message composer needs. Pure, so it is tested without a terminal.

use crate::keymap::{Action, Context, KeyCombo, Keymap};

/// Text and a cursor, a byte offset that always sits on a character
/// boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TextField {
    text: String,
    cursor: usize,
}

impl TextField {
    /// The text.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The cursor, as a byte offset into the text.
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether there is nothing but white space.
    pub(crate) fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Inserts text at the cursor and moves the cursor after it.
    pub(crate) fn insert(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    /// Deletes the character before the cursor.
    pub(crate) fn backspace(&mut self) {
        if let Some(previous) = self.previous_boundary() {
            self.text.replace_range(previous..self.cursor, "");
            self.cursor = previous;
        }
    }

    /// Deletes the character under the cursor.
    pub(crate) fn delete(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.text.replace_range(self.cursor..next, "");
        }
    }

    /// Moves one character left.
    pub(crate) fn left(&mut self) {
        if let Some(previous) = self.previous_boundary() {
            self.cursor = previous;
        }
    }

    /// Moves one character right.
    pub(crate) fn right(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.cursor = next;
        }
    }

    /// Moves to the start of the line.
    pub(crate) fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    /// Moves to the end of the line.
    pub(crate) fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    /// Moves to the line above, at the same column or the end of that line.
    pub(crate) fn up(&mut self) {
        let start = self.line_start(self.cursor);
        if start == 0 {
            self.cursor = 0;
            return;
        }
        let column = self.text[start..self.cursor].chars().count();
        let above = self.line_start(start - 1);
        self.cursor = self.at_column(above, column);
    }

    /// Moves to the line below, at the same column or the end of that line.
    pub(crate) fn down(&mut self) {
        let end = self.line_end(self.cursor);
        if end == self.text.len() {
            self.cursor = end;
            return;
        }
        let column = self.text[self.line_start(self.cursor)..self.cursor]
            .chars()
            .count();
        self.cursor = self.at_column(end + 1, column);
    }

    /// Deletes from the start of the line to the cursor.
    pub(crate) fn delete_to_line_start(&mut self) {
        let start = self.line_start(self.cursor);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// Deletes the word before the cursor, and the spaces after it.
    pub(crate) fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches([' ', '\t']);
        let start = trimmed
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        // At the start of a line, the word before is the line break.
        let start = if start == self.cursor {
            self.previous_boundary().unwrap_or(start)
        } else {
            start
        };
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn previous_boundary(&self) -> Option<usize> {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.text[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |index| index + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |index| at + index)
    }

    /// The offset of a column in the line that starts at `start`, or the
    /// end of that line when it is shorter.
    fn at_column(&self, start: usize, column: usize) -> usize {
        let end = self.line_end(start);
        self.text[start..end]
            .char_indices()
            .nth(column)
            .map_or(end, |(index, _)| start + index)
    }
}

/// Applies an editing key to a field: the shortcuts of the keymap's text
/// fields (by default `ctrl-w` and `ctrl-backspace` delete a word, `ctrl-u`
/// the start of the line, `ctrl-a` and `ctrl-e` go to its ends), then
/// characters, deletions and cursor moves. Returns whether the key edits
/// text.
pub(crate) fn apply_key(field: &mut TextField, key: KeyCombo, keymap: &Keymap) -> bool {
    match keymap.action(&[Context::Editing], key) {
        Some(Action::DeleteWord) => field.delete_word(),
        Some(Action::DeleteToLineStart) => field.delete_to_line_start(),
        Some(Action::LineStart) => field.home(),
        Some(Action::LineEnd) => field.end(),
        _ => return apply_plain_key(field, key),
    }
    true
}

/// The keys every text field knows, which are not shortcuts: characters,
/// Backspace, Delete and the arrows.
fn apply_plain_key(field: &mut TextField, key: KeyCombo) -> bool {
    use ratatui::crossterm::event::KeyCode;
    match key.code() {
        KeyCode::Backspace => field.backspace(),
        KeyCode::Delete => field.delete(),
        KeyCode::Left => field.left(),
        KeyCode::Right => field.right(),
        KeyCode::Up => field.up(),
        KeyCode::Down => field.down(),
        _ => match key.typed_char() {
            Some(c) => field.insert(c.encode_utf8(&mut [0; 4])),
            None => return false,
        },
    }
    true
}

/// Pasted text as the composer keeps it: line breaks as `\n`, tabs as
/// spaces, and no other control characters.
pub(crate) fn clean_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field with the cursor where `|` is.
    fn field(marked: &str) -> TextField {
        let cursor = marked.find('|').unwrap();
        TextField {
            text: marked.replacen('|', "", 1),
            cursor,
        }
    }

    /// The field with `|` at the cursor.
    fn marked(field: &TextField) -> String {
        let mut text = field.text.clone();
        text.insert(field.cursor, '|');
        text
    }

    fn after(start: &str, edit: impl Fn(&mut TextField)) -> String {
        let mut field = field(start);
        edit(&mut field);
        marked(&field)
    }

    #[test]
    fn typing_and_deleting_respect_characters() {
        assert_eq!(after("se|", |f| f.insert("ñor ✓")), "señor ✓|");
        assert_eq!(after("señ|", TextField::backspace), "se|");
        assert_eq!(after("|✓x", TextField::delete), "|x");
        assert_eq!(after("|", TextField::backspace), "|");
        assert_eq!(after("ab|", TextField::delete), "ab|");
        assert_eq!(after("é|", TextField::left), "|é");
        assert_eq!(after("|é", TextField::right), "é|");
    }

    #[test]
    fn lines_have_a_start_an_end_and_neighbours() {
        assert_eq!(
            after("one\ntw|o\nthree", TextField::home),
            "one\n|two\nthree"
        );
        assert_eq!(
            after("one\ntw|o\nthree", TextField::end),
            "one\ntwo|\nthree"
        );
        assert_eq!(after("one\ntw|o\nthree", TextField::up), "on|e\ntwo\nthree");
        assert_eq!(
            after("one\ntw|o\nthree", TextField::down),
            "one\ntwo\nth|ree"
        );
        assert_eq!(after("a\nlong|er", TextField::up), "a|\nlonger");
        assert_eq!(after("o|ne", TextField::up), "|one");
        assert_eq!(after("o|ne", TextField::down), "one|");
    }

    #[test]
    fn words_and_lines_can_be_deleted_backwards() {
        assert_eq!(
            after("send the patch  |", TextField::delete_word),
            "send the |"
        );
        assert_eq!(after("one\n|two", TextField::delete_word), "one|two");
        assert_eq!(
            after("first\nsecond li|ne", TextField::delete_to_line_start),
            "first\n|ne"
        );
        let mut field = field("  |");
        assert!(field.is_blank());
        field.insert("x");
        assert!(!field.is_blank());
    }

    #[test]
    fn pasted_text_keeps_line_breaks_only() {
        assert_eq!(clean_paste("a\r\nb\rc\td\u{1b}[0m"), "a\nb\nc    d[0m");
    }
}
