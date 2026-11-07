//! Key bindings (ADR 0009).
//!
//! Every binding is declared once, in one table, with its context, its
//! action and its description. Key dispatch, the help overlay, the hint bar
//! and the key tables of the user guide are all generated from that table,
//! so they cannot drift apart.
//!
//! Users rebind an action by giving it new keys, written like `q`, `ctrl-c`,
//! `shift-tab`, `enter` or `f5`:
//!
//! ```
//! use volee_tui::Keymap;
//!
//! let keymap = Keymap::with_overrides([("take-over", vec!["ctrl-a"])])?;
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
    const fn char(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
        }
    }

    /// A character with Control.
    const fn ctrl(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
        }
    }

    /// A named key without modifiers.
    const fn key(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::NONE,
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

    /// Whether Control is held.
    pub(crate) fn ctrl_held(self) -> bool {
        self.modifiers.contains(KeyModifiers::CONTROL)
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
#[error("`{0}` is not a key (examples: `q`, `ctrl-c`, `shift-tab`, `enter`, `f5`)")]
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
    /// Show or hide the help.
    Help => "help",
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
    /// Jump to an agent by its letter.
    Jump => "jump",
    /// Go back to the previously selected agent.
    JumpBack => "jump-back",
    /// Filter agents.
    Filter => "filter",
    /// Take over the selected agent in this terminal.
    TakeOver => "take-over",
    /// Write a message to the selected agent.
    Message => "message",
    /// Show the selected agent's changes.
    Diff => "diff",
    /// Forget the selected agent: stop listing it, after asking.
    Forget => "forget",
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
    /// Apply what was typed.
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

/// Where a binding applies. Bindings of the focused panel take precedence
/// over bindings that apply everywhere; the help and the filter prompt
/// capture every key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Context {
    /// Everywhere, unless the help or a prompt is open.
    Global,
    /// The list of agents.
    Agents,
    /// The hierarchy view.
    Hierarchy,
    /// The network view.
    Network,
    /// The repositories view.
    Repos,
    /// The timeline view.
    Timeline,
    /// The detail panel and its preview.
    Detail,
    /// The filter prompt.
    Filter,
    /// The message composer.
    Composer,
    /// The changes window.
    Diff,
    /// A yes or no question on the bottom line.
    Confirm,
    /// The messages window.
    Inbox,
    /// The menu of programs to open a worktree with.
    Tools,
    /// A dialog that launches an agent or a mission.
    Dialog,
    /// The help overlay.
    Help,
}

impl Context {
    /// Contexts in the order of the help and of the user guide.
    pub(crate) const ALL: [Self; 15] = [
        Self::Global,
        Self::Agents,
        Self::Hierarchy,
        Self::Network,
        Self::Repos,
        Self::Timeline,
        Self::Detail,
        Self::Filter,
        Self::Composer,
        Self::Diff,
        Self::Confirm,
        Self::Inbox,
        Self::Tools,
        Self::Dialog,
        Self::Help,
    ];

    /// The title of the context in the help.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Global => "Everywhere",
            Self::Agents => "Agents list",
            Self::Hierarchy => "Hierarchy",
            Self::Network => "Network",
            Self::Repos => "Repositories",
            Self::Timeline => "Timeline",
            Self::Detail => "Detail and preview",
            Self::Filter => "Filter prompt",
            Self::Composer => "Message composer",
            Self::Diff => "Changes",
            Self::Confirm => "Questions",
            Self::Inbox => "Messages",
            Self::Tools => "Open with",
            Self::Dialog => "Launch dialogs",
            Self::Help => "Help",
        }
    }
}

/// One line of the binding table.
#[derive(Clone, Debug)]
pub(crate) struct Binding {
    /// The keys, the first one is shown in hints.
    pub(crate) keys: Vec<KeyCombo>,
    /// What they do.
    pub(crate) action: Action,
    /// Where.
    pub(crate) context: Context,
    /// A sentence for the help.
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
    /// The action of the first key.
    pub(crate) action: Action,
}

