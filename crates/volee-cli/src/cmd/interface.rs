//! `volee` without a command: the interface on every agent of this machine.

use std::path::PathBuf;

use anyhow::{Context, Result};
use volee_config::Config;
use volee_tui::{ColorDepth, DaemonSource, Keymap, Options, Theme};

use crate::connect::{connect_or_start, paths};

/// Starts the daemon when it does not run, then shows its agents until the
/// person quits. The theme and the key overrides come from the
/// configuration.
pub(crate) async fn run() -> Result<()> {
    let options = options(None)?;
    let client = connect_or_start().await?;
    let source = DaemonSource::connect(client)
        .await
        .context("cannot follow the volee daemon")?;
    volee_tui::run(source, options).await?;
    Ok(())
}

/// The interface's colors and keys from the configuration file; `theme`
/// replaces the configured theme.
pub(crate) fn options(theme: Option<&str>) -> Result<Options> {
    let paths = paths()?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let file = paths.config_file();
    let config = Config::load(&file, &home)?;
    let name = theme
        .or(config.tui.theme.as_deref())
        .unwrap_or(Theme::DEFAULT);
    let theme = Theme::load(name, Some(&paths.config_dir.join("themes")))?
        .with_color_depth(ColorDepth::detect());
    let overrides = config
        .tui
        .keys
        .iter()
        .map(|(action, key)| (action.as_str(), vec![key.as_str()]));
    let keymap = Keymap::with_overrides(overrides)
        .with_context(|| format!("{}: [tui.keys]", file.display()))?;
    let mut options = Options::default();
    options.theme = theme;
    options.keymap = keymap;
    Ok(options)
}
