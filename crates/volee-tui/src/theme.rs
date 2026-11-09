//! Themes: the colors of the interface (ADR 0009).
//!
//! Widgets never name a color. They ask the [`Theme`] for the style of a
//! [`StyleRole`], such as `state.working` or `diff.added`. A theme is a TOML
//! file that maps roles to colors through a palette, in the style of Helix:
//!
//! ```toml
//! inherits = "catppuccin-mocha"   # optional: start from another theme
//!
//! [palette]                       # named colors, used by the roles
//! peach = "#fab387"
//!
//! [roles]
//! "state.needs_input" = { fg = "peach", modifiers = ["bold"] }
//! "role.worker" = "blue"          # a plain string sets the foreground
//! ```
//!
//! A color is a palette name, an ANSI color name (`red`, `light-blue`,
//! `dark-gray`...), `reset` for the terminal's default, a `#rrggbb` value or a
//! 256-color index. A role that a theme leaves out falls back to its parent
//! role (`state.working` falls back to `state`), then to the terminal's
//! default style.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use volee_core::AgentState;
use volee_core::role::Role;

macro_rules! style_roles {
    ($($(#[doc = $doc:literal])+ $variant:ident => $key:literal,)+) => {
        /// A semantic role that widgets style themselves with.
        ///
        /// Theme files name roles by their [`key`](Self::key), for example
        /// `state.needs_input`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum StyleRole {
            $($(#[doc = $doc])+ $variant,)+
        }

        impl StyleRole {
            /// Every role, in the order of the documentation.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The name of the role in theme files.
            pub const fn key(self) -> &'static str {
                match self {
                    $(Self::$variant => $key,)+
                }
            }

            /// What the role is used for, in one sentence.
            pub fn description(self) -> &'static str {
                match self {
                    $(Self::$variant => concat!($($doc),+).trim_ascii(),)+
                }
            }
        }
    };
}

style_roles! {
    /// The background and default text color of the whole screen.
    Background => "background",
    /// Regular text.
    Text => "text",
    /// Secondary text: labels, separators, elapsed times, paths.
    TextDim => "text.dim",
    /// Panel titles.
    Title => "title",
    /// Borders of panels.
    Border => "border",
    /// The border and the title of the focused panel.
    BorderFocused => "border.focused",
    /// The selected row of a list.
    Selection => "selection",
    /// The border of the selected card in the hierarchy.
    CardSelected => "card.selected",
    /// The top bar.
    Topbar => "topbar",
    /// The volee badge at the left of the top bar.
    TopbarBrand => "topbar.brand",
    /// Tabs that are not shown.
    Tab => "tab",
    /// The tab on screen.
    TabActive => "tab.active",
    /// The number in front of each tab.
    TabNumber => "tab.number",
    /// Counters in the top bar.
    Counter => "counter",
    /// The count of agents that need you, when it is not zero.
    CounterUrgent => "counter.urgent",
    /// The letter that jumps to an agent.
    Letter => "letter",
    /// An agent volee launched but has not seen running yet.
    StateLaunching => "state.launching",
    /// An agent running a turn.
    StateWorking => "state.working",
    /// An agent that waits for a person: permission, question or dialog.
    StateNeedsInput => "state.needs_input",
    /// An agent waiting for its next instruction.
    StateIdle => "state.idle",
    /// An agent that finished its last task.
    StateDone => "state.done",
    /// An agent that ended with an error.
    StateFailed => "state.failed",
    /// An agent whose process stopped; its conversation can be resumed.
    StateStopped => "state.stopped",
    /// An agent volee cannot find any more.
    StateLost => "state.lost",
    /// The lead agent you talk to.
    RoleLead => "role.lead",
    /// The coordinator of a mission.
    RoleCoordinator => "role.coordinator",
    /// A worker in one repository.
    RoleWorker => "role.worker",
    /// A helper started by another agent inside its session.
    RoleSubagent => "role.subagent",
    /// A session volee did not launch.
    RoleIndependent => "role.independent",
    /// Lines between cards, and who launched whom in the network view.
    Edge => "edge",
    /// A message moving along an edge of the network view.
    EdgePulse => "edge.pulse",
    /// Agents that exchanged messages, in the network view.
    EdgeMessage => "edge.message",
    /// A question that waits for its answer, in the network view.
    EdgeQuestion => "edge.question",
    /// Added lines.
    DiffAdded => "diff.added",
    /// Removed lines.
    DiffRemoved => "diff.removed",
    /// Pull request numbers.
    BadgePr => "badge.pr",
    /// Checks that passed.
    ChecksPassing => "checks.passing",
    /// Checks that failed.
    ChecksFailing => "checks.failing",
    /// Checks that still run.
    ChecksPending => "checks.pending",
    /// Activity sparklines on cards.
    Sparkline => "sparkline",
    /// Keys in the hint bar and in the help.
    HintKey => "hint.key",
    /// Descriptions in the hint bar and in the help.
    HintText => "hint.text",
    /// Short notices at the bottom of the screen.
    Notice => "notice",
    /// Notices about something that failed.
    NoticeError => "notice.error",
    /// Text being typed, such as a filter.
    Input => "input",
    /// Words that stand in for missing content, such as an empty view.
    Placeholder => "placeholder",
}

/// The themes compiled into volee, by name.
const BUILTIN: [(&str, &str); 5] = [
    (
        "catppuccin-mocha",
        include_str!("../themes/catppuccin-mocha.toml"),
    ),
    (
        "catppuccin-macchiato",
        include_str!("../themes/catppuccin-macchiato.toml"),
    ),
    (
        "catppuccin-frappe",
        include_str!("../themes/catppuccin-frappe.toml"),
    ),
    (
        "catppuccin-latte",
        include_str!("../themes/catppuccin-latte.toml"),
    ),
    ("terminal", include_str!("../themes/terminal.toml")),
];

/// How many themes an `inherits` chain may hold.
const MAX_CHAIN: usize = 8;

/// Finds the text of a theme by name, or `None`. The flag asks for the
/// built-in theme only, for themes that extend the built-in of their name.
type Fetch<'a> = dyn FnMut(&str, bool) -> Result<Option<String>, ThemeError> + 'a;

/// Why a theme could not be loaded.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ThemeError {
    /// No built-in theme and no file has this name.
    #[error("no theme named `{name}` (built-in themes: {available})")]
    Unknown {
        /// The name asked for.
        name: String,
        /// The built-in names, for the message.
        available: String,
    },
    /// The name could escape the themes directory.
    #[error("`{0}` is not a valid theme name (use letters, digits, `-`, `_` and `.`)")]
    InvalidName(String),
    /// The file is not a valid theme.
    #[error("theme `{theme}` is not valid: {message}")]
    Parse {
        /// The theme.
        theme: String,
        /// What the parser said.
        message: String,
    },
    /// A value is not a color.
    #[error("theme `{theme}`: `{value}` in `{key}` is not a color or a palette name")]
    Color {
        /// The theme.
        theme: String,
        /// The palette entry or role.
        key: String,
        /// The value written.
        value: String,
    },
    /// A modifier is not known.
    #[error("theme `{theme}`: `{value}` in `{key}` is not a modifier")]
    Modifier {
        /// The theme.
        theme: String,
        /// The role.
        key: String,
        /// The value written.
        value: String,
    },
    /// Themes inherit from each other in a loop, or the chain is too long.
    #[error("themes inherit from each other in a loop or too deeply: {chain}")]
    Inheritance {
        /// The chain, from the theme asked for to its ancestors.
        chain: String,
    },
    /// The theme file exists but cannot be read.
    #[error("cannot read the theme file {path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The error.
        #[source]
        source: std::io::Error,
    },
}

