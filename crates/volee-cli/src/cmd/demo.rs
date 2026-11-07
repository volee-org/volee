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
    let mut options = options(theme)?;
    options.clock = clock.clone();
    options.banner = Some("demo · simulated data".to_owned());
    volee_tui::run(DemoSource::new(clock), options).await?;
    Ok(())
}
