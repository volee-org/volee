//! Key bindings (ADR 0009, ADR 0025).
//!
//! Every binding is declared once, in one table, with its context, its
//! action, a short label and a description. Key dispatch, the key popups,
//! the hint bar and the key tables of the user guide are all generated from
//! that table, so they cannot drift apart.
//!
//! A binding is a sequence of one to three keys: `q`, `g g`, or `space n`
//! behind the leader key. Users move an action to other keys, in every
//! context or in one, and an empty list unbinds it:
//!
//! ```
//! use volee_tui::keymap::{KeyOverride, Keymap};
//!
//! let keymap = Keymap::with_overrides([
//!     KeyOverride::new("take-over", ["ctrl-a"]),
//!     KeyOverride::new("first", ["g g", "home"]).in_context("agents"),
//!     KeyOverride::new("adopt", Vec::<String>::new()),
//! ])?;
//! # Ok::<(), volee_tui::KeymapError>(())
//! ```

use std::fmt;
use std::str::FromStr;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key and its modifiers.
///
/// Shift is folded into characters: `G` is a capital G, not `shift-g`, so a
/// binding matches whatever keyboard layout produced the character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl KeyCombo {
    /// A plain character key.
    pub(crate) const fn char(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
        }
    }

    /// A character with Control.
    #[cfg(test)]
    pub(crate) const fn ctrl(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
        }
    }

    /// The combo of a key event from the terminal.
    pub fn from_event(event: &KeyEvent) -> Self {
        Self::normalized(event.code, event.modifiers)
    }

    fn normalized(code: KeyCode, modifiers: KeyModifiers) -> Self {
        let mut modifiers =
            modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        let code = match code {
            KeyCode::Char(c) => {
                modifiers.remove(KeyModifiers::SHIFT);
                KeyCode::Char(c)
            }
            KeyCode::BackTab => {
                modifiers.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => {
                modifiers.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            other => other,
        };
        Self { code, modifiers }
    }

    /// The character typed, when the key types one (no Control or Alt).
    pub(crate) fn typed_char(self) -> Option<char> {
        match self.code {
            KeyCode::Char(c)
                if !self
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Some(c)
            }
            _ => None,
        }
    }

    /// The key without modifiers.
    pub(crate) fn code(self) -> KeyCode {
        self.code
    }

    /// The letter of a Control and letter key, such as `q` for `ctrl-q`.
    pub fn ctrl_letter(self) -> Option<char> {
        match self.code {
            KeyCode::Char(c)
                if self.modifiers == KeyModifiers::CONTROL && c.is_ascii_alphabetic() =>
            {
                Some(c.to_ascii_lowercase())
            }
            _ => None,
        }
    }

    /// How the key is shown in popups and the hint bar: like
    /// [`Display`](fmt::Display), with `␣` for the space bar.
    pub(crate) fn short(self) -> String {
        if self.code == KeyCode::Char(' ') && self.modifiers.is_empty() {
            "␣".to_owned()
        } else {
            self.to_string()
        }
    }

    /// A printable character without modifiers, other than space: such keys
    /// are written together in a sequence (`gg`).
    fn plain(self) -> bool {
        matches!(self.code, KeyCode::Char(c) if c != ' ') && self.modifiers.is_empty()
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            f.write_str("ctrl-")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            f.write_str("alt-")?;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            f.write_str("shift-")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::F(n) => write!(f, "f{n}"),
            code => f.write_str(match code {
                KeyCode::Enter => "enter",
                KeyCode::Esc => "esc",
                KeyCode::Tab => "tab",
                KeyCode::BackTab => "shift-tab",
                KeyCode::Backspace => "backspace",
                KeyCode::Delete => "del",
                KeyCode::Insert => "ins",
                KeyCode::Up => "↑",
                KeyCode::Down => "↓",
                KeyCode::Left => "←",
                KeyCode::Right => "→",
                KeyCode::Home => "home",
                KeyCode::End => "end",
                KeyCode::PageUp => "pgup",
                KeyCode::PageDown => "pgdn",
                _ => "?",
            }),
        }
    }
}

/// Why a key could not be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{0}` is not a key (examples: `q`, `ctrl-c`, `shift-tab`, `enter`, `f5`, or a sequence of up \
     to three keys such as `g g` or `space n`)"
)]
pub struct ParseKeyError(String);

impl FromStr for KeyCombo {
    type Err = ParseKeyError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let error = || ParseKeyError(input.to_owned());
        let mut modifiers = KeyModifiers::NONE;
        let mut rest = input;
        loop {
            let lower = rest.to_ascii_lowercase();
            let (modifier, prefix) = if lower.starts_with("ctrl-") {
                (KeyModifiers::CONTROL, "ctrl-")
            } else if lower.starts_with("alt-") {
                (KeyModifiers::ALT, "alt-")
            } else if lower.starts_with("shift-") {
                (KeyModifiers::SHIFT, "shift-")
            } else {
                break;
            };
            if rest.len() == prefix.len() {
                break;
            }
            modifiers |= modifier;
            rest = &rest[prefix.len()..];
        }
        let mut chars = rest.chars();
        let code = match (chars.next(), chars.next()) {
            (Some(c), None) => {
                if modifiers.contains(KeyModifiers::SHIFT) && c.is_ascii_alphabetic() {
                    KeyCode::Char(c.to_ascii_uppercase())
                } else {
                    KeyCode::Char(c)
                }
            }
            (None, _) => return Err(error()),
            _ => match rest.to_ascii_lowercase().as_str() {
                "enter" | "return" => KeyCode::Enter,
                "esc" | "escape" => KeyCode::Esc,
                "tab" => KeyCode::Tab,
                "backtab" => KeyCode::BackTab,
                "backspace" => KeyCode::Backspace,
                "del" | "delete" => KeyCode::Delete,
                "ins" | "insert" => KeyCode::Insert,
                "space" => KeyCode::Char(' '),
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
                "pgup" | "pageup" => KeyCode::PageUp,
                "pgdn" | "pagedown" => KeyCode::PageDown,
                name => match name.strip_prefix('f').map(str::parse::<u8>) {
                    Some(Ok(n @ 1..=24)) => KeyCode::F(n),
                    _ => return Err(error()),
                },
            },
        };
        Ok(Self::normalized(code, modifiers))
    }
}

/// The longest key sequence: three keys, such as `space g s`.
pub const MAX_SEQUENCE: usize = 3;

/// A sequence of one to [`MAX_SEQUENCE`] keys that triggers a binding: `q`,
/// `g g` or `space n`.
///
/// Written with spaces between keys (`g g`, `space n`, `ctrl-w h`); plain
/// characters may also be written together (`gg`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeySeq(Vec<KeyCombo>);

