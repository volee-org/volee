//! `volee demo`: the interface on a simulated team of agents.

use std::sync::Arc;

use anyhow::Result;
use volee_tui::{DemoSource, SystemClock};

use crate::cmd::interface::options;

/// Runs the interface on the demo until the person quits. The theme and the
/// key overrides come from the configuration; `theme` replaces the
/// configured theme.
pub(crate) async fn run(theme: Option<&str>) -> Result<()> {
    let clock = Arc::new(SystemClock);
    let demo = DemoSource::new(clock.clone());
    let mut options = options(theme)?;
    options.clock = clock;
    options.banner = Some("demo · simulated data".to_owned());
    // The demo's own repositories, so its launch dialogs have choices.
    options.repos = demo.repos();
    volee_tui::run(demo, options).await?;
    Ok(())
}
