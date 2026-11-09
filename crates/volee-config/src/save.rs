//! Saving a setting without rewriting the rest of the file.

use std::io;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, value};

use crate::ConfigError;

/// Sets `[tui] theme = "<theme>"` in the configuration file at `path`.
///
/// Comments, key order and every other setting stay as they were. The file,
/// its parent directory and the `[tui]` table are created when missing. The
/// file is replaced atomically (a temporary file in the same directory, then a
/// rename), and left untouched when it is not valid TOML.
pub fn set_theme(path: &Path, theme: &str) -> Result<(), ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    let mut doc: DocumentMut =
        text.parse()
            .map_err(|error: toml_edit::TomlError| ConfigError::Parse {
                path: path.to_owned(),
                message: error.to_string().trim_end().to_owned(),
            })?;
    let tui = doc
        .entry("tui")
        .or_insert_with(|| Item::Table(Table::new()));
    let Some(tui) = tui.as_table_mut() else {
        return Err(ConfigError::Invalid {
            path: path.to_owned(),
            message: "`tui` must be a table (`[tui]`)".to_owned(),
        });
    };
    tui.set_implicit(false);
    match tui.get_mut("theme") {
        // Replace the value but keep the comments around it.
        Some(item) if item.is_value() => {
            let decor = item.as_value().map(|old| old.decor().clone());
            *item = value(theme);
            if let (Some(decor), Some(new)) = (decor, item.as_value_mut()) {
                *new.decor_mut() = decor;
            }
        }
        _ => {
            tui.insert("theme", value(theme));
        }
    }
    write_atomically(path, &doc.to_string())
}

fn write_atomically(path: &Path, text: &str) -> Result<(), ConfigError> {
    let write = |source| ConfigError::Write {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(write)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let temp: PathBuf = path.with_file_name(name);
    let result = std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, path));
    if let Err(source) = result {
        // Best effort: the temporary file may not exist.
        let _ = std::fs::remove_file(&temp);
        return Err(write(source));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, KeyList, KeySetting};

    fn theme_of(path: &Path) -> Option<String> {
        Config::load(path, path).unwrap().tui.theme
    }

    #[test]
    fn a_missing_file_and_directory_are_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("volee/config.toml");
        set_theme(&path, "catppuccin-latte").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[tui]\ntheme = \"catppuccin-latte\"\n"
        );
        assert_eq!(theme_of(&path).as_deref(), Some("catppuccin-latte"));
    }

    #[test]
    fn comments_order_and_other_sections_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let before = "# my volee\n[agents]\nmodel = \"opus\" # the big one\n\n\
                      [repos.api]\npath = \"/srv/api\"\n";
        std::fs::write(&path, before).unwrap();
        set_theme(&path, "terminal").unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.starts_with(before), "{after}");
        assert!(after.ends_with("[tui]\ntheme = \"terminal\"\n"), "{after}");
    }

    #[test]
    fn an_existing_theme_is_replaced_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[tui]\n# colors\ntheme = \"old\" # why not\nreduced_motion = true\n",
        )
        .unwrap();
        set_theme(&path, "new").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[tui]\n# colors\ntheme = \"new\" # why not\nreduced_motion = true\n"
        );
    }

    #[test]
    fn other_tui_keys_and_subtables_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[tui]\nreduced_motion = true\n\n[tui.keys]\ntake-over = \"ctrl-a\"\n",
        )
        .unwrap();
        set_theme(&path, "terminal").unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            after,
            "[tui]\nreduced_motion = true\ntheme = \"terminal\"\n\n[tui.keys]\ntake-over = \"ctrl-a\"\n"
        );
        let config = Config::load(&path, dir.path()).unwrap();
        assert!(config.tui.reduced_motion);
        assert_eq!(
            config.tui.keys["take-over"],
            KeySetting::Keys(KeyList::One("ctrl-a".into()))
        );
    }

    #[test]
    fn a_tui_table_that_only_has_subtables_gets_its_theme() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[tui.keys]\ntake-over = \"ctrl-a\"\n").unwrap();
        set_theme(&path, "terminal").unwrap();
        assert_eq!(theme_of(&path).as_deref(), Some("terminal"));
    }

    #[test]
    fn invalid_toml_is_refused_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[tui\ntheme = \n").unwrap();
        let error = set_theme(&path, "terminal").unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[tui\ntheme = \n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_tui_that_is_not_a_table_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "tui = 3\n").unwrap();
        let error = set_theme(&path, "terminal").unwrap_err();
        assert!(matches!(error, ConfigError::Invalid { .. }), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "tui = 3\n");
    }

    #[test]
    fn an_unreadable_path_is_a_read_error() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the file should be.
        let error = set_theme(dir.path(), "terminal").unwrap_err();
        assert!(matches!(error, ConfigError::Read { .. }), "{error}");
    }
}