impl KeySeq {
    /// The keys, in the order they are pressed.
    pub(crate) fn keys(&self) -> &[KeyCombo] {
        &self.0
    }

    /// The only key of a one-key sequence.
    pub fn single(&self) -> Option<KeyCombo> {
        match self.0.as_slice() {
            [key] => Some(*key),
            _ => None,
        }
    }

    /// How the sequence is shown in popups and the hint bar: `gg`, `␣n`,
    /// `ctrl-d`.
    pub(crate) fn short(&self) -> String {
        if self.0.iter().all(|key| key.plain() || key.short() == "␣") {
            self.0.iter().map(|key| key.short()).collect()
        } else {
            let keys: Vec<String> = self.0.iter().map(|key| key.short()).collect();
            keys.join(" ")
        }
    }
}

impl From<KeyCombo> for KeySeq {
    fn from(key: KeyCombo) -> Self {
        Self(vec![key])
    }
}

impl fmt::Display for KeySeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.len() > 1 && self.0.iter().all(|key| key.plain()) {
            return self.0.iter().try_for_each(|key| write!(f, "{key}"));
        }
        for (index, key) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{key}")?;
        }
        Ok(())
    }
}

impl FromStr for KeySeq {
    type Err = ParseKeyError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let error = || ParseKeyError(input.to_owned());
        let mut keys = Vec::new();
        for word in input.split_whitespace() {
            match word.parse::<KeyCombo>() {
                Ok(key) => keys.push(key),
                // `gg`: plain characters written together.
                Err(_)
                    if word.chars().count() <= MAX_SEQUENCE
                        && word.chars().all(|c| c.is_ascii_graphic()) =>
                {
                    keys.extend(word.chars().map(KeyCombo::char));
                }
                Err(_) => return Err(error()),
            }
        }
        if keys.is_empty() || keys.len() > MAX_SEQUENCE {
            return Err(error());
        }
        Ok(Self(keys))
    }
}

macro_rules! actions {
    ($($(#[doc = $doc:literal])+ $variant:ident => $name:literal,)+) => {
        /// Something a key can do.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[non_exhaustive]
        pub enum Action {
            $($(#[doc = $doc])+ $variant,)+
        }

        impl Action {
            /// Every action.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The name used in configuration files.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }
        }
    };
}

actions! {
    /// Quit volee.
    Quit => "quit",
    /// Show the keys of the current screen in a popup.
    Help => "help",
    /// Show every key, in every context.
    AllKeys => "all-keys",
    /// Open the command palette.
    Commands => "commands",
    /// Pick a colour theme.
    PickTheme => "pick-theme",
    /// Close, cancel or go back.
    Back => "back",
    /// Focus the next panel.
    NextPane => "next-pane",
    /// Focus the previous panel.
    PreviousPane => "previous-pane",
    /// Show the hierarchy view.
    ShowHierarchy => "show-hierarchy",
    /// Show the network view.
    ShowNetwork => "show-network",
    /// Show the repositories view.
    ShowRepos => "show-repos",
    /// Show the timeline view.
    ShowTimeline => "show-timeline",
    /// Show the previous view.
    PreviousView => "previous-view",
    /// Show the next view.
    NextView => "next-view",
    /// Jump to an agent by its letter.
    Jump => "jump",
    /// Go back to the previously selected agent.
    JumpBack => "jump-back",
    /// Filter agents.
    Filter => "filter",
    /// Select the next agent that matches the filter.
    NextMatch => "next-match",
    /// Select the previous agent that matches the filter.
    PreviousMatch => "previous-match",
    /// Take over the selected agent in this terminal.
    TakeOver => "take-over",
    /// Write a message to the selected agent.
    Message => "message",
    /// Show the selected agent's changes.
    Diff => "diff",
    /// Show the selected agent's screen full size.
    FullScreen => "full-screen",
    /// Forget the selected agent: stop listing it, after asking.
    Forget => "forget",
    /// Move the selected agent's conversation into a volee host.
    Adopt => "adopt",
    /// Show the agents in volee hosts as a wall of live screens.
    ShowWall => "show-wall",
    /// Move down.
    Down => "down",
    /// Move up.
    Up => "up",
    /// Move left.
    Left => "left",
    /// Move right.
    Right => "right",
    /// Go to the first item.
    First => "first",
    /// Go to the last item.
    Last => "last",
    /// Move down half a page.
    HalfPageDown => "half-page-down",
    /// Move up half a page.
    HalfPageUp => "half-page-up",
    /// Move the timeline's cursor to the selected agent's previous event.
    PreviousEvent => "previous-event",
    /// Move the timeline's cursor to the selected agent's next event.
    NextEvent => "next-event",
    /// Show only the selected agent and its descendants.
    FocusSubtree => "focus-subtree",
    /// Show more detail on cards.
    ZoomIn => "zoom-in",
    /// Show less detail on cards.
    ZoomOut => "zoom-out",
    /// Choose the detail level from the available size again.
    ZoomAuto => "zoom-auto",
    /// Refresh the preview or the changes.
    Refresh => "refresh",
    /// Apply what was typed or chosen.
    Apply => "apply",
    /// Send the message.
    Send => "send",
    /// Start a new line in the message.
    NewLine => "new-line",
    /// The next kind of message.
    NextKind => "next-kind",
    /// The previous kind of message.
    PreviousKind => "previous-kind",
    /// Switch between the summary of the changes and the full patch.
    ToggleFull => "toggle-full",
    /// Answer yes to the question on the bottom line.
    Confirm => "confirm",
    /// Show your messages and the questions waiting for you.
    Messages => "messages",
    /// Reply in the open thread.
    Reply => "reply",
    /// Summarize the open thread with a model.
    Summarize => "summarize",
    /// Open the agent's worktree in another program.
    OpenWith => "open-with",
    /// Open the worktree in your editor.
    OpenEditor => "open-editor",
    /// Open the changes in your diff viewer.
    OpenDiff => "open-diff",
    /// Open the worktree in your git interface.
    OpenGit => "open-git",
    /// Open the worktree in an editor with a window of its own.
    OpenIde => "open-ide",
    /// Launch an agent in a new worktree.
    NewAgent => "new-agent",
    /// Create a mission.
    NewMission => "new-mission",
    /// The next field of a dialog.
    NextField => "next-field",
    /// The previous field of a dialog.
    PreviousField => "previous-field",
    /// Launch what a dialog describes: the agent, or the mission.
    Launch => "launch",
    /// The previous option of a choice in a dialog.
    PreviousChoice => "previous-choice",
    /// The next option of a choice in a dialog.
    NextChoice => "next-choice",
    /// Pick or unpick the option under the cursor.
    ToggleChoice => "toggle-choice",
    /// Delete the word before the cursor.
    DeleteWord => "delete-word",
    /// Delete from the start of the line to the cursor.
    DeleteToLineStart => "delete-to-line-start",
    /// Move the cursor to the start of the line.
    LineStart => "line-start",
    /// Move the cursor to the end of the line.
    LineEnd => "line-end",
    /// Leave an agent you took over and come back (Control and a letter).
    Leave => "leave",
}

impl FromStr for Action {
    type Err = KeymapError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|action| action.name() == name)
            .ok_or_else(|| KeymapError::UnknownAction(name.to_owned()))
    }
}

macro_rules! contexts {
    ($($(#[doc = $doc:literal])+ $variant:ident => $name:literal, $title:literal,)+) => {
        /// Where a binding applies. Bindings of the focused panel take
        /// precedence over bindings that apply everywhere; prompts, popups and
        /// windows capture every key.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub(crate) enum Context {
            $($(#[doc = $doc])+ $variant,)+
        }

        impl Context {
            /// Contexts in the order of the help and of the user guide.
            pub(crate) const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The name used in configuration files, such as `agents` in
            /// `[tui.keys.agents]`.
            pub(crate) const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }

            /// The title of the context in the help.
            pub(crate) const fn title(self) -> &'static str {
                match self {
                    $(Self::$variant => $title,)+
                }
            }
        }
    };
}

contexts! {
    /// Everywhere, unless a prompt, a popup or a window is open.
    Global => "global", "Everywhere",
    /// The list of agents.
    Agents => "agents", "Agents list",
    /// The hierarchy view.
    Hierarchy => "hierarchy", "Hierarchy",
    /// The network view.
    Network => "network", "Network",
    /// The repositories view.
    Repos => "repos", "Repositories",
    /// The timeline view.
    Timeline => "timeline", "Timeline",
    /// The detail panel and its preview.
    Detail => "detail", "Detail and preview",
    /// The selected agent's screen, full size.
    Screen => "screen", "Full screen",
    /// The filter prompt.
    Filter => "filter", "Filter prompt",
    /// The message composer.
    Composer => "composer", "Message composer",
    /// The changes window.
    Diff => "diff", "Changes",
    /// A yes or no question on the bottom line.
    Confirm => "confirm", "Questions",
    /// The messages window.
    Inbox => "inbox", "Messages",
    /// The menu of programs to open a worktree with.
    Tools => "tools", "Open with",
    /// A dialog that launches an agent or a mission.
    Dialog => "dialog", "Launch dialogs",
    /// A choice field of a launch dialog, when it has the keyboard.
    Choice => "choice", "Choices in dialogs",
    /// Text fields: the composer, the dialogs, the filter and the palette.
    Editing => "editing", "Text fields",
    /// The popup of the current screen's keys.
    Keys => "keys", "Keys popup",
    /// The list of every key.
    Help => "help", "All keys",
    /// The command palette.
    Commands => "commands", "Command palette",
    /// The theme picker.
    Themes => "themes", "Theme picker",
    /// The wall of live screens.
    Wall => "wall", "Wall of screens",
    /// An agent you took over, in a volee host.
    Attached => "attached", "Taken over",
}

impl Context {
    /// Whether the context is a panel, active together with
    /// [`Global`](Self::Global).
    const fn with_global(self) -> bool {
        matches!(
            self,
            Self::Agents
                | Self::Hierarchy
                | Self::Network
                | Self::Repos
                | Self::Timeline
                | Self::Detail
        )
    }

    /// Whether two contexts can be active at the same time, so that their
    /// keys must not be ambiguous.
    fn together(self, other: Self) -> bool {
        self == other
            || (self == Self::Global && other.with_global())
            || (other == Self::Global && self.with_global())
    }
}

impl FromStr for Context {
    type Err = KeymapError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|context| context.name() == name)
            .ok_or_else(|| KeymapError::UnknownContext(name.to_owned()))
    }
}

/// One line of the binding table.
#[derive(Clone, Debug)]
pub(crate) struct Binding {
    /// The key sequences; the first one is shown in hints and popups.
    pub(crate) keys: Vec<KeySeq>,
    /// What they do.
    pub(crate) action: Action,
    /// Where.
    pub(crate) context: Context,
    /// A word or two for the key popups.
    pub(crate) label: &'static str,
    /// A sentence for the list of every key and the user guide.
    pub(crate) description: &'static str,
    /// A word or two for the hint bar, when the binding belongs there.
    pub(crate) hint: Option<&'static str>,
}

/// One entry of the hint bar, such as `j/k move`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hint {
    /// The keys, joined with `/`.
    pub(crate) keys: String,
    /// What they do.
    pub(crate) text: &'static str,
    /// The action of the first key, `None` for the leader.
    pub(crate) action: Option<Action>,
}