/// Why a keymap override was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum KeymapError {
    /// The action does not exist.
    #[error("unknown action `{0}` in the key bindings")]
    UnknownAction(String),
    /// A key could not be read.
    #[error("action `{action}`: {source}")]
    InvalidKey {
        /// The action.
        action: String,
        /// The error.
        #[source]
        source: ParseKeyError,
    },
    /// An action was given no key; actions can be moved, not removed.
    #[error("action `{0}` needs at least one key")]
    NoKeys(String),
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
    /// Each override replaces every key of the action, in every context.
    pub fn with_overrides<I, A, K>(overrides: I) -> Result<Self, KeymapError>
    where
        I: IntoIterator<Item = (A, Vec<K>)>,
        A: AsRef<str>,
        K: AsRef<str>,
    {
        let mut keymap = Self::default();
        for (name, keys) in overrides {
            let name = name.as_ref();
            let action: Action = name.parse()?;
            if keys.is_empty() {
                return Err(KeymapError::NoKeys(name.to_owned()));
            }
            let keys = keys
                .iter()
                .map(|key| {
                    key.as_ref()
                        .parse::<KeyCombo>()
                        .map_err(|source| KeymapError::InvalidKey {
                            action: name.to_owned(),
                            source,
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            for binding in &mut keymap.bindings {
                if binding.action == action {
                    binding.keys.clone_from(&keys);
                }
            }
        }
        keymap.check()?;
        Ok(keymap)
    }

    /// Refuses a key bound to two actions in the same context.
    fn check(&self) -> Result<(), KeymapError> {
        for (index, binding) in self.bindings.iter().enumerate() {
            for other in &self.bindings[index + 1..] {
                if other.context != binding.context || other.action == binding.action {
                    continue;
                }
                if let Some(key) = binding.keys.iter().find(|key| other.keys.contains(key)) {
                    return Err(KeymapError::Conflict {
                        key: key.to_string(),
                        context: binding.context.title(),
                        first: binding.action.name(),
                        second: other.action.name(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Every binding, in table order.
    pub(crate) fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// The action of a key and the context that bound it, looking at the
    /// contexts in order.
    pub(crate) fn lookup(&self, contexts: &[Context], key: KeyCombo) -> Option<(Context, Action)> {
        contexts.iter().find_map(|context| {
            self.bindings
                .iter()
                .find(|binding| binding.context == *context && binding.keys.contains(&key))
                .map(|binding| (binding.context, binding.action))
        })
    }

    /// The action of a key, looking at the contexts in order.
    pub(crate) fn action(&self, contexts: &[Context], key: KeyCombo) -> Option<Action> {
        self.lookup(contexts, key).map(|(_, action)| action)
    }

    /// The keys of an action in a context, for messages such as "press `?`".
    pub(crate) fn first_key(&self, context: Context, action: Action) -> Option<KeyCombo> {
        self.bindings
            .iter()
            .find(|binding| binding.context == context && binding.action == action)
            .and_then(|binding| binding.keys.first().copied())
    }

    /// The hint bar for the active contexts: bindings with a hint, in table
    /// order, the focused panel first. Neighbours with the same hint are
    /// merged (`j/k move`), and an action shown once is not repeated.
    pub(crate) fn hints(&self, contexts: &[Context]) -> Vec<Hint> {
        let mut hints: Vec<Hint> = Vec::new();
        let mut shown: Vec<Action> = Vec::new();
        for context in contexts {
            let mut previous: Option<&'static str> = None;
            for binding in self.bindings.iter().filter(|b| b.context == *context) {
                let (Some(text), Some(key)) = (binding.hint, binding.keys.first()) else {
                    previous = None;
                    continue;
                };
                if shown.contains(&binding.action)
                    || self.action(contexts, *key) != Some(binding.action)
                {
                    continue;
                }
                shown.push(binding.action);
                match hints.last_mut() {
                    Some(last) if previous == Some(text) => {
                        last.keys.push('/');
                        last.keys.push_str(&key.to_string());
                    }
                    _ => hints.push(Hint {
                        keys: key.to_string(),
                        text,
                        action: binding.action,
                    }),
                }
                previous = Some(text);
            }
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
            let title = context.title();
            writeln!(
                out,
                "### {title}\n\n| Keys | What it does | Action |\n| --- | --- | --- |"
            )
            .unwrap();
            for binding in self.bindings.iter().filter(|b| b.context == context) {
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

// One binding per line reads best as a table; rustfmt would spread each one
// over seven lines.
#[rustfmt::skip]
#[allow(clippy::too_many_lines)] // One table of every binding, one per line.
fn default_bindings() -> Vec<Binding> {
    use Action as A;
    use Context as C;
    use KeyCode as K;

    let bind = |context, keys: &[KeyCombo], action, description, hint| Binding {
        keys: keys.to_vec(),
        action,
        context,
        description,
        hint,
    };
    let ch = KeyCombo::char;
    let ctrl = KeyCombo::ctrl;
    let key = KeyCombo::key;
    let alt_enter = KeyCombo {
        code: K::Enter,
        modifiers: KeyModifiers::ALT,
    };

    vec![
        bind(C::Global, &[ch('\'')], A::Jump, "jump to an agent: press ' then its letter", Some("jump")),
        bind(C::Global, &[ch('-')], A::JumpBack, "back to the agent selected before", None),
        bind(C::Global, &[ch('/')], A::Filter, "filter agents by name, repository, branch or mission", Some("filter")),
        bind(C::Global, &[ch('m')], A::Message, "write a message to the selected agent", Some("message")),
        bind(C::Global, &[ch('d')], A::Diff, "show the selected agent's changes", Some("diff")),
        bind(C::Global, &[ch('a')], A::TakeOver, "take over the selected agent in this terminal", Some("take over")),
        bind(C::Global, &[ch('i')], A::Messages, "your messages, questions waiting for you first", Some("messages")),
        bind(C::Global, &[ch('o')], A::OpenWith, "open the selected agent's worktree in your editor, diff viewer or git tool", None),
        bind(C::Global, &[ch('n')], A::NewAgent, "launch an agent in a new worktree", None),
        bind(C::Global, &[ch('N')], A::NewMission, "create a mission, which launches its coordinator", None),
        bind(C::Global, &[ch('F')], A::Forget, "forget the selected agent: stop listing it (asks first)", None),
        bind(C::Global, &[key(K::Tab)], A::NextPane, "focus the next panel", None),
        bind(C::Global, &[key(K::BackTab)], A::PreviousPane, "focus the previous panel", None),
        bind(C::Global, &[ch('1')], A::ShowHierarchy, "show the hierarchy", None),
        bind(C::Global, &[ch('2')], A::ShowNetwork, "show the network", None),
        bind(C::Global, &[ch('3')], A::ShowRepos, "show repositories and worktrees", None),
        bind(C::Global, &[ch('4')], A::ShowTimeline, "show the timeline", None),
        bind(C::Global, &[ch('?')], A::Help, "show or hide the keys", Some("help")),
        bind(C::Global, &[key(K::Esc)], A::Back, "close, cancel or go back (never quits)", None),
        bind(C::Global, &[ch('q'), ctrl('c')], A::Quit, "quit volee (asks first when a draft would be lost)", Some("quit")),

        bind(C::Agents, &[ch('h'), key(K::Left)], A::Left, "parent agent", Some("move")),
        bind(C::Agents, &[ch('j'), key(K::Down)], A::Down, "next agent", Some("move")),
        bind(C::Agents, &[ch('k'), key(K::Up)], A::Up, "previous agent", Some("move")),
        bind(C::Agents, &[ch('l'), key(K::Right)], A::Right, "first child agent", Some("move")),
        bind(C::Agents, &[ch('g'), key(K::Home)], A::First, "first agent", None),
        bind(C::Agents, &[ch('G'), key(K::End)], A::Last, "last agent", None),
        bind(C::Agents, &[key(K::Enter)], A::FocusSubtree, "show the agent's subtree in the hierarchy", Some("subtree")),
        // The list has the keyboard at first: the zoom keys reach the view
        // beside it from there too.
        bind(C::Agents, &[ch('+'), ch('=')], A::ZoomIn, "more detail in the view beside the list", None),
        bind(C::Agents, &[ch('_')], A::ZoomOut, "less detail in the view beside the list", None),
        bind(C::Agents, &[ch('0')], A::ZoomAuto, "the view's detail chosen from the size again", None),

        bind(C::Hierarchy, &[ch('h'), key(K::Left)], A::Left, "card on the left", Some("move")),
        bind(C::Hierarchy, &[ch('j'), key(K::Down)], A::Down, "card below", Some("move")),
        bind(C::Hierarchy, &[ch('k'), key(K::Up)], A::Up, "card above", Some("move")),
        bind(C::Hierarchy, &[ch('l'), key(K::Right)], A::Right, "card on the right", Some("move")),
        bind(C::Hierarchy, &[ch('g'), key(K::Home)], A::First, "first card", None),
        bind(C::Hierarchy, &[ch('G'), key(K::End)], A::Last, "last card", None),
        bind(C::Hierarchy, &[key(K::Enter)], A::FocusSubtree, "show only the selected agent and its descendants", Some("subtree")),
        bind(C::Hierarchy, &[ch('+'), ch('=')], A::ZoomIn, "more detail on cards", Some("zoom")),
        bind(C::Hierarchy, &[ch('_')], A::ZoomOut, "less detail on cards", Some("zoom")),
        bind(C::Hierarchy, &[ch('0')], A::ZoomAuto, "choose the detail level from the size", None),

        bind(C::Network, &[ch('h'), key(K::Left)], A::Left, "agent on the left", Some("move")),
        bind(C::Network, &[ch('j'), key(K::Down)], A::Down, "agent below", Some("move")),
        bind(C::Network, &[ch('k'), key(K::Up)], A::Up, "agent above", Some("move")),
        bind(C::Network, &[ch('l'), key(K::Right)], A::Right, "agent on the right", Some("move")),
        bind(C::Network, &[ch('g'), key(K::Home)], A::First, "first agent, top left", None),
        bind(C::Network, &[ch('G'), key(K::End)], A::Last, "last agent, bottom right", None),
        bind(C::Network, &[ch('+'), ch('=')], A::ZoomIn, "more detail on agents", Some("zoom")),
        bind(C::Network, &[ch('_')], A::ZoomOut, "less detail on agents", Some("zoom")),
        bind(C::Network, &[ch('0')], A::ZoomAuto, "choose the detail level from the size", None),

        bind(C::Repos, &[ch('j'), key(K::Down)], A::Down, "next agent", Some("move")),
        bind(C::Repos, &[ch('k'), key(K::Up)], A::Up, "previous agent", Some("move")),
        bind(C::Repos, &[ch('h'), key(K::Left)], A::Left, "previous repository", Some("repository")),
        bind(C::Repos, &[ch('l'), key(K::Right)], A::Right, "next repository", Some("repository")),
        bind(C::Repos, &[ch('g'), key(K::Home)], A::First, "first agent", None),
        bind(C::Repos, &[ch('G'), key(K::End)], A::Last, "last agent", None),
        bind(C::Repos, &[ch('+'), ch('=')], A::ZoomIn, "more detail per agent", Some("zoom")),
        bind(C::Repos, &[ch('_')], A::ZoomOut, "less detail per agent", Some("zoom")),
        bind(C::Repos, &[ch('0')], A::ZoomAuto, "choose the detail level from the size", None),

        bind(C::Timeline, &[ch('j'), key(K::Down)], A::Down, "next agent", Some("move")),
        bind(C::Timeline, &[ch('k'), key(K::Up)], A::Up, "previous agent", Some("move")),
        bind(C::Timeline, &[ch('h'), key(K::Left)], A::Left, "move the cursor back in time", Some("scrub")),
        bind(C::Timeline, &[ch('l'), key(K::Right)], A::Right, "move the cursor forward in time", Some("scrub")),
        bind(C::Timeline, &[ch('[')], A::PreviousEvent, "cursor to the selected agent's previous event", Some("event")),
        bind(C::Timeline, &[ch(']')], A::NextEvent, "cursor to the selected agent's next event", Some("event")),
        bind(C::Timeline, &[ch('g'), key(K::Home)], A::First, "first agent", None),
        bind(C::Timeline, &[ch('G'), key(K::End)], A::Last, "last agent", None),
        bind(C::Timeline, &[ch('+'), ch('=')], A::ZoomIn, "shorter time span, more detail", Some("zoom")),
        bind(C::Timeline, &[ch('_')], A::ZoomOut, "longer time span", Some("zoom")),
        bind(C::Timeline, &[ch('0')], A::ZoomAuto, "back to now and the default time span", None),

        bind(C::Detail, &[ch('j'), key(K::Down)], A::Down, "scroll the preview down", Some("scroll")),
        bind(C::Detail, &[ch('k'), key(K::Up)], A::Up, "scroll the preview up", Some("scroll")),
        bind(C::Detail, &[ch('r')], A::Refresh, "refresh the preview", Some("refresh")),

        bind(C::Filter, &[key(K::Enter)], A::Apply, "keep the filter and close the prompt", Some("apply")),
        bind(C::Filter, &[key(K::Esc)], A::Back, "clear the filter and close the prompt", Some("cancel")),
        bind(C::Filter, &[ctrl('c')], A::Quit, "quit volee", None),

        bind(C::Composer, &[key(K::Enter)], A::Send, "send the message", Some("send")),
        bind(C::Composer, &[alt_enter, ctrl('j')], A::NewLine, "start a new line", Some("new line")),
        bind(C::Composer, &[key(K::Tab)], A::NextKind, "next kind of message", Some("kind")),
        bind(C::Composer, &[key(K::BackTab)], A::PreviousKind, "previous kind of message", Some("kind")),
        bind(C::Composer, &[key(K::Esc)], A::Back, "close the composer and keep the draft", Some("close")),
        bind(C::Composer, &[ctrl('c')], A::Quit, "quit volee (asks first)", None),

        bind(C::Diff, &[ch('j'), key(K::Down)], A::Down, "scroll down", Some("scroll")),
        bind(C::Diff, &[ch('k'), key(K::Up)], A::Up, "scroll up", Some("scroll")),
        bind(C::Diff, &[ch('g'), key(K::Home)], A::First, "top", None),
        bind(C::Diff, &[ch('G'), key(K::End)], A::Last, "bottom", None),
        bind(C::Diff, &[ch('f')], A::ToggleFull, "switch between the summary and the full patch", Some("summary/patch")),
        bind(C::Diff, &[ch('r')], A::Refresh, "load the changes again", Some("refresh")),
        bind(C::Diff, &[ch('o')], A::OpenWith, "open the worktree in your editor, diff viewer or git tool", Some("open with")),
        bind(C::Diff, &[key(K::Esc), ch('d'), ch('q')], A::Back, "close the changes", Some("close")),
        bind(C::Diff, &[ctrl('c')], A::Quit, "quit volee", None),

        bind(C::Confirm, &[ch('y')], A::Confirm, "yes", Some("yes")),
        bind(C::Confirm, &[ch('n'), key(K::Esc)], A::Back, "no", Some("no")),
        bind(C::Confirm, &[ctrl('c')], A::Quit, "quit volee without asking again", None),

        bind(C::Inbox, &[ch('j'), key(K::Down)], A::Down, "next row, or further down the thread you read", Some("move")),
        bind(C::Inbox, &[ch('k'), key(K::Up)], A::Up, "previous row, or further up the thread you read", Some("move")),
        bind(C::Inbox, &[ch('g'), key(K::Home)], A::First, "first row, or the start of the thread", None),
        bind(C::Inbox, &[ch('G'), key(K::End)], A::Last, "last row, or the end of the thread", None),
        bind(C::Inbox, &[key(K::Enter)], A::Apply, "read the thread, which marks its messages read", Some("read")),
        bind(C::Inbox, &[ch('r')], A::Reply, "reply: the answer to the question, else a note to the agent", Some("reply")),
        bind(C::Inbox, &[ch('s')], A::Summarize, "summarize the thread with a model, when summaries are on", Some("summarize")),
        bind(C::Inbox, &[key(K::Esc), ch('q'), ch('i')], A::Back, "leave the thread, then close the messages", Some("close")),
        bind(C::Inbox, &[ctrl('c')], A::Quit, "quit volee", None),

        bind(C::Tools, &[ch('e')], A::OpenEditor, "your editor ($VISUAL, $EDITOR or [tui.tools] editor)", Some("editor")),
        bind(C::Tools, &[ch('d')], A::OpenDiff, "your diff viewer (Neovim with diffview, or [tui.tools] diff)", Some("diff")),
        bind(C::Tools, &[ch('g')], A::OpenGit, "your git tool (lazygit, or [tui.tools] git)", Some("git")),
        bind(C::Tools, &[ch('w')], A::OpenIde, "an editor with its own window (VS Code, or [tui.tools] ide)", Some("window")),
        bind(C::Tools, &[key(K::Esc), ch('q'), ch('o')], A::Back, "close the menu", Some("close")),
        bind(C::Tools, &[ctrl('c')], A::Quit, "quit volee", None),

        // Enter never launches: a launch costs money, and the task and the
        // goal take several lines.
        bind(C::Dialog, &[ctrl('s')], A::Launch, "launch the agent, or create the mission", Some("launch")),
        bind(C::Dialog, &[key(K::Tab)], A::NextField, "next field", Some("field")),
        bind(C::Dialog, &[key(K::BackTab)], A::PreviousField, "previous field", Some("field")),
        bind(C::Dialog, &[key(K::Enter), alt_enter, ctrl('j')], A::NewLine, "a new line in the task or the goal, the next field elsewhere", Some("new line")),
        bind(C::Dialog, &[key(K::Esc)], A::Back, "cancel", Some("cancel")),
        bind(C::Dialog, &[ctrl('c')], A::Quit, "quit volee", None),

        bind(C::Help, &[ch('j'), key(K::Down)], A::Down, "scroll down", Some("scroll")),
        bind(C::Help, &[ch('k'), key(K::Up)], A::Up, "scroll up", Some("scroll")),
        bind(C::Help, &[key(K::Esc), ch('?'), ch('q')], A::Back, "close the help", Some("close")),
        bind(C::Help, &[ctrl('c')], A::Quit, "quit volee", None),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventKind;

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new_with_kind(code, modifiers, KeyEventKind::Press)
    }

    #[test]
    fn keys_parse_and_display_round_trip() {
        for text in [
            "q",
            "G",
            "?",
            "-",
            "+",
            "'",
            "ctrl-c",
            "alt-x",
            "shift-tab",
            "enter",
            "esc",
            "tab",
            "space",
            "f5",
            "↑",
            "home",
            "pgdn",
            "ctrl--",
        ] {
            let combo: KeyCombo = text.parse().unwrap();
            let shown = combo.to_string();
            assert_eq!(
                shown.parse::<KeyCombo>().unwrap(),
                combo,
                "{text} -> {shown}"
            );
        }
        assert_eq!("up".parse::<KeyCombo>().unwrap().to_string(), "↑");
        assert_eq!("Ctrl-C".parse::<KeyCombo>().unwrap().to_string(), "ctrl-C");
        assert_eq!("shift-g".parse::<KeyCombo>().unwrap(), KeyCombo::char('G'));
        assert_eq!(
            "backtab".parse::<KeyCombo>().unwrap().to_string(),
            "shift-tab"
        );
    }

    #[test]
    fn bad_keys_are_refused() {
        for text in ["", "ctrl-", "f99", "hyper-x", "enterr"] {
            assert!(text.parse::<KeyCombo>().is_err(), "{text}");
        }
    }

    #[test]
    fn events_are_normalized_like_written_keys() {
        assert_eq!(
            KeyCombo::from_event(&event(KeyCode::Char('?'), KeyModifiers::SHIFT)),
            KeyCombo::char('?')
        );
        assert_eq!(
            KeyCombo::from_event(&event(KeyCode::BackTab, KeyModifiers::SHIFT)),
            "shift-tab".parse().unwrap()
        );
        assert_eq!(
            KeyCombo::from_event(&event(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            "ctrl-c".parse().unwrap()
        );
        assert_eq!(
            KeyCombo::from_event(&event(KeyCode::Char('x'), KeyModifiers::SUPER)),
            KeyCombo::char('x')
        );
    }

    #[test]
    fn the_default_table_has_no_conflicts_and_every_action() {
        let keymap = Keymap::default();
        keymap.check().unwrap();
        for action in Action::ALL {
            assert!(
                keymap
                    .bindings()
                    .iter()
                    .any(|binding| binding.action == *action),
                "{action:?} has no key"
            );
            assert_eq!(action.name().parse::<Action>().unwrap(), *action);
        }
    }

    #[test]
    fn the_focused_panel_takes_precedence_over_global_keys() {
        let keymap = Keymap::default();
        let contexts = [Context::Help];
        assert_eq!(
            keymap.action(&contexts, KeyCombo::char('q')),
            Some(Action::Back)
        );
        let contexts = [Context::Agents, Context::Global];
        assert_eq!(
            keymap.action(&contexts, KeyCombo::char('q')),
            Some(Action::Quit)
        );
        assert_eq!(
            keymap.action(&contexts, KeyCombo::char('j')),
            Some(Action::Down)
        );
        // The zoom keys reach the view beside the list.
        assert_eq!(
            keymap.action(&contexts, KeyCombo::char('+')),
            Some(Action::ZoomIn)
        );
        let contexts = [Context::Hierarchy, Context::Global];
        assert_eq!(
            keymap.action(&contexts, KeyCombo::char('=')),
            Some(Action::ZoomIn)
        );
        assert_eq!(
            keymap.action(&contexts, KeyCombo::char('-')),
            Some(Action::JumpBack)
        );
    }

    #[test]
    fn overrides_move_every_binding_of_an_action() {
        let keymap =
            Keymap::with_overrides([("down", vec!["n"]), ("take-over", vec!["ctrl-a"])]).unwrap();
        let contexts = [Context::Hierarchy, Context::Global];
        assert_eq!(
            keymap.action(&contexts, KeyCombo::char('n')),
            Some(Action::Down)
        );
        assert_eq!(keymap.action(&contexts, KeyCombo::char('j')), None);
        assert_eq!(
            keymap.action(&contexts, KeyCombo::ctrl('a')),
            Some(Action::TakeOver)
        );
        assert_eq!(keymap.action(&contexts, KeyCombo::char('a')), None);
    }

    #[test]
    fn overrides_are_checked() {
        assert_eq!(
            Keymap::with_overrides([("teleport", vec!["t"])]).unwrap_err(),
            KeymapError::UnknownAction("teleport".into())
        );
        assert!(matches!(
            Keymap::with_overrides([("quit", vec!["ctrl-"])]).unwrap_err(),
            KeymapError::InvalidKey { .. }
        ));
        assert_eq!(
            Keymap::with_overrides([("quit", Vec::<String>::new())]).unwrap_err(),
            KeymapError::NoKeys("quit".into())
        );
        let error = Keymap::with_overrides([("filter", vec!["a"])]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "`a` is bound to both `filter` and `take-over` (Everywhere)"
        );
    }

    #[test]
    fn hints_follow_the_focused_panel() {
        let keymap = Keymap::default();
        let line = |contexts: &[Context]| -> String {
            keymap
                .hints(contexts)
                .iter()
                .map(|hint| format!("{} {}", hint.keys, hint.text))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        assert_eq!(
            line(&[Context::Hierarchy, Context::Global]),
            "h/j/k/l move · enter subtree · +/_ zoom · ' jump · / filter · m message · d diff · \
             a take over · i messages · ? help · q quit"
        );
        assert_eq!(line(&[Context::Help]), "j/k scroll · esc close");
        assert_eq!(line(&[Context::Filter]), "enter apply · esc cancel");
        assert_eq!(
            line(&[Context::Composer]),
            "enter send · alt-enter new line · tab/shift-tab kind · esc close"
        );
        assert_eq!(
            line(&[Context::Diff]),
            "j/k scroll · f summary/patch · r refresh · o open with · esc close"
        );
        assert_eq!(line(&[Context::Confirm]), "y yes · n no");
    }

    #[test]
    fn the_markdown_tables_list_every_binding() {
        let markdown = Keymap::default().markdown();
        assert!(markdown.starts_with(
            "### Everywhere\n\n| Keys | What it does | Action |\n| --- | --- | --- |\n"
        ));
        assert!(markdown.contains(
            "| `q`, `ctrl-c` | quit volee (asks first when a draft would be lost) | `quit` |"
        ));
        assert!(markdown.contains("| `alt-enter`, `ctrl-j` | start a new line | `new-line` |"));
        assert!(
            markdown
                .contains("| `a` | take over the selected agent in this terminal | `take-over` |")
        );
        assert!(markdown.contains("### Hierarchy"));
        assert_eq!(
            markdown.matches("\n| `").count(),
            Keymap::default().bindings().len()
        );
    }
}