/// How many colors the terminal can show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24-bit colors.
    #[default]
    TrueColor,
    /// The xterm 256-color palette: RGB colors are replaced by the nearest
    /// palette entry.
    Ansi256,
}

impl ColorDepth {
    /// Reads `COLORTERM`: terminals that support 24-bit colors set it to
    /// `truecolor` or `24bit`.
    pub fn detect() -> Self {
        Self::from_colorterm(std::env::var("COLORTERM").ok().as_deref())
    }

    /// The depth announced by a `COLORTERM` value.
    pub fn from_colorterm(value: Option<&str>) -> Self {
        match value.map(str::to_ascii_lowercase).as_deref() {
            Some("truecolor" | "24bit") => Self::TrueColor,
            _ => Self::Ansi256,
        }
    }

    /// The color to send to a terminal of this depth.
    pub fn adapt(self, color: Color) -> Color {
        match (self, color) {
            (Self::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(nearest_256(r, g, b)),
            _ => color,
        }
    }
}

/// The xterm 256-color index closest to an RGB color, among the 6x6x6 cube
/// and the gray ramp. The first 16 entries are skipped on purpose: terminals
/// let users redefine them.
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |value: u8| -> u8 {
        let mut best = 0;
        for (index, candidate) in (0u8..).zip(LEVELS) {
            if value.abs_diff(candidate) < value.abs_diff(LEVELS[usize::from(best)]) {
                best = index;
            }
        }
        best
    };
    let distance = |(r2, g2, b2): (u8, u8, u8)| -> u32 {
        let d = |a: u8, b: u8| u32::from(a.abs_diff(b)).pow(2);
        d(r, r2) + d(g, g2) + d(b, b2)
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (
        LEVELS[usize::from(ri)],
        LEVELS[usize::from(gi)],
        LEVELS[usize::from(bi)],
    );
    let cube_index = 16 + 36 * ri + 6 * gi + bi;
    let average = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    // The ramp holds 24 grays: 8, 18, ..., 238.
    let step = u8::try_from((average.saturating_sub(3) / 10).min(23)).unwrap_or(23);
    let gray = 8 + 10 * step;
    if distance((gray, gray, gray)) < distance(cube) {
        232 + step
    } else {
        cube_index
    }
}

/// A role's value in a theme file: a foreground color, or a full style.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum RoleValue {
    Foreground(String),
    Style(StyleSpec),
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StyleSpec {
    fg: Option<String>,
    bg: Option<String>,
    #[serde(default)]
    modifiers: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    inherits: Option<String>,
    #[serde(default)]
    palette: BTreeMap<String, String>,
    #[serde(default)]
    roles: BTreeMap<String, RoleValue>,
}

/// A resolved theme: one style per [`StyleRole`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    name: String,
    depth: ColorDepth,
    styles: Vec<Style>,
}

