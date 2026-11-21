//! The per-install token that authenticates Claude Code's `http` hooks.
//!
//! It is 32 random bytes in hex, kept in a file only its owner can read. The
//! daemon reads it at start and `volee doctor` writes it into the hook
//! settings: whoever lacks it cannot add events.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Reads the token at `path`, creating it when the file is missing or empty.
pub(crate) fn read_or_create(path: &Path) -> io::Result<String> {
    if let Some(token) = read(path)? {
        return Ok(token);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let token = generate()?;
    let created = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path);
    match created {
        Ok(mut file) => {
            writeln!(file, "{token}")?;
            Ok(token)
        }
        // Another command created it first: use its token.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if let Some(token) = read(path)? {
                return Ok(token);
            }
            // An empty leftover: replace it.
            fs::remove_file(path)?;
            read_or_create(path)
        }
        Err(error) => Err(error),
    }
}

/// The token in `path`, if the file exists and holds one. Tightens the
/// permissions of a file created by someone else.
fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let token = text.trim().to_owned();
            if token.is_empty() {
                return Ok(None);
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            Ok(Some(token))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn generate() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut token = String::with_capacity(64);
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(token, "{byte:02x}");
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_call_creates_a_private_token_and_later_calls_return_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/hook-token");
        let first = read_or_create(&path).unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(read_or_create(&path).unwrap(), first);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn two_installs_get_different_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let a = read_or_create(&dir.path().join("a")).unwrap();
        let b = read_or_create(&dir.path().join("b")).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn an_empty_file_gets_a_token_and_a_loose_file_is_tightened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook-token");
        fs::write(&path, "").unwrap();
        let token = read_or_create(&path).unwrap();
        assert_eq!(token.len(), 64);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read_or_create(&path).unwrap(), token);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
