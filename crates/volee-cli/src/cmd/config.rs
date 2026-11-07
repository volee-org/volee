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
    let config = Config::load(&file, &home)?;
    if file.exists() {
        println!("{} is valid.", file.display());
    } else {
        println!(
            "No configuration file at {}: using the defaults.",
            file.display()
        );
    }
    if let Some(model) = config.model_for(None) {
        println!("  default model     {model}");
    }
    match config.agents.permission_mode {
        Some(mode) => println!("  permission mode   {mode}"),
        None => println!("  permission mode   manual (the default)"),
    }
    println!("  wake timeout      {} s", config.agents.wake_timeout);
    println!("  worktrees         inside each repository, in .claude/worktrees");
    if config.repos.is_empty() {
        println!("  repositories      none (add [repos.<name>] tables)");
    }
    for (name, repo) in &config.repos {
        let exists = if repo.path.is_dir() {
            ""
        } else {
            "  (missing)"
        };
        println!("  repo {name:<12} {}{exists}", repo.path.display());
    }
    Ok(())
}