impl Default for Theme {
    fn default() -> Self {
        // The default theme is compiled in and parsed by a unit test, so it
        // always resolves.
        Self::builtin(Self::DEFAULT).unwrap_or_else(|_| Self::plain())
    }
}

impl Theme {
    /// The name of the default theme.
    pub const DEFAULT: &'static str = "catppuccin-mocha";

    /// The names of the themes compiled into volee.
    pub fn builtin_names() -> impl Iterator<Item = &'static str> {
        BUILTIN.iter().map(|(name, _)| *name)
    }

    /// Every theme a person can pick: the built-in ones in their stable
    /// order, then the `*.toml` files of `user_dir` (names without the
    /// extension, sorted, without the names already built in). A missing or
    /// unreadable directory adds nothing.
    pub fn available_names(user_dir: Option<&Path>) -> Vec<String> {
        let mut names: Vec<String> = Self::builtin_names().map(str::to_owned).collect();
        let Some(entries) = user_dir.and_then(|dir| std::fs::read_dir(dir).ok()) else {
            return names;
        };
        let user: BTreeSet<String> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
            .filter(|name| !names.contains(name) && validate_name(name).is_ok())
            .collect();
        names.extend(user);
        names
    }

    /// A theme compiled into volee.
    pub fn builtin(name: &str) -> Result<Self, ThemeError> {
        Self::resolve(name, &mut |wanted, _| {
            Ok(builtin_text(wanted).map(str::to_owned))
        })
    }

    /// Loads a theme by name: `<user_dir>/<name>.toml` when it exists,
    /// otherwise the built-in theme of that name.
    ///
    /// A user theme may inherit from a built-in theme of the same name, to
    /// change a few roles of it.
    pub fn load(name: &str, user_dir: Option<&Path>) -> Result<Self, ThemeError> {
        Self::resolve(name, &mut |wanted, builtin_only| {
            if !builtin_only && let Some(dir) = user_dir {
                let path = dir.join(format!("{wanted}.toml"));
                match std::fs::read_to_string(&path) {
                    Ok(text) => return Ok(Some(text)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => return Err(ThemeError::Io { path, source }),
                }
            }
            Ok(builtin_text(wanted).map(str::to_owned))
        })
    }

    /// A theme from TOML text. `inherits` may name a built-in theme.
    pub fn from_toml(name: &str, text: &str) -> Result<Self, ThemeError> {
        Self::resolve(name, &mut |wanted, builtin_only| {
            Ok(if wanted == name && !builtin_only {
                Some(text.to_owned())
            } else {
                builtin_text(wanted).map(str::to_owned)
            })
        })
    }

    /// The same theme for a terminal that shows `depth` colors.
    #[must_use]
    pub fn with_color_depth(mut self, depth: ColorDepth) -> Self {
        self.depth = depth;
        for style in &mut self.styles {
            style.fg = style.fg.map(|color| depth.adapt(color));
            style.bg = style.bg.map(|color| depth.adapt(color));
        }
        self
    }

    /// The theme's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The color depth the theme was adapted to.
    pub fn color_depth(&self) -> ColorDepth {
        self.depth
    }

    /// The style of a role.
    pub fn style(&self, role: StyleRole) -> Style {
        self.styles.get(role as usize).copied().unwrap_or_default()
    }

    /// The style that shows an agent's state.
    pub fn state(&self, state: &AgentState) -> Style {
        self.style(match state {
            AgentState::Launching => StyleRole::StateLaunching,
            AgentState::Working => StyleRole::StateWorking,
            AgentState::NeedsInput { .. } => StyleRole::StateNeedsInput,
            AgentState::Idle => StyleRole::StateIdle,
            AgentState::Done => StyleRole::StateDone,
            AgentState::Failed => StyleRole::StateFailed,
            AgentState::Stopped => StyleRole::StateStopped,
            AgentState::Lost => StyleRole::StateLost,
        })
    }

    /// The style that shows an agent's role.
    pub fn role(&self, role: Role) -> Style {
        self.style(match role {
            Role::Lead => StyleRole::RoleLead,
            Role::Coordinator => StyleRole::RoleCoordinator,
            Role::Worker => StyleRole::RoleWorker,
            Role::Subagent => StyleRole::RoleSubagent,
            Role::Independent => StyleRole::RoleIndependent,
        })
    }

    /// A theme with no colors at all, used only if the default theme could
    /// not be built.
    fn plain() -> Self {
        Self {
            name: "plain".to_owned(),
            depth: ColorDepth::TrueColor,
            styles: vec![Style::default(); StyleRole::ALL.len()],
        }
    }

    /// Loads `name` and its ancestors through `fetch`, then resolves every
    /// role. `fetch(name, builtin_only)` returns the text of a theme, or
    /// `None` when there is none.
    fn resolve(name: &str, fetch: &mut Fetch<'_>) -> Result<Self, ThemeError> {
        let mut chain: Vec<(String, ThemeFile)> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut next = Some((name.to_owned(), false));
        while let Some((current, builtin_only)) = next.take() {
            validate_name(&current)?;
            if chain.len() >= MAX_CHAIN || !seen.insert((current.clone(), builtin_only)) {
                let mut names: Vec<&str> = chain.iter().map(|(n, _)| n.as_str()).collect();
                names.push(&current);
                return Err(ThemeError::Inheritance {
                    chain: names.join(" > "),
                });
            }
            let text = fetch(&current, builtin_only)?.ok_or_else(|| ThemeError::Unknown {
                name: current.clone(),
                available: Self::builtin_names().collect::<Vec<_>>().join(", "),
            })?;
            let file: ThemeFile = toml::from_str(&text).map_err(|error| ThemeError::Parse {
                theme: current.clone(),
                message: error.message().to_owned(),
            })?;
            // A theme that inherits from its own name extends the built-in one.
            next = file.inherits.clone().map(|parent| {
                let builtin_only = parent == current;
                (parent, builtin_only)
            });
            chain.push((current, file));
        }

        let mut palette: BTreeMap<String, String> = BTreeMap::new();
        let mut roles: BTreeMap<String, RoleValue> = BTreeMap::new();
        for (_, file) in chain.iter().rev() {
            palette.extend(file.palette.iter().map(|(k, v)| (k.clone(), v.clone())));
            roles.extend(file.roles.iter().map(|(k, v)| (k.clone(), v.clone())));
        }

        let resolver = Resolver {
            theme: name,
            palette: palette
                .iter()
                .map(|(key, value)| {
                    Color::from_str(value)
                        .map(|color| (key.as_str(), color))
                        .map_err(|_| ThemeError::Color {
                            theme: name.to_owned(),
                            key: format!("palette.{key}"),
                            value: value.clone(),
                        })
                })
                .collect::<Result<_, _>>()?,
        };
        let known: BTreeSet<&str> = StyleRole::ALL.iter().map(|role| role.key()).collect();
        for key in roles.keys() {
            let is_parent = known
                .iter()
                .any(|role| role.starts_with(key.as_str()) && role[key.len()..].starts_with('.'));
            if !known.contains(key.as_str()) && !is_parent {
                tracing::warn!(
                    theme = name,
                    role = key.as_str(),
                    "unknown theme role ignored"
                );
            }
        }
        let styles = StyleRole::ALL
            .iter()
            .map(|role| match lookup(&roles, role.key()) {
                Some((key, value)) => resolver.style(key, value),
                None => Ok(Style::default()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            name: name.to_owned(),
            depth: ColorDepth::TrueColor,
            styles,
        })
    }
}

impl fmt::Display for StyleRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

fn builtin_text(name: &str) -> Option<&'static str> {
    BUILTIN
        .iter()
        .find(|(builtin, _)| *builtin == name)
        .map(|(_, text)| *text)
}

