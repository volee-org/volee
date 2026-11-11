//! `volee` without a command: the interface on every agent of this machine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use volee_config::{Config, KeySetting};
use volee_tui::keymap::KeyOverride;
use volee_tui::{ColorDepth, DaemonSource, Keymap, Options, Theme, ThemeError, ThemeSaver};

use crate::background;
use crate::connect::{connect_or_start, paths};
use crate::tools;

/// Starts the daemon when it does not run, then shows its agents until the
/// person quits. The theme, the key overrides and reduced motion come from
/// the configuration.
pub(crate) async fn run(theme: Option<&str>) -> Result<()> {
    init_logging(&paths()?.log_dir());
    let mut options = options(theme)?;
    // Agents in volee hosts are shown live and typed into in edit mode.
    options.hosts = Some(paths()?.state_dir.join("hosts"));
    // The theme picked in the interface is kept in the configuration file;
    // the demo leaves it alone.
    let file = paths()?.config_file();
    options.theme_saver = Some(ThemeSaver::new(move |name| {
        volee_config::set_theme(&file, name)
            .map(|()| format!("kept in {}", file.display()))
            .map_err(|error| error.to_string())
    }));
    let client = connect_or_start().await?;
    let source = DaemonSource::connect(client)
        .await
        .context("cannot follow the volee daemon")?;
    volee_tui::run(source, options).await?;
    Ok(())
}

/// With `VOLEE_LOG` set (a filter, such as `volee_tui=debug`), the interface
/// writes its diagnostics to `interface.log`, beside the daemon's log,
/// readable only by the person. The terminal is the interface's: nothing
/// may be printed there. Without the variable nothing is written.
fn init_logging(log_dir: &Path) {
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(filter) = tracing_subscriber::EnvFilter::try_from_env("VOLEE_LOG") else {
        return;
    };
    let opened = std::fs::create_dir_all(log_dir).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(log_dir.join("interface.log"))
    });
    let Ok(file) = opened else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init();
}

/// The interface's colors, keys, motion, repositories and tools from the
/// configuration file and the machine; `theme` (the `--theme` flag) replaces
/// the configured theme. With neither, the theme follows the terminal's
/// background, light or dark, and is the default when that stays unknown.
pub(crate) fn options(theme: Option<&str>) -> Result<Options> {
    let paths = paths()?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let file = paths.config_file();
    let config = Config::load(&file, &home)?;
    let themes_dir = paths.config_dir.join("themes");
    let name = theme
        .or(config.tui.theme.as_deref())
        .or_else(|| background::guess_theme())
        .unwrap_or(Theme::DEFAULT);
    let depth = ColorDepth::detect();
    let theme = Theme::load(name, Some(&themes_dir))
        .map_err(|error| unknown_theme(error, &themes_dir))?
        .with_color_depth(depth);
    // Every theme for the picker; one that does not load is left out.
    let themes = Theme::available_names(Some(&themes_dir))
        .into_iter()
        .filter_map(|name| {
            Theme::load(&name, Some(&themes_dir))
                .ok()
                .map(|theme| (name, theme.with_color_depth(depth)))
        })
        .collect();
    let keymap = Keymap::with_overrides(key_overrides(&config.tui.keys))
        .with_context(|| format!("{}: [tui.keys]", file.display()))?;
    let var = |name: &str| std::env::var(name).ok();
    let path = std::env::var("PATH").ok();
    let has = |program: &str| tools::on_path(program, path.as_deref());
    let diffview = tools::data_home(var).is_some_and(|data| tools::diffview_installed(&data));
    let machine = tools::Machine {
        var: &var,
        has: &has,
        diffview,
    };
    let mut options = Options::default();
    options.theme = theme;
    name.clone_into(&mut options.theme_name);
    options.themes = themes;
    options.keymap = keymap;
    options.tools =
        tools::resolve(&config.tui.tools, &machine).with_context(|| file.display().to_string())?;
    options.repos = config.repos.keys().cloned().collect();
    options.reduced_motion = config.tui.reduced_motion;
    Ok(options)
}

/// The keys of the configuration, for commands outside the interface (the
/// wall, taking over from a shell). A configuration that cannot be read
/// leaves the default keys, with a warning.
pub(crate) fn keymap_or_default() -> Keymap {
    let configured = || -> Result<Keymap> {
        let paths = paths()?;
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let file = paths.config_file();
        let config = Config::load(&file, &home)?;
        Keymap::with_overrides(key_overrides(&config.tui.keys))
            .with_context(|| format!("{}: [tui.keys]", file.display()))
    };
    configured().unwrap_or_else(|error| {
        eprintln!("volee: default keys, the configured ones cannot be read: {error:#}");
        Keymap::default()
    })
}

/// How the leave key reads in a sentence: `Ctrl+Q`.
pub(crate) fn leave_key_name(keymap: &Keymap) -> String {
    format!("Ctrl+{}", keymap.leave_letter().to_ascii_uppercase())
}

/// The `[tui.keys]` table as keymap overrides: `action = "key"` or a list
/// of keys (empty unbinds) for every context, and `[tui.keys.<context>]`
/// tables for one.
pub(crate) fn key_overrides(keys: &BTreeMap<String, KeySetting>) -> Vec<KeyOverride> {
    let mut overrides = Vec::new();
    for (name, setting) in keys {
        match setting {
            KeySetting::Keys(list) => overrides.push(KeyOverride::new(name, list.keys())),
            KeySetting::Context(actions) => {
                for (action, list) in actions {
                    overrides.push(KeyOverride::new(action, list.keys()).in_context(name));
                }
            }
        }
    }
    overrides
}

/// A theme error; for an unknown name, it lists every theme that can be
/// picked, the user's own included.
fn unknown_theme(error: ThemeError, themes_dir: &Path) -> anyhow::Error {
    if matches!(error, ThemeError::Unknown { .. }) {
        let names = Theme::available_names(Some(themes_dir)).join(", ");
        anyhow::Error::new(error).context(format!("available themes: {names}"))
    } else {
        error.into()
    }
}
