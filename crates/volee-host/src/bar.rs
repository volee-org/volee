//! The bar at the bottom of a take over: one line that says the terminal is
//! inside volee, which agent it shows, and how to come back.
//!
//! The agent gets every row but the last one. Its output goes through
//! [`confine`], so that its scrolling never reaches the bar, and the bar is
//! drawn again after its output, the cursor and pen put back where the agent
//! left them.

/// What the bar says about the agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttachBar {
    /// The agent's name.
    pub name: String,
    /// Where it works, such as `volee › release`.
    pub place: Option<String>,
}

/// The fewest rows a terminal needs for the bar: the agent keeps at least
/// two.
pub(crate) const MIN_ROWS: u16 = 3;

/// The scrolling region that keeps the agent above the bar.
pub(crate) fn region(agent_rows: u16) -> Vec<u8> {
    format!("\x1b[1;{agent_rows}r").into_bytes()
}

/// `chunk` (complete sequences only) with every reset of the scrolling
/// region brought back above the bar: `CSI r` and a bottom margin past the
/// agent's rows become the agent's own region, and a full or soft reset is
/// followed by it. DEC private `CSI ? ... r` (restore modes) is left alone.
pub(crate) fn confine(chunk: &[u8], agent_rows: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(chunk.len() + 16);
    let mut i = 0;
    while i < chunk.len() {
        if chunk[i] != 0x1b {
            out.push(chunk[i]);
            i += 1;
            continue;
        }
        match chunk.get(i + 1) {
            // RIS: a full reset clears the margins.
            Some(b'c') => {
                out.extend_from_slice(b"\x1bc");
                out.extend_from_slice(&region(agent_rows));
                i += 2;
            }
            Some(b'[') => {
                let start = i + 2;
                let Some(len) = chunk[start..]
                    .iter()
                    .position(|byte| (0x40..=0x7e).contains(byte))
                else {
                    out.extend_from_slice(&chunk[i..]);
                    break;
                };
                let end = start + len;
                let params = &chunk[start..end];
                let whole = &chunk[i..=end];
                match chunk[end] {
                    b'r' if params.iter().all(|b| b.is_ascii_digit() || *b == b';') => {
                        out.extend_from_slice(&margins(params, agent_rows));
                    }
                    // DECSTR, a soft reset, also clears the margins.
                    b'p' if params == b"!" => {
                        out.extend_from_slice(whole);
                        out.extend_from_slice(&region(agent_rows));
                    }
                    _ => out.extend_from_slice(whole),
                }
                i = end + 1;
            }
            _ => {
                out.push(chunk[i]);
                i += 1;
            }
        }
    }
    out
}

/// DECSTBM with its bottom margin kept inside the agent's rows.
fn margins(params: &[u8], agent_rows: u16) -> Vec<u8> {
    let text = std::str::from_utf8(params).unwrap_or("");
    let mut fields = text.split(';');
    let top: u16 = fields
        .next()
        .and_then(|field| field.parse().ok())
        .filter(|top| *top > 0)
        .unwrap_or(1);
    let bottom: u16 = fields
        .next()
        .and_then(|field| field.parse().ok())
        .filter(|bottom| *bottom > 0 && *bottom <= agent_rows)
        .unwrap_or(agent_rows);
    format!("\x1b[{};{bottom}r", top.min(bottom)).into_bytes()
}

/// The words of the bar, cut or padded to `cols` cells.
pub(crate) fn text(bar: &AttachBar, working: bool, leave: char, cols: u16) -> String {
    let place = bar
        .place
        .as_deref()
        .map(|place| format!(" · {place}"))
        .unwrap_or_default();
    let state = if working { "▸ working" } else { "· idle" };
    let leave = leave.to_ascii_uppercase();
    let words = format!(
        " volee │ {}{place} │ {state} │ Ctrl+{leave} back to volee ",
        bar.name
    );
    let width = usize::from(cols);
    let mut line: String = words.chars().take(width).collect();
    let used = line.chars().count();
    line.extend(std::iter::repeat_n(' ', width.saturating_sub(used)));
    line
}

/// Draws the bar on the terminal's last row, then runs `restore`, which
/// puts the agent's cursor and pen back. The cursor hides while the bar is
/// drawn, so it never flickers at the bottom.
pub(crate) fn draw(line: &str, rows: u16, restore: &[u8]) -> Vec<u8> {
    let mut bytes = format!("\x1b[?25l\x1b[{rows};1H\x1b[0m\x1b(B\x1b[1;7m").into_bytes();
    bytes.extend_from_slice(line.as_bytes());
    bytes.extend_from_slice(b"\x1b[0m");
    bytes.extend_from_slice(restore);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confined(chunk: &[u8]) -> String {
        String::from_utf8(confine(chunk, 23)).unwrap()
    }

    #[test]
    fn resets_of_the_scrolling_region_stay_above_the_bar() {
        assert_eq!(confined(b"a\x1b[rb"), "a\x1b[1;23rb");
        assert_eq!(confined(b"\x1b[;r"), "\x1b[1;23r");
        assert_eq!(confined(b"\x1b[0;0r"), "\x1b[1;23r");
        assert_eq!(confined(b"\x1b[5;30r"), "\x1b[5;23r");
        // The agent's own region, inside its rows, stays.
        assert_eq!(confined(b"\x1b[2;10r"), "\x1b[2;10r");
        assert_eq!(confined(b"x\x1bcy"), "x\x1bc\x1b[1;23ry");
        assert_eq!(confined(b"\x1b[!p"), "\x1b[!p\x1b[1;23r");
    }

    #[test]
    fn other_sequences_pass_untouched() {
        for chunk in [
            &b"\x1b[?1049h\x1b[2J\x1b[H"[..],
            b"\x1b[?1;2r",
            b"\x1b[38;2;1;2;3mhi\x1b[0m",
            b"\x1b]0;title\x07",
            "é ✓".as_bytes(),
        ] {
            assert_eq!(confine(chunk, 23), chunk, "{chunk:?}");
        }
    }

    #[test]
    fn the_bar_names_the_agent_and_the_way_back() {
        let bar = AttachBar {
            name: "billing".into(),
            place: Some("shop › feat/invoices".into()),
        };
        let line = text(&bar, true, 'q', 80);
        assert!(line.starts_with(
            " volee │ billing · shop › feat/invoices │ ▸ working │ Ctrl+Q back to volee "
        ));
        assert_eq!(line.chars().count(), 80);
        assert!(text(&bar, false, 'g', 80).contains("· idle │ Ctrl+G back"));
        assert_eq!(text(&bar, true, 'q', 12).chars().count(), 12);
        let drawn = String::from_utf8(draw("x", 24, b"<restore>")).unwrap();
        assert!(drawn.starts_with("\x1b[?25l\x1b[24;1H"));
        assert!(drawn.ends_with("x\x1b[0m<restore>"));
    }
}