fn validate_name(name: &str) -> Result<(), ThemeError> {
    let valid = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(ThemeError::InvalidName(name.to_owned()))
    }
}

/// The value of a role, or of its closest parent role.
fn lookup<'a>(
    roles: &'a BTreeMap<String, RoleValue>,
    key: &'a str,
) -> Option<(&'a str, &'a RoleValue)> {
    let mut key = key;
    loop {
        if let Some(value) = roles.get(key) {
            return Some((key, value));
        }
        key = &key[..key.rfind('.')?];
    }
}

struct Resolver<'a> {
    theme: &'a str,
    palette: BTreeMap<&'a str, Color>,
}

impl Resolver<'_> {
    fn color(&self, key: &str, value: &str) -> Result<Color, ThemeError> {
        if let Some(color) = self.palette.get(value) {
            return Ok(*color);
        }
        Color::from_str(value).map_err(|_| ThemeError::Color {
            theme: self.theme.to_owned(),
            key: key.to_owned(),
            value: value.to_owned(),
        })
    }

    fn style(&self, key: &str, value: &RoleValue) -> Result<Style, ThemeError> {
        let spec = match value {
            RoleValue::Foreground(fg) => StyleSpec {
                fg: Some(fg.clone()),
                ..StyleSpec::default()
            },
            RoleValue::Style(spec) => spec.clone(),
        };
        let mut style = Style::default();
        if let Some(fg) = &spec.fg {
            style = style.fg(self.color(key, fg)?);
        }
        if let Some(bg) = &spec.bg {
            style = style.bg(self.color(key, bg)?);
        }
        for modifier in &spec.modifiers {
            style = style.add_modifier(parse_modifier(modifier).ok_or_else(|| {
                ThemeError::Modifier {
                    theme: self.theme.to_owned(),
                    key: key.to_owned(),
                    value: modifier.clone(),
                }
            })?);
        }
        Ok(style)
    }
}

