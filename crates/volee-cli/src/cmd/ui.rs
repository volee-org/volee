//! `volee ui`: opens the web interface.
//!
//! The daemon serves the interface on the hook server's loopback port. The
//! link carries the per-install token in its fragment (`#token=...`): a
//! fragment is never sent over the network, so it appears in no log, and the
//! page keeps it for the tab and removes it from the address bar.

use std::net::{Ipv4Addr, SocketAddr};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::doctor::{health, settings_port};
use crate::connect::paths;
use crate::hook_token;

/// Prints the link and, unless `no_open`, opens it in the default browser.
pub(crate) fn run(no_open: bool) -> Result<()> {
    let paths = paths()?;
    let port = settings_port(&paths);
    if let Err(why) = health(SocketAddr::from((Ipv4Addr::LOCALHOST, port))) {
        bail!(
            "the daemon does not answer on 127.0.0.1:{port} ({why}). Start it with `volee daemon start`."
        );
    }
    let token = hook_token::read_or_create(&paths.hook_token_file())
        .with_context(|| format!("cannot read {}", paths.hook_token_file().display()))?;
    let url = link(port, &token);
    println!("{url}");
    if no_open {
        return Ok(());
    }
    if let Err(why) = open(&url) {
        println!("Could not open the browser ({why}); open the link above.");
    }
    Ok(())
}

/// The address of the interface with its token.
fn link(port: u16, token: &str) -> String {
    format!("http://127.0.0.1:{port}/#token={token}")
}

fn open(url: &str) -> std::io::Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(program)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!("{program} failed")))
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_travels_in_the_fragment_never_the_query() {
        let url = link(47835, "abc123");
        assert_eq!(url, "http://127.0.0.1:47835/#token=abc123");
        assert!(!url.contains('?'));
    }
}
