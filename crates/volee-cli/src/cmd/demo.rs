//! `volee demo`: the interface on a simulated team of agents.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use volee_config::Config;
use volee_tui::{ColorDepth, DemoSource, Keymap, Options, SystemClock, Theme};

use crate::connect::paths;

/// Runs the interface on the demo until the person quits. The theme and the
/// key overrides come from the configuration; `theme` replaces the
/// configured theme.
pub(crate) async fn run(theme: Option<&str>) -> Result<()> {
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

    let clock = Arc::new(SystemClock);
    let mut options = Options::default();
    options.theme = theme;
    options.keymap = keymap;
    options.clock = clock.clone();
    options.banner = Some("demo · simulated data".to_owned());
    volee_tui::run(DemoSource::new(clock), options).await?;
    Ok(())
}