/// What the keys typed so far do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// They make a whole sequence, bound to this action in this context.
    Action(Context, Action),
    /// They start longer sequences: wait for the next key.
    Prefix,
    /// Nothing is bound to them.
    Unbound,
}

/// A key that continues the sequence typed so far, for the which-key popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Next {
    /// The key.
    pub(crate) key: KeyCombo,
    /// The label of its binding, or the name of the group it opens.
    pub(crate) label: &'static str,
    /// Whether it opens a group of longer sequences.
    pub(crate) group: bool,
}

/// Keys of actions shown together, short: `1-4`, `ctrl-d/u`, `tab/⇧tab`,
/// `h/j/k/l`.
pub(crate) fn join_keys(keys: &[String]) -> String {
    let digits: Vec<u32> = keys
        .iter()
        .filter_map(|key| {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c.to_digit(10),
                _ => None,
            }
        })
        .collect();
    if keys.len() > 2
        && digits.len() == keys.len()
        && digits.windows(2).all(|pair| pair[1] == pair[0] + 1)
    {
        return format!("{}-{}", digits[0], digits[digits.len() - 1]);
    }
    let rests: Vec<&str> = keys
        .iter()
        .filter_map(|key| key.strip_prefix("ctrl-"))
        .collect();
    if keys.len() > 1
        && rests.len() == keys.len()
        && rests.iter().all(|rest| rest.chars().count() == 1)
    {
        return format!("ctrl-{}", rests.join("/"));
    }
    keys.iter()
        .map(|key| key.replace("shift-", "⇧"))
        .collect::<Vec<_>>()
        .join("/")
}

/// The name of a group of sequences, shown in the which-key popup.
pub(crate) fn group_name(prefix: &[KeyCombo]) -> &'static str {
    match prefix {
        [key] if *key == KeyCombo::char(' ') => "leader",
        [key] if *key == KeyCombo::char('g') => "go to",
        _ => "more",
    }
}

