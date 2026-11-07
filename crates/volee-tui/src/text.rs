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

#[cfg(test)]
mod tests {
    use super::*;

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
