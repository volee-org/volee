//! `volee` without a command: the interface on every agent of this machine.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use volee_config::Config;
use volee_tui::{ColorDepth, DaemonSource, Keymap, Options, Theme, ThemeError};

use crate::background;
use crate::connect::{connect_or_start, paths};
use crate::tools;

/// Starts the daemon when it does not run, then shows its agents until the
/// person quits. The theme, the key overrides and reduced motion come from
/// the configuration.
pub(crate) async fn run(theme: Option<&str>) -> Result<()> {
    let options = options(theme)?;
    let client = connect_or_start().await?;
    let source = DaemonSource::connect(client)
        .await
        .context("cannot follow the volee daemon")?;
    volee_tui::run(source, options).await?;
    Ok(())
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
    let theme = Theme::load(name, Some(&themes_dir))
        .map_err(|error| unknown_theme(error, &themes_dir))?
        .with_color_depth(ColorDepth::detect());
    let overrides = config
        .tui
        .keys
        .iter()
        .map(|(action, key)| (action.as_str(), vec![key.as_str()]));
    let keymap = Keymap::with_overrides(overrides)
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
    options.keymap = keymap;
    options.tools =
        tools::resolve(&config.tui.tools, &machine).with_context(|| file.display().to_string())?;
    options.repos = config.repos.keys().cloned().collect();
    options.reduced_motion = config.tui.reduced_motion;
    Ok(options)
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
