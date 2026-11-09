//! libghostty-vt styles in ratatui's terms.

use libghostty_vt::style::{Style as VtStyle, StyleColor, Underline};
use ratatui::style::{Color, Modifier, Style};

/// Converts a cell's style. Palette colors stay indexed and default colors
/// stay unset, so that the person's terminal theme applies, as it would in
/// the agent's own terminal.
pub(crate) fn to_ratatui(style: &VtStyle) -> Style {
    let mut result = Style::default();
    if let Some(color) = color(style.fg_color) {
        result = result.fg(color);
    }
    if let Some(color) = color(style.bg_color) {
        result = result.bg(color);
    }
    let flags = [
        (style.bold, Modifier::BOLD),
        (style.italic, Modifier::ITALIC),
        (style.faint, Modifier::DIM),
        (style.blink, Modifier::SLOW_BLINK),
        (style.inverse, Modifier::REVERSED),
        (style.invisible, Modifier::HIDDEN),
        (style.strikethrough, Modifier::CROSSED_OUT),
        (
            !matches!(style.underline, Underline::None),
            Modifier::UNDERLINED,
        ),
    ];
    let modifiers = flags
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(Modifier::empty(), |all, (_, modifier)| all | modifier);
    result.add_modifier(modifiers)
}

/// The SGR sequence that sets `style` from a reset state.
pub(crate) fn to_sgr(style: Style) -> Vec<u8> {
    let mut params: Vec<String> = vec!["0".to_owned()];
    let modifiers = [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
        (Modifier::UNDERLINED, "4"),
        (Modifier::SLOW_BLINK, "5"),
        (Modifier::REVERSED, "7"),
        (Modifier::HIDDEN, "8"),
        (Modifier::CROSSED_OUT, "9"),
    ];
    for (modifier, code) in modifiers {
        if style.add_modifier.contains(modifier) {
            params.push(code.to_owned());
        }
    }
    for (color, base) in [(style.fg, 38), (style.bg, 48)] {
        match color {
            Some(Color::Indexed(index)) => params.push(format!("{base};5;{index}")),
            Some(Color::Rgb(r, g, b)) => params.push(format!("{base};2;{r};{g};{b}")),
            _ => {}
        }
    }
    format!("\x1b[{}m", params.join(";")).into_bytes()
}

fn color(color: StyleColor) -> Option<Color> {
    match color {
        StyleColor::None => None,
        StyleColor::Palette(index) => Some(Color::Indexed(index.0)),
        StyleColor::Rgb(rgb) => Some(Color::Rgb(rgb.r, rgb.g, rgb.b)),
    }
}
