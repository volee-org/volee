//! Text measured in terminal cells, not in bytes or characters.

use std::borrow::Cow;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The width of `text` in terminal cells.
pub(crate) fn width(text: &str) -> usize {
    text.width()
}

/// `text` cut to at most `max` cells, ending with `…` when something was cut.
pub(crate) fn truncate(text: &str, max: usize) -> Cow<'_, str> {
    if text.width() <= max {
        return Cow::Borrowed(text);
    }
    if max == 0 {
        return Cow::Borrowed("");
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    Cow::Owned(out)
}

/// `text` cut from its start to at most `max` cells, beginning with `…`.
/// Suits paths, whose end matters most.
pub(crate) fn truncate_start(text: &str, max: usize) -> Cow<'_, str> {
    if text.width() <= max {
        return Cow::Borrowed(text);
    }
    if max == 0 {
        return Cow::Borrowed("");
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    for c in text.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        kept.push(c);
        used += w;
    }
    let mut out = String::from("…");
    out.extend(kept.iter().rev());
    Cow::Owned(out)
}

/// `text` with a capital first letter, to start a sentence.
pub(crate) fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `text` without its final period, to go inside a longer sentence.
pub(crate) fn clause(text: &str) -> &str {
    text.trim_end().trim_end_matches('.')
}

/// The rows of `text` in a field `width` cells wide, as byte ranges without
/// the line breaks. Long lines break after the last space that fits, or
/// anywhere when a word is longer than a row.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut line_start = 0;
    for line in text.split('\n') {
        let line_end = line_start + line.len();
        let mut start = line_start;
        loop {
            let mut used = 0;
            let mut fit = start;
            let mut space = None;
            for (index, c) in text[start..line_end].char_indices() {
                let cells = c.width().unwrap_or(0);
                if used + cells > width {
                    break;
                }
                used += cells;
                if c == ' ' {
                    space = Some(start + index);
                }
                fit = start + index + c.len_utf8();
            }
            if fit >= line_end {
                rows.push((start, line_end));
                break;
            }
            if text[fit..].starts_with(' ') {
                rows.push((start, fit));
                start = fit + 1;
            } else if let Some(space) = space.filter(|space| *space > start) {
                rows.push((start, space));
                start = space + 1;
            } else {
                // A character wider than the row still takes a row.
                let fit = if fit == start {
                    start + text[start..].chars().next().map_or(1, char::len_utf8)
                } else {
                    fit
                };
                rows.push((start, fit));
                start = fit;
            }
        }
        line_start = line_end + 1;
    }
    rows
}

/// The row and the column of the cursor among `rows`. At a soft break the
/// cursor goes to the start of the next row.
pub(crate) fn cursor_position(
    rows: &[(usize, usize)],
    text: &str,
    cursor: usize,
) -> (usize, usize) {
    let mut found = None;
    for (row, (start, end)) in rows.iter().enumerate() {
        if *start <= cursor && cursor < *end {
            found = Some((row, *start));
            break;
        }
        if cursor == *end {
            found = Some((row, *start));
        }
    }
    let (row, start) = found.unwrap_or((rows.len().saturating_sub(1), cursor));
    (row, width(&text[start..cursor.max(start)]))
}

/// `words` as one command line that a POSIX shell or fish runs as is: a word
/// stays as it is when no shell treats its characters specially, and is
/// single-quoted otherwise, such as an empty word or a name with a space.
pub(crate) fn command_line(words: &[String]) -> String {
    words
        .iter()
        .map(|word| shell_word(word))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_word(word: &str) -> Cow<'_, str> {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-./:,+@%".contains(&byte));
    if plain {
        Cow::Borrowed(word)
    } else {
        Cow::Owned(format!("'{}'", word.replace('\'', r"'\''")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines_quote_only_what_a_shell_would_split() {
        let words = |list: &[&str]| {
            list.iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            command_line(&words(&["tuios", "attach", "work"])),
            "tuios attach work"
        );
        assert_eq!(
            command_line(&words(&["tuios", "--server", "", "attach", "my work"])),
            "tuios --server '' attach 'my work'"
        );
        assert_eq!(command_line(&words(&["it's"])), r"'it'\''s'");
    }

    #[test]
    fn messages_fit_into_sentences() {
        assert_eq!(sentence("lost the daemon"), "Lost the daemon");
        assert_eq!(sentence("élan"), "Élan");
        assert_eq!(sentence(""), "");
        assert_eq!(clause("it runs elsewhere. "), "it runs elsewhere");
        assert_eq!(clause("no period"), "no period");
    }

    #[test]
    fn short_text_is_kept() {
        assert_eq!(truncate("billing", 7), "billing");
        assert_eq!(truncate_start("billing", 10), "billing");
    }

    #[test]
    fn long_text_ends_with_an_ellipsis() {
        assert_eq!(truncate("invoice-schema", 8), "invoice…");
        assert_eq!(width(&truncate("invoice-schema", 8)), 8);
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(truncate("abc", 1), "…");
    }

    #[test]
    fn paths_keep_their_end() {
        assert_eq!(truncate_start("/home/dev/code/billing", 12), "…ode/billing");
    }

    #[test]
    fn wide_characters_count_twice() {
        assert_eq!(width("日本"), 4);
        assert_eq!(truncate("日本語です", 5), "日本…");
        assert_eq!(truncate_start("日本語です", 5), "…です");
    }
}