/// Why a keymap override was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum KeymapError {
    /// The action does not exist.
    #[error("unknown action `{0}` in the key bindings")]
    UnknownAction(String),
    /// The context does not exist.
    #[error(
        "unknown context `{0}` in the key bindings (contexts: global, agents, hierarchy, \
         network, repos, timeline, detail, screen, filter, composer, diff, confirm, inbox, \
         tools, dialog, choice, editing, keys, help, commands, themes, wall, attached)"
    )]
    UnknownContext(String),
    /// The action has no key in that context to move.
    #[error("action `{action}` has no key in `{context}`")]
    NotInContext {
        /// The action.
        action: String,
        /// The context.
        context: String,
    },
    /// A key could not be read.
    #[error("action `{action}`: {source}")]
    InvalidKey {
        /// The action.
        action: String,
        /// The error.
        #[source]
        source: ParseKeyError,
    },
    /// The leave key must be Control and a letter.
    #[error(
        "action `leave`: `{0}` must be Control and a letter, such as `ctrl-q`: it is the only key \
         every terminal sends the same way while an agent owns the keyboard"
    )]
    LeaveKey(String),
    /// Two actions share a key where both apply.
    #[error("`{key}` is bound to both `{first}` and `{second}` ({context})")]
    Conflict {
        /// The key.
        key: String,
        /// Where both apply.
        context: &'static str,
        /// The first action.
        first: &'static str,
        /// The second action.
        second: &'static str,
    },
    /// A sequence starts with a key that is bound on its own where both
    /// apply, so it could never be typed.
    #[error(
        "`{short}` (`{first}`) starts `{long}` (`{second}`) ({context}): a key cannot both act \
         and start a longer sequence"
    )]
    Ambiguous {
        /// The shorter sequence.
        short: String,
        /// Its action.
        first: &'static str,
        /// The longer sequence.
        long: String,
        /// Its action.
        second: &'static str,
        /// Where both apply.
        context: &'static str,
    },
}

/// A change to the default bindings: new keys for an action, in every
/// context where it has keys or in one; no keys unbinds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyOverride {
    action: String,
    context: Option<String>,
    keys: Vec<String>,
}

impl KeyOverride {
    /// New keys for `action` everywhere it has keys.
    pub fn new<K: AsRef<str>>(action: &str, keys: impl IntoIterator<Item = K>) -> Self {
        Self {
            action: action.to_owned(),
            context: None,
            keys: keys
                .into_iter()
                .map(|key| key.as_ref().to_owned())
                .collect(),
        }
    }

    /// Only in `context`, such as `agents` or `wall`.
    #[must_use]
    pub fn in_context(mut self, context: &str) -> Self {
        self.context = Some(context.to_owned());
        self
    }
}

/// The key bindings.
#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: default_bindings(),
        }
    }
}

