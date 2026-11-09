//! Guessing whether the terminal is light or dark, to pick a theme when the
//! person chose none.
//!
//! The terminal is asked for its background color (OSC 11, through
//! `terminal-colorsaurus`, with a timeout). When it does not answer, the
//! `COLORFGBG` variable some terminals set is read. When nothing answers,
//! volee keeps its default theme.

use std::io::IsTerminal;
use std::time::Duration;

use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};

/// How long to wait for the terminal. Terminals that do not support the
/// query are normally detected much sooner; the wait is the worst case, on
/// a slow link.
const TIMEOUT: Duration = Duration::from_millis(700);

/// The brightness of the terminal's background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Background {
    Dark,
    Light,
}

/// The built-in theme that suits a terminal background.
pub(crate) const fn theme_for(background: Background) -> &'static str {
    match background {
        Background::Dark => "catppuccin-mocha",
        Background::Light => "catppuccin-latte",
    }
}

/// The theme for this terminal, or `None` when it cannot be told.
///
/// Never asks when stdin or stdout is not a terminal.
pub(crate) fn guess_theme() -> Option<&'static str> {
    guess().map(theme_for)
}

fn guess() -> Option<Background> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }
    query_terminal().or_else(|| parse_colorfgbg(std::env::var("COLORFGBG").ok().as_deref()?))
}

/// Asks the terminal for its background color.
fn query_terminal() -> Option<Background> {
    let mut options = QueryOptions::default();
    options.timeout = TIMEOUT;
    match theme_mode(options).ok()? {
        ThemeMode::Dark => Some(Background::Dark),
        ThemeMode::Light => Some(Background::Light),
    }
}

/// Reads `COLORFGBG` (`fg;bg`, or `fg;default;bg`): background colors 0 to
/// 6 and 8 are dark, 7 and 9 to 15 are light.
pub(crate) fn parse_colorfgbg(value: &str) -> Option<Background> {
    let background: u8 = value.rsplit(';').next()?.trim().parse().ok()?;
    match background {
        0..=6 | 8 => Some(Background::Dark),
        7 | 9..=15 => Some(Background::Light),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorfgbg_tells_dark_from_light() {
        for (value, expected) in [
            ("15;0", Some(Background::Dark)),
            ("7;0", Some(Background::Dark)),
            ("12;8", Some(Background::Dark)),
            ("0;6", Some(Background::Dark)),
            ("0;7", Some(Background::Light)),
            ("0;15", Some(Background::Light)),
            ("0;9", Some(Background::Light)),
            ("0;default;15", Some(Background::Light)),
            (" 0 ; 15 ", Some(Background::Light)),
        ] {
            assert_eq!(parse_colorfgbg(value), expected, "{value}");
        }
    }

    #[test]
    fn colorfgbg_garbage_is_no_answer() {
        for value in [
            "",
            "15",
            "0;",
            "0;default",
            "0;16",
            "0;-1",
            "light",
            "0;300",
        ] {
            // A lone number has no separator: it is read as the background.
            let expected = (value == "15").then_some(Background::Light);
            assert_eq!(parse_colorfgbg(value), expected, "{value}");
        }
    }

    #[test]
    fn each_background_has_its_theme() {
        assert_eq!(theme_for(Background::Dark), "catppuccin-mocha");
        assert_eq!(theme_for(Background::Light), "catppuccin-latte");
        let builtin: Vec<&str> = volee_tui::Theme::builtin_names().collect();
        assert!(builtin.contains(&theme_for(Background::Dark)));
        assert!(builtin.contains(&theme_for(Background::Light)));
        assert_eq!(theme_for(Background::Dark), volee_tui::Theme::DEFAULT);
    }
}
