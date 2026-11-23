//! `volee config path | init | check`.

use std::fs::OpenOptions;
use std::io::Write;

use anyhow::{Context, Result};
use volee_config::{Config, EXAMPLE};

use crate::connect::paths;

/// Prints where the configuration file lives.
pub(crate) fn path() -> Result<()> {
    println!("{}", paths()?.config_file().display());
    Ok(())
}

/// Writes the commented example, unless a file already exists.
pub(crate) fn init() -> Result<()> {
    let path = paths()?.config_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let file = OpenOptions::new().write(true).create_new(true).open(&path);
    match file {
        Ok(mut file) => {
            file.write_all(EXAMPLE.as_bytes())
                .with_context(|| format!("cannot write {}", path.display()))?;
            println!(
                "Wrote {}. Edit it to add your repositories.",
                path.display()
            );
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            println!("{} already exists; nothing changed.", path.display());
            Ok(())
        }
        Err(error) => Err(error).with_context(|| format!("cannot create {}", path.display())),
    }
}

/// Loads the configuration and summarizes it, or explains what is wrong.
pub(crate) fn check() -> Result<()> {
    let paths = paths()?;
    let file = paths.config_file();
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    if let Some(legacy) = volee_config::legacy_file(&paths.config_dir) {
        println!("warning: {}", volee_config::legacy_warning(&legacy));
    }
    let config = Config::load(&file, &home)?;
    if file.exists() {
        println!("{} is valid.", file.display());
    } else {
        println!(
            "No configuration file at {}: using the defaults.",
            file.display()
        );
    }
    println!("  background fetch  {}", config.git.fetch);
    if config.repos.is_empty() {
        println!("  repositories      none (add entries to `repos`)");
    }
    for line in repository_lines(&config, std::path::Path::is_dir) {
        println!("{line}");
    }
    Ok(())
}

/// One line per repository, its path aligned after the longest name. `exists`
/// tells whether a path is a folder on this machine.
fn repository_lines(config: &Config, exists: impl Fn(&std::path::Path) -> bool) -> Vec<String> {
    let width = config
        .repos
        .keys()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(0)
        .max(12);
    let mut lines = Vec::new();
    for (name, repo) in &config.repos {
        let missing = if exists(&repo.path) {
            ""
        } else {
            "  (missing)"
        };
        lines.push(format!(
            "  repo {name:<width$} {}{missing}",
            repo.path.display()
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn repositories_align_on_the_longest_name() {
        let config = Config::parse(
            "return { repos = {\
             shop = { path = '/srv/shop' },\
             ['simulateur-freelance'] = { path = '/srv/sim' } } }",
            Path::new("config.lua"),
            Path::new("/home/ada"),
        )
        .unwrap();
        let lines = repository_lines(&config, |path| path != Path::new("/srv/sim"));
        assert_eq!(
            lines,
            [
                "  repo shop                 /srv/shop",
                "  repo simulateur-freelance /srv/sim  (missing)",
            ]
        );
    }
}