impl Keymap {
    /// The default bindings with some actions moved to other keys.
    ///
    /// An override without a context replaces every key of the action in
    /// every context where it has keys; with a context, only there. An empty
    /// list of keys unbinds the action.
    pub fn with_overrides(
        overrides: impl IntoIterator<Item = KeyOverride>,
    ) -> Result<Self, KeymapError> {
        let mut keymap = Self::default();
        for KeyOverride {
            action: name,
            context,
            keys,
        } in overrides
        {
            let action: Action = name.parse()?;
            let context = context.as_deref().map(str::parse::<Context>).transpose()?;
            let keys = keys
                .iter()
                .map(|key| {
                    key.parse::<KeySeq>()
                        .map_err(|source| KeymapError::InvalidKey {
                            action: name.clone(),
                            source,
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if action == Action::Leave
                && let Some(key) = keys.iter().find(|key| {
                    key.single()
                        .and_then(KeyCombo::ctrl_letter)
                        .is_none_or(|letter| !volee_host::keys::valid_leave(letter))
                })
            {
                return Err(KeymapError::LeaveKey(key.to_string()));
            }
            let mut found = false;
            for binding in &mut keymap.bindings {
                if binding.action == action && context.is_none_or(|c| c == binding.context) {
                    binding.keys.clone_from(&keys);
                    found = true;
                }
            }
            if !found && let Some(context) = context {
                return Err(KeymapError::NotInContext {
                    action: name,
                    context: context.name().to_owned(),
                });
            }
        }
        keymap.check()?;
        Ok(keymap)
    }

    /// Refuses a sequence bound to two actions, or one that starts another,
    /// where both apply.
    fn check(&self) -> Result<(), KeymapError> {
        for (index, binding) in self.bindings.iter().enumerate() {
            for other in &self.bindings[index + 1..] {
                if !binding.context.together(other.context) {
                    continue;
                }
                for key in &binding.keys {
                    for other_key in &other.keys {
                        let (a, b) = (key.keys(), other_key.keys());
                        if a == b {
                            // The panel's key wins over the same key
                            // everywhere: that is precedence, not a conflict.
                            if binding.context == other.context && binding.action != other.action {
                                return Err(KeymapError::Conflict {
                                    key: key.to_string(),
                                    context: binding.context.title(),
                                    first: binding.action.name(),
                                    second: other.action.name(),
                                });
                            }
                            continue;
                        }
                        let (short, long, first, second) = if a.len() < b.len() {
                            (key, other_key, binding, other)
                        } else {
                            (other_key, key, other, binding)
                        };
                        if long.keys().starts_with(short.keys()) {
                            return Err(KeymapError::Ambiguous {
                                short: short.to_string(),
                                first: first.action.name(),
                                long: long.to_string(),
                                second: second.action.name(),
                                context: first.context.title(),
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Every binding, in table order.
    pub(crate) fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// What `pending` followed by `key` does, looking at the contexts in
    /// order.
    pub(crate) fn resolve(
        &self,
        contexts: &[Context],
        pending: &[KeyCombo],
        key: KeyCombo,
    ) -> Resolved {
        let mut typed = pending.to_vec();
        typed.push(key);
        for context in contexts {
            if let Some(binding) = self.bindings.iter().find(|binding| {
                binding.context == *context && binding.keys.iter().any(|seq| seq.keys() == typed)
            }) {
                return Resolved::Action(binding.context, binding.action);
            }
        }
        let starts = self.bindings.iter().any(|binding| {
            contexts.contains(&binding.context)
                && binding
                    .keys
                    .iter()
                    .any(|seq| seq.keys().len() > typed.len() && seq.keys().starts_with(&typed))
        });
        if starts {
            Resolved::Prefix
        } else {
            Resolved::Unbound
        }
    }

    /// The action of a one-key sequence, looking at the contexts in order.
    pub(crate) fn action(&self, contexts: &[Context], key: KeyCombo) -> Option<Action> {
        match self.resolve(contexts, &[], key) {
            Resolved::Action(_, action) => Some(action),
            Resolved::Prefix | Resolved::Unbound => None,
        }
    }

    /// The keys that continue `pending`, one per key, the most specific
    /// context first, for the which-key popup.
    pub(crate) fn continuations(&self, contexts: &[Context], pending: &[KeyCombo]) -> Vec<Next> {
        let mut next: Vec<Next> = Vec::new();
        for context in contexts {
            for binding in self.bindings.iter().filter(|b| b.context == *context) {
                for seq in &binding.keys {
                    let keys = seq.keys();
                    if keys.len() <= pending.len() || !keys.starts_with(pending) {
                        continue;
                    }
                    let key = keys[pending.len()];
                    if next.iter().any(|entry| entry.key == key) {
                        continue;
                    }
                    let group = keys.len() > pending.len() + 1;
                    let label = if group {
                        group_name(&keys[..=pending.len()])
                    } else {
                        binding.label
                    };
                    next.push(Next { key, label, group });
                }
            }
        }
        next
    }

    /// The first keys of an action in a context, for messages such as
    /// "press `?`".
    pub(crate) fn first_key(&self, context: Context, action: Action) -> Option<&KeySeq> {
        self.bindings
            .iter()
            .find(|binding| binding.context == context && binding.action == action)
            .and_then(|binding| binding.keys.first())
    }

    /// The leave key of a taken-over agent: Control and a letter.
    pub fn leave_key(&self) -> KeyCombo {
        self.first_key(Context::Attached, Action::Leave)
            .and_then(KeySeq::single)
            .filter(|key| key.ctrl_letter().is_some())
            .unwrap_or(LEAVE)
    }

    /// The letter of the leave key, for `volee_host::attach_leaving`.
    pub fn leave_letter(&self) -> char {
        self.leave_key()
            .ctrl_letter()
            .unwrap_or(volee_host::keys::DEFAULT_LEAVE)
    }

    /// The hint bar for the active contexts: bindings with a hint, in table
    /// order, the focused panel first. Neighbours with the same hint are
    /// merged (`j/k move`), an action shown once is not repeated, and the
    /// leader key is announced when sequences start with it.
    pub(crate) fn hints(&self, contexts: &[Context]) -> Vec<Hint> {
        let mut hints: Vec<Hint> = Vec::new();
        let mut merged: Vec<Vec<String>> = Vec::new();
        let mut shown: Vec<Action> = Vec::new();
        for context in contexts {
            let mut previous: Option<&'static str> = None;
            for binding in self.bindings.iter().filter(|b| b.context == *context) {
                let (Some(text), Some(seq)) = (binding.hint, binding.keys.first()) else {
                    previous = None;
                    continue;
                };
                let reaches = seq
                    .single()
                    .is_none_or(|key| self.action(contexts, key) == Some(binding.action));
                if shown.contains(&binding.action) || !reaches {
                    continue;
                }
                shown.push(binding.action);
                match (hints.last_mut(), merged.last_mut()) {
                    (Some(_), Some(keys)) if previous == Some(text) => keys.push(seq.short()),
                    _ => {
                        hints.push(Hint {
                            keys: String::new(),
                            text,
                            action: Some(binding.action),
                        });
                        merged.push(vec![seq.short()]);
                    }
                }
                previous = Some(text);
            }
        }
        for (hint, keys) in hints.iter_mut().zip(&merged) {
            hint.keys = join_keys(keys);
        }
        let leader = KeyCombo::char(' ');
        if self.resolve(contexts, &[], leader) == Resolved::Prefix {
            let at = hints
                .iter()
                .position(|hint| hint.action == Some(Action::Help))
                .unwrap_or(hints.len());
            hints.insert(
                at,
                Hint {
                    keys: leader.short(),
                    text: "more",
                    action: None,
                },
            );
        }
        hints
    }

    /// The key tables of the user guide, one per context, in Markdown. A
    /// test keeps the guide in step with the table.
    #[cfg(test)]
    pub(crate) fn markdown(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for context in Context::ALL {
            let (title, name) = (context.title(), context.name());
            writeln!(
                out,
                "### {title} (`{name}`)\n\n| Keys | What it does | Action |\n| --- | --- | --- |"
            )
            .unwrap();
            for binding in self.bindings.iter().filter(|b| b.context == *context) {
                let keys: Vec<String> = binding.keys.iter().map(|key| format!("`{key}`")).collect();
                let (keys, action) = (keys.join(", "), binding.action.name());
                writeln!(out, "| {keys} | {} | `{action}` |", binding.description).unwrap();
            }
            out.push('\n');
        }
        out.pop();
        out
    }
}

/// The default leave key.
const LEAVE: KeyCombo = KeyCombo {
    code: KeyCode::Char('q'),
    modifiers: KeyModifiers::CONTROL,
};

/// The sequences of a default binding. Every default key parses: debug
/// builds, and so the tests, check it.
fn seqs(keys: &[&str]) -> Vec<KeySeq> {
    let parsed: Vec<KeySeq> = keys
        .iter()
        .filter_map(|key| key.parse::<KeySeq>().ok())
        .collect();
    debug_assert_eq!(parsed.len(), keys.len(), "default keys {keys:?}");
    parsed
}

// One binding per line reads best as a table; rustfmt would spread each one
// over many lines. Keys are written as in config files; a test checks that
// every one parses.
#[rustfmt::skip]
#[allow(clippy::too_many_lines)] // One table of every binding, one per line.
fn default_bindings() -> Vec<Binding> {
    use Action as A;
    use Context as C;

    let bind = |context, keys: &[&str], action, label, hint, description| Binding {
        keys: seqs(keys),
        action,
        context,
        label,
        description,
        hint,
    };

    vec![
        bind(C::Global, &["'"], A::Jump, "jump to…", Some("jump"), "jump to an agent: press ' then its letter"),
        bind(C::Global, &["-"], A::JumpBack, "jump back", None, "back to the agent selected before"),
        bind(C::Global, &["/"], A::Filter, "filter", Some("filter"), "filter agents by name, repository, branch or mission"),
        bind(C::Global, &["n"], A::NextMatch, "next match", Some("match"), "the next agent that matches the filter, round the list"),
        bind(C::Global, &["N"], A::PreviousMatch, "previous match", Some("match"), "the previous agent that matches the filter, round the list"),
        bind(C::Global, &["m"], A::Message, "message", Some("message"), "write a message to the selected agent"),
        bind(C::Global, &["d"], A::Diff, "diff", Some("diff"), "show the selected agent's changes"),
        bind(C::Global, &["a"], A::TakeOver, "take over", Some("take over"), "take over the selected agent in this terminal"),
        bind(C::Global, &["z"], A::FullScreen, "full screen", Some("full screen"), "show the selected agent's screen full size"),
        bind(C::Global, &["i"], A::Messages, "messages", Some("messages"), "your messages, questions waiting for you first"),
        bind(C::Global, &["o", "space o"], A::OpenWith, "open with…", None, "open the selected agent's worktree in your editor, diff viewer or git tool"),
        bind(C::Global, &["A", "space a"], A::Adopt, "adopt", Some("adopt"), "adopt the selected agent into a volee host, so that you can take it over"),
        bind(C::Global, &["F", "space f"], A::Forget, "forget", None, "forget the selected agent: stop listing it (asks first)"),
        bind(C::Global, &["space n"], A::NewAgent, "new agent", None, "launch an agent in a new worktree"),
        bind(C::Global, &["space N"], A::NewMission, "new mission", None, "create a mission, which launches its coordinator"),
        bind(C::Global, &["space w", "5"], A::ShowWall, "wall", None, "show the agents in volee hosts as a wall of live screens"),
        bind(C::Global, &["space t"], A::PickTheme, "theme…", None, "pick a colour theme, with a live preview; volee keeps your choice"),
        bind(C::Global, &["space k"], A::AllKeys, "all keys", None, "list every key, in every context"),
        bind(C::Global, &[":"], A::Commands, "commands", Some("commands"), "the command palette: type part of a command's name and press enter"),
        bind(C::Global, &["tab"], A::NextPane, "next panel", Some("panel"), "focus the next panel"),
        bind(C::Global, &["shift-tab"], A::PreviousPane, "previous panel", Some("panel"), "focus the previous panel"),
        bind(C::Global, &["1"], A::ShowHierarchy, "hierarchy", Some("views"), "show the hierarchy"),
        bind(C::Global, &["2"], A::ShowNetwork, "network", Some("views"), "show the network"),
        bind(C::Global, &["3"], A::ShowRepos, "repositories", Some("views"), "show repositories and worktrees"),
        bind(C::Global, &["4"], A::ShowTimeline, "timeline", Some("views"), "show the timeline"),
        bind(C::Global, &["H"], A::PreviousView, "previous view", Some("view"), "show the previous view"),
        bind(C::Global, &["L"], A::NextView, "next view", Some("view"), "show the next view"),
        bind(C::Global, &["?"], A::Help, "keys", Some("help"), "show the keys of this screen; ? again lists every key"),
        bind(C::Global, &["esc"], A::Back, "back", None, "close, cancel or go back (never quits)"),
        bind(C::Global, &["q", "ctrl-c"], A::Quit, "quit", Some("quit"), "quit volee (asks first when a draft would be lost)"),

        bind(C::Agents, &["h", "left"], A::Left, "parent", Some("move"), "parent agent"),
        bind(C::Agents, &["j", "down"], A::Down, "next", Some("move"), "next agent"),
        bind(C::Agents, &["k", "up"], A::Up, "previous", Some("move"), "previous agent"),
        bind(C::Agents, &["l", "right"], A::Right, "child", Some("move"), "first child agent"),
        bind(C::Agents, &["g g", "home"], A::First, "first", Some("first/last"), "first agent"),
        bind(C::Agents, &["G", "end"], A::Last, "last", Some("first/last"), "last agent"),
        bind(C::Agents, &["ctrl-d", "pgdn"], A::HalfPageDown, "half page down", None, "half a page of agents down"),
        bind(C::Agents, &["ctrl-u", "pgup"], A::HalfPageUp, "half page up", None, "half a page of agents up"),
        bind(C::Agents, &["enter"], A::FocusSubtree, "subtree", Some("subtree"), "show the agent's subtree in the hierarchy"),
        // The list has the keyboard at first: the zoom keys reach the view
        // beside it from there too.
        bind(C::Agents, &["+", "="], A::ZoomIn, "zoom in", None, "more detail in the view beside the list"),
        bind(C::Agents, &["_"], A::ZoomOut, "zoom out", None, "less detail in the view beside the list"),
        bind(C::Agents, &["0"], A::ZoomAuto, "auto zoom", None, "the view's detail chosen from the size again"),

        bind(C::Hierarchy, &["h", "left"], A::Left, "left", Some("move"), "card on the left"),
        bind(C::Hierarchy, &["j", "down"], A::Down, "below", Some("move"), "card below"),
        bind(C::Hierarchy, &["k", "up"], A::Up, "above", Some("move"), "card above"),
        bind(C::Hierarchy, &["l", "right"], A::Right, "right", Some("move"), "card on the right"),
        bind(C::Hierarchy, &["g g", "home"], A::First, "first", Some("first/last"), "first card"),
        bind(C::Hierarchy, &["G", "end"], A::Last, "last", Some("first/last"), "last card"),
        bind(C::Hierarchy, &["enter"], A::FocusSubtree, "subtree", Some("subtree"), "show only the selected agent and its descendants"),
        bind(C::Hierarchy, &["+", "="], A::ZoomIn, "zoom in", Some("zoom"), "more detail on cards"),
        bind(C::Hierarchy, &["_"], A::ZoomOut, "zoom out", Some("zoom"), "less detail on cards"),
        bind(C::Hierarchy, &["0"], A::ZoomAuto, "auto zoom", None, "choose the detail level from the size"),

        bind(C::Network, &["h", "left"], A::Left, "left", Some("move"), "agent on the left"),
        bind(C::Network, &["j", "down"], A::Down, "below", Some("move"), "agent below"),
        bind(C::Network, &["k", "up"], A::Up, "above", Some("move"), "agent above"),
        bind(C::Network, &["l", "right"], A::Right, "right", Some("move"), "agent on the right"),
        bind(C::Network, &["g g", "home"], A::First, "first", Some("first/last"), "first agent, top left"),
        bind(C::Network, &["G", "end"], A::Last, "last", Some("first/last"), "last agent, bottom right"),
        bind(C::Network, &["+", "="], A::ZoomIn, "zoom in", Some("zoom"), "more detail on agents"),
        bind(C::Network, &["_"], A::ZoomOut, "zoom out", Some("zoom"), "less detail on agents"),
        bind(C::Network, &["0"], A::ZoomAuto, "auto zoom", None, "choose the detail level from the size"),

        bind(C::Repos, &["j", "down"], A::Down, "next", Some("move"), "next agent"),
        bind(C::Repos, &["k", "up"], A::Up, "previous", Some("move"), "previous agent"),
        bind(C::Repos, &["h", "left"], A::Left, "previous repo", Some("repository"), "previous repository"),
        bind(C::Repos, &["l", "right"], A::Right, "next repo", Some("repository"), "next repository"),
        bind(C::Repos, &["g g", "home"], A::First, "first", Some("first/last"), "first agent"),
        bind(C::Repos, &["G", "end"], A::Last, "last", Some("first/last"), "last agent"),
        bind(C::Repos, &["+", "="], A::ZoomIn, "zoom in", Some("zoom"), "more detail per agent"),
        bind(C::Repos, &["_"], A::ZoomOut, "zoom out", Some("zoom"), "less detail per agent"),
        bind(C::Repos, &["0"], A::ZoomAuto, "auto zoom", None, "choose the detail level from the size"),

        bind(C::Timeline, &["j", "down"], A::Down, "next lane", Some("move"), "next agent"),
        bind(C::Timeline, &["k", "up"], A::Up, "previous lane", Some("move"), "previous agent"),
        bind(C::Timeline, &["h", "left"], A::Left, "back in time", Some("scrub"), "move the cursor back in time"),
        bind(C::Timeline, &["l", "right"], A::Right, "forward in time", Some("scrub"), "move the cursor forward in time"),
        bind(C::Timeline, &["["], A::PreviousEvent, "previous event", Some("event"), "cursor to the selected agent's previous event"),
        bind(C::Timeline, &["]"], A::NextEvent, "next event", Some("event"), "cursor to the selected agent's next event"),
        bind(C::Timeline, &["g g", "home"], A::First, "first", Some("first/last"), "first agent"),
        bind(C::Timeline, &["G", "end"], A::Last, "last", Some("first/last"), "last agent"),
        bind(C::Timeline, &["+", "="], A::ZoomIn, "zoom in", Some("zoom"), "shorter time span, more detail"),
        bind(C::Timeline, &["_"], A::ZoomOut, "zoom out", Some("zoom"), "longer time span"),
        bind(C::Timeline, &["0"], A::ZoomAuto, "now", None, "back to now and the default time span"),

        bind(C::Detail, &["j", "down"], A::Down, "scroll down", Some("scroll"), "scroll the preview down"),
        bind(C::Detail, &["k", "up"], A::Up, "scroll up", Some("scroll"), "scroll the preview up"),
        bind(C::Detail, &["ctrl-d", "pgdn"], A::HalfPageDown, "half page down", None, "scroll the preview half a page down"),
        bind(C::Detail, &["ctrl-u", "pgup"], A::HalfPageUp, "half page up", None, "scroll the preview half a page up"),
        bind(C::Detail, &["r"], A::Refresh, "refresh", Some("refresh"), "refresh the preview"),
        bind(C::Detail, &["enter"], A::FullScreen, "full screen", Some("full screen"), "show the agent's screen full size"),

        bind(C::Screen, &["k", "up"], A::Up, "scroll up", Some("scroll"), "scroll up, to older lines"),
        bind(C::Screen, &["j", "down"], A::Down, "scroll down", Some("scroll"), "scroll down, to newer lines"),
        bind(C::Screen, &["ctrl-u", "pgup"], A::HalfPageUp, "half page up", None, "half a page up, to older lines"),
        bind(C::Screen, &["ctrl-d", "pgdn"], A::HalfPageDown, "half page down", None, "half a page down, to newer lines"),
        bind(C::Screen, &["g g", "home"], A::First, "top", None, "the top of the screen"),
        bind(C::Screen, &["G", "end"], A::Last, "bottom", None, "the bottom, the newest lines"),
        bind(C::Screen, &["r"], A::Refresh, "refresh", Some("refresh"), "refresh the screen"),
        bind(C::Screen, &["a"], A::TakeOver, "take over", Some("take over"), "take over the agent in this terminal"),
        bind(C::Screen, &["z", "esc", "q"], A::Back, "back", Some("back"), "come back to the interface"),
        bind(C::Screen, &["ctrl-c"], A::Quit, "quit", None, "quit volee"),

        bind(C::Filter, &["enter"], A::Apply, "apply", Some("apply"), "keep the filter and close the prompt; n and N then go through the matches"),
        bind(C::Filter, &["esc"], A::Back, "cancel", Some("cancel"), "clear the filter and close the prompt"),
        bind(C::Filter, &["ctrl-c"], A::Quit, "quit", None, "quit volee"),

        bind(C::Composer, &["enter"], A::Send, "send", Some("send"), "send the message"),
        bind(C::Composer, &["alt-enter", "ctrl-j"], A::NewLine, "new line", Some("new line"), "start a new line"),
        bind(C::Composer, &["tab"], A::NextKind, "next kind", Some("kind"), "next kind of message"),
        bind(C::Composer, &["shift-tab"], A::PreviousKind, "previous kind", Some("kind"), "previous kind of message"),
        bind(C::Composer, &["esc"], A::Back, "close", Some("close"), "close the composer and keep the draft"),
        bind(C::Composer, &["ctrl-c"], A::Quit, "quit", None, "quit volee (asks first)"),

        bind(C::Diff, &["j", "down"], A::Down, "scroll down", Some("scroll"), "scroll down"),
        bind(C::Diff, &["k", "up"], A::Up, "scroll up", Some("scroll"), "scroll up"),
        bind(C::Diff, &["ctrl-d", "pgdn"], A::HalfPageDown, "half page down", None, "half a page down"),
        bind(C::Diff, &["ctrl-u", "pgup"], A::HalfPageUp, "half page up", None, "half a page up"),
        bind(C::Diff, &["g g", "home"], A::First, "top", None, "top"),
        bind(C::Diff, &["G", "end"], A::Last, "bottom", None, "bottom"),
        bind(C::Diff, &["f"], A::ToggleFull, "summary/patch", Some("summary/patch"), "switch between the summary and the full patch"),
        bind(C::Diff, &["r"], A::Refresh, "refresh", Some("refresh"), "load the changes again"),
        bind(C::Diff, &["o"], A::OpenWith, "open with…", Some("open with"), "open the worktree in your editor, diff viewer or git tool"),
        bind(C::Diff, &["esc", "d", "q"], A::Back, "close", Some("close"), "close the changes"),
        bind(C::Diff, &["ctrl-c"], A::Quit, "quit", None, "quit volee"),

        bind(C::Confirm, &["y"], A::Confirm, "yes", Some("yes"), "yes"),
        bind(C::Confirm, &["n", "esc"], A::Back, "no", Some("no"), "no"),
        bind(C::Confirm, &["ctrl-c"], A::Quit, "quit", None, "quit volee without asking again"),

        bind(C::Inbox, &["j", "down"], A::Down, "next", Some("move"), "next row, or further down the thread you read"),
        bind(C::Inbox, &["k", "up"], A::Up, "previous", Some("move"), "previous row, or further up the thread you read"),
        bind(C::Inbox, &["ctrl-d", "pgdn"], A::HalfPageDown, "half page down", None, "half a page down"),
        bind(C::Inbox, &["ctrl-u", "pgup"], A::HalfPageUp, "half page up", None, "half a page up"),
        bind(C::Inbox, &["g g", "home"], A::First, "first", None, "first row, or the start of the thread"),
        bind(C::Inbox, &["G", "end"], A::Last, "last", None, "last row, or the end of the thread"),
        bind(C::Inbox, &["enter"], A::Apply, "read", Some("read"), "read the thread, which marks its messages read"),
        bind(C::Inbox, &["r"], A::Reply, "reply", Some("reply"), "reply: the answer to the question, else a note to the agent"),
        bind(C::Inbox, &["s"], A::Summarize, "summarize", Some("summarize"), "summarize the thread with a model, when summaries are on"),
        bind(C::Inbox, &["esc", "q", "i"], A::Back, "close", Some("close"), "leave the thread, then close the messages"),
        bind(C::Inbox, &["ctrl-c"], A::Quit, "quit", None, "quit volee"),

        bind(C::Tools, &["e"], A::OpenEditor, "editor", Some("editor"), "your editor ($VISUAL, $EDITOR or [tui.tools] editor)"),
        bind(C::Tools, &["d"], A::OpenDiff, "diff viewer", Some("diff"), "your diff viewer (Neovim with diffview, or [tui.tools] diff)"),
        bind(C::Tools, &["g"], A::OpenGit, "git tool", Some("git"), "your git tool (lazygit, or [tui.tools] git)"),
        bind(C::Tools, &["w"], A::OpenIde, "window editor", Some("window"), "an editor with its own window (VS Code, or [tui.tools] ide)"),
        bind(C::Tools, &["esc", "q", "o"], A::Back, "close", Some("close"), "close the menu"),
        bind(C::Tools, &["ctrl-c"], A::Quit, "quit", None, "quit volee"),

        // Enter never launches: a launch costs money, and the task and the
        // goal take several lines.
        bind(C::Dialog, &["ctrl-s"], A::Launch, "launch", Some("launch"), "launch the agent, or create the mission"),
        bind(C::Dialog, &["tab"], A::NextField, "next field", Some("field"), "next field"),
        bind(C::Dialog, &["shift-tab"], A::PreviousField, "previous field", Some("field"), "previous field"),
        bind(C::Dialog, &["enter", "alt-enter", "ctrl-j"], A::NewLine, "new line", Some("new line"), "a new line in the task or the goal, the next field elsewhere"),
        bind(C::Dialog, &["esc"], A::Back, "cancel", Some("cancel"), "cancel"),
        bind(C::Dialog, &["ctrl-c"], A::Quit, "quit", None, "quit volee"),

        bind(C::Choice, &["left", "h"], A::PreviousChoice, "previous", None, "the previous option, or the cursor to the previous repository"),
        bind(C::Choice, &["right", "l"], A::NextChoice, "next", None, "the next option, or the cursor to the next repository"),
        bind(C::Choice, &["space"], A::ToggleChoice, "pick", None, "pick or unpick the repository under the cursor; the next option of a single choice"),

        bind(C::Editing, &["ctrl-w", "ctrl-backspace"], A::DeleteWord, "delete word", None, "delete the word before the cursor"),
        bind(C::Editing, &["ctrl-u"], A::DeleteToLineStart, "delete to start", None, "delete from the start of the line to the cursor"),
        bind(C::Editing, &["ctrl-a", "home"], A::LineStart, "line start", None, "the cursor to the start of the line"),
        bind(C::Editing, &["ctrl-e", "end"], A::LineEnd, "line end", None, "the cursor to the end of the line"),

        bind(C::Keys, &["?"], A::AllKeys, "all keys", Some("all keys"), "list every key, in every context"),
        bind(C::Keys, &["esc", "q"], A::Back, "close", Some("close"), "close the popup; any other key closes it and acts"),

        bind(C::Help, &["j", "down"], A::Down, "scroll down", Some("scroll"), "scroll down"),
        bind(C::Help, &["k", "up"], A::Up, "scroll up", Some("scroll"), "scroll up"),
        bind(C::Help, &["ctrl-d", "pgdn"], A::HalfPageDown, "half page down", None, "half a page down"),
        bind(C::Help, &["ctrl-u", "pgup"], A::HalfPageUp, "half page up", None, "half a page up"),
        bind(C::Help, &["g g", "home"], A::First, "top", None, "top"),
        bind(C::Help, &["G", "end"], A::Last, "bottom", None, "bottom"),
        bind(C::Help, &["esc", "?", "q"], A::Back, "close", Some("close"), "close the list"),
        bind(C::Help, &["ctrl-c"], A::Quit, "quit", None, "quit volee"),

        bind(C::Commands, &["enter"], A::Apply, "run", Some("run"), "run the selected command"),
        bind(C::Commands, &["down", "ctrl-n", "tab"], A::Down, "next", Some("move"), "the next command"),
        bind(C::Commands, &["up", "ctrl-p", "shift-tab"], A::Up, "previous", Some("move"), "the previous command"),
        bind(C::Commands, &["esc"], A::Back, "close", Some("close"), "close the palette"),
        bind(C::Commands, &["ctrl-c"], A::Quit, "quit", None, "quit volee"),

        bind(C::Themes, &["j", "down"], A::Down, "next", Some("move"), "the next theme, shown at once"),
        bind(C::Themes, &["k", "up"], A::Up, "previous", Some("move"), "the previous theme, shown at once"),
        bind(C::Themes, &["enter"], A::Apply, "keep", Some("keep"), "keep the theme and save it in the configuration"),
        bind(C::Themes, &["esc", "q"], A::Back, "cancel", Some("cancel"), "back to the theme you had"),
        bind(C::Themes, &["ctrl-c"], A::Quit, "quit", None, "quit volee"),

        bind(C::Wall, &["h", "left"], A::Left, "left", Some("move"), "the screen on the left"),
        bind(C::Wall, &["j", "down"], A::Down, "below", Some("move"), "the screen below"),
        bind(C::Wall, &["k", "up"], A::Up, "above", Some("move"), "the screen above"),
        bind(C::Wall, &["l", "right"], A::Right, "right", Some("move"), "the screen on the right"),
        bind(C::Wall, &["z"], A::FullScreen, "zoom", Some("zoom"), "the selected screen alone, full size, and back"),
        bind(C::Wall, &["a", "enter"], A::TakeOver, "take over", Some("take over"), "take over the selected agent; the leave key comes back to the wall"),
        bind(C::Wall, &["q", "esc"], A::Back, "back", Some("back"), "leave the wall"),
        bind(C::Wall, &["ctrl-c"], A::Quit, "quit", None, "leave the wall"),

        bind(C::Attached, &["ctrl-q"], A::Leave, "leave", None, "leave the agent you took over and come back; Control and a letter only"),
    ]
}

#[cfg(test)]
mod tests;