fn parse_modifier(name: &str) -> Option<Modifier> {
    Some(match name {
        "bold" => Modifier::BOLD,
        "dim" => Modifier::DIM,
        "italic" => Modifier::ITALIC,
        "underlined" => Modifier::UNDERLINED,
        "reversed" => Modifier::REVERSED,
        "crossed-out" => Modifier::CROSSED_OUT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_theme_resolves_and_defines_every_role() {
        for name in Theme::builtin_names() {
            let theme = Theme::builtin(name).unwrap();
            assert_eq!(theme.name(), name);
            let file: ThemeFile = toml::from_str(builtin_text(name).unwrap()).unwrap();
            let mut roles: BTreeMap<String, RoleValue> = BTreeMap::new();
            if let Some(parent) = &file.inherits {
                let parent: ThemeFile = toml::from_str(builtin_text(parent).unwrap()).unwrap();
                roles.extend(parent.roles);
            }
            roles.extend(file.roles);
            for role in StyleRole::ALL {
                assert!(roles.contains_key(role.key()), "{name} misses {role}");
            }
        }
    }

    #[test]
    fn the_default_theme_is_catppuccin_mocha() {
        let theme = Theme::default();
        assert_eq!(theme.name(), "catppuccin-mocha");
        assert_eq!(
            theme.style(StyleRole::Background).bg,
            Some(Color::Rgb(0x1e, 0x1e, 0x2e))
        );
    }

    #[test]
    fn role_keys_are_unique_and_documented() {
        let keys: BTreeSet<&str> = StyleRole::ALL.iter().map(|role| role.key()).collect();
        assert_eq!(keys.len(), StyleRole::ALL.len());
        for role in StyleRole::ALL {
            assert!(role.description().ends_with('.'), "{role}");
            assert!(!role.description().starts_with(' '), "{role}");
        }
    }

    #[test]
    fn roles_resolve_through_the_palette() {
        let theme = Theme::from_toml(
            "mine",
            r##"
            [palette]
            accent = "#112233"
            [roles]
            text = "accent"
            "diff.added" = { fg = "green", bg = "#000000", modifiers = ["bold", "italic"] }
            "##,
        )
        .unwrap();
        assert_eq!(
            theme.style(StyleRole::Text).fg,
            Some(Color::Rgb(0x11, 0x22, 0x33))
        );
        let added = theme.style(StyleRole::DiffAdded);
        assert_eq!(added.fg, Some(Color::Green));
        assert_eq!(added.bg, Some(Color::Rgb(0, 0, 0)));
        assert!(
            added
                .add_modifier
                .contains(Modifier::BOLD | Modifier::ITALIC)
        );
    }

    #[test]
    fn a_missing_role_falls_back_to_its_parent_then_to_the_default() {
        let theme = Theme::from_toml("mine", "[roles]\nstate = \"red\"\n").unwrap();
        assert_eq!(theme.style(StyleRole::StateWorking).fg, Some(Color::Red));
        assert_eq!(theme.style(StyleRole::StateLost).fg, Some(Color::Red));
        assert_eq!(theme.style(StyleRole::Edge), Style::default());
    }

    #[test]
    fn a_child_palette_recolors_the_inherited_roles() {
        let theme = Theme::from_toml(
            "mine",
            r##"
            inherits = "catppuccin-mocha"
            [palette]
            yellow = "#ffff00"
            [roles]
            edge = "blue"
            "##,
        )
        .unwrap();
        let mocha = Theme::builtin("catppuccin-mocha").unwrap();
        assert_eq!(
            theme.style(StyleRole::StateNeedsInput).fg,
            Some(Color::Rgb(255, 255, 0))
        );
        assert_eq!(
            theme.style(StyleRole::Edge),
            mocha.style(StyleRole::RoleWorker)
        );
        assert_eq!(theme.style(StyleRole::Text), mocha.style(StyleRole::Text));
    }

    #[test]
    fn flavors_share_the_roles_of_mocha() {
        let latte = Theme::builtin("catppuccin-latte").unwrap();
        assert_eq!(
            latte.style(StyleRole::Background).bg,
            Some(Color::Rgb(0xef, 0xf1, 0xf5))
        );
        assert_eq!(
            latte.style(StyleRole::RoleCoordinator).fg,
            Some(Color::Rgb(0xfe, 0x64, 0x0b))
        );
    }

    #[test]
    fn available_names_list_builtins_then_sorted_user_themes() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["zen.toml", "dusk.toml", "terminal.toml", "notes.txt"] {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        std::fs::create_dir(dir.path().join("folder.toml.d")).unwrap();
        let names = Theme::available_names(Some(dir.path()));
        let builtin: Vec<&str> = Theme::builtin_names().collect();
        assert_eq!(names[..builtin.len()], builtin[..]);
        assert_eq!(names[builtin.len()..], ["dusk", "zen"]);
    }

    #[test]
    fn available_names_without_a_directory_are_the_builtins() {
        let builtin: Vec<&str> = Theme::builtin_names().collect();
        assert_eq!(Theme::available_names(None), builtin);
        let missing = Path::new("/nonexistent/volee/themes");
        assert_eq!(Theme::available_names(Some(missing)), builtin);
    }

    #[test]
    fn a_user_theme_can_extend_the_builtin_of_the_same_name() {
        let dir = std::env::temp_dir().join(format!("volee-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("terminal.toml"),
            "inherits = \"terminal\"\n[roles]\nedge = \"red\"\n",
        )
        .unwrap();
        let theme = Theme::load("terminal", Some(&dir)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(theme.style(StyleRole::Edge).fg, Some(Color::Red));
        assert_eq!(theme.style(StyleRole::StateWorking).fg, Some(Color::Cyan));
    }

    #[test]
    fn loading_falls_back_to_builtins_without_a_user_file() {
        let dir = std::env::temp_dir().join("volee-theme-test-missing");
        let theme = Theme::load("terminal", Some(&dir)).unwrap();
        assert_eq!(theme, Theme::builtin("terminal").unwrap());
        assert!(matches!(
            Theme::load("nope", None),
            Err(ThemeError::Unknown { .. })
        ));
    }

    #[test]
    fn inheritance_loops_are_errors() {
        let error = Theme::resolve("a", &mut |name, _| {
            Ok(Some(match name {
                "a" => "inherits = \"b\"".to_owned(),
                _ => "inherits = \"a\"".to_owned(),
            }))
        })
        .unwrap_err();
        assert!(matches!(error, ThemeError::Inheritance { ref chain } if chain == "a > b > a"));
    }

    #[test]
    fn bad_values_are_reported_with_their_place() {
        let error = Theme::from_toml("mine", "[roles]\ntext = \"chartreuse-ish\"\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "theme `mine`: `chartreuse-ish` in `text` is not a color or a palette name"
        );
        let error = Theme::from_toml(
            "mine",
            "[roles]\ntext = { fg = \"red\", modifiers = [\"blink\"] }\n",
        )
        .unwrap_err();
        assert!(matches!(error, ThemeError::Modifier { .. }));
        let error = Theme::from_toml("mine", "[palette]\nx = \"nope\"\n").unwrap_err();
        assert!(matches!(error, ThemeError::Color { ref key, .. } if key == "palette.x"));
        let error = Theme::from_toml("mine", "colours = 1\n").unwrap_err();
        assert!(matches!(error, ThemeError::Parse { .. }));
    }

    #[test]
    fn theme_names_cannot_escape_the_directory() {
        for name in ["../secrets", "a/b", ".hidden", ""] {
            assert!(matches!(
                Theme::load(name, None),
                Err(ThemeError::InvalidName(_))
            ));
        }
    }

    #[test]
    fn colorterm_decides_the_depth() {
        assert_eq!(
            ColorDepth::from_colorterm(Some("truecolor")),
            ColorDepth::TrueColor
        );
        assert_eq!(
            ColorDepth::from_colorterm(Some("24BIT")),
            ColorDepth::TrueColor
        );
        assert_eq!(
            ColorDepth::from_colorterm(Some("8bit")),
            ColorDepth::Ansi256
        );
        assert_eq!(ColorDepth::from_colorterm(None), ColorDepth::Ansi256);
    }

    #[test]
    fn rgb_colors_map_to_the_nearest_xterm_color() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(255, 0, 0), 196);
        assert_eq!(nearest_256(0x80, 0x80, 0x80), 244);
        assert_eq!(nearest_256(0x1e, 0x1e, 0x2e), 235);
        assert_eq!(nearest_256(0x87, 0xaf, 0xff), 111);
        let theme = Theme::default().with_color_depth(ColorDepth::Ansi256);
        assert!(StyleRole::ALL.iter().all(|role| {
            let style = theme.style(*role);
            !matches!(style.fg, Some(Color::Rgb(..))) && !matches!(style.bg, Some(Color::Rgb(..)))
        }));
        assert_eq!(theme.color_depth(), ColorDepth::Ansi256);
    }
}
