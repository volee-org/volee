//! Where hosts live, and the record each one keeps (ADR 0019).

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::proto::{Exit, Size};

/// The directory of hosts: `<state>/hosts`, readable by its owner only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDir {
    path: PathBuf,
}

impl HostDir {
    /// The directory at `path`; nothing is created yet.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Its path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Creates the directory if needed, mode 0700, and tightens an existing
    /// one to 0700.
    pub fn ensure(&self) -> io::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.path)?;
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700))
    }

    /// The socket of host `id`.
    pub fn socket(&self, id: &str) -> PathBuf {
        self.path.join(format!("{id}.sock"))
    }

    /// The record of host `id`.
    pub fn record(&self, id: &str) -> PathBuf {
        self.path.join(format!("{id}.json"))
    }

    /// The log of host `id`.
    pub fn log(&self, id: &str) -> PathBuf {
        self.path.join(format!("{id}.log"))
    }

    /// Every record in the directory, oldest first, each with whether its
    /// host answers on its socket.
    pub fn list(&self) -> io::Result<Vec<(HostRecord, bool)>> {
        let entries = match fs::read_dir(&self.path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut records = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Ok(record) = HostRecord::read(&path) else {
                continue;
            };
            let live = UnixStream::connect(self.socket(&record.id)).is_ok();
            records.push((record, live));
        }
        records.sort_by_key(|(record, _)| record.started_at);
        Ok(records)
    }
}

/// What a host says about itself, in `<id>.json`. It survives the host, and
/// a reboot (ADR 0024).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecord {
    /// The host's id: the agent's id.
    pub id: String,
    /// The protocol version the host speaks.
    pub protocol: u32,
    /// The host's process id.
    pub host_pid: u32,
    /// The program's process id.
    pub child_pid: Option<u32>,
    /// The harness profile (ADR 0022), `generic` for any command line.
    pub profile: String,
    /// The display name of the agent it runs, shown by its clients.
    #[serde(default)]
    pub name: Option<String>,
    /// The program's command line.
    pub command: Vec<String>,
    /// Its working directory.
    pub cwd: PathBuf,
    /// When it started, in seconds since the Unix epoch.
    pub started_at: u64,
    /// Its current size.
    pub size: Size,
    /// How it ended, once it did.
    pub exit: Option<Exit>,
}

impl HostRecord {
    /// Reads a record.
    pub fn read(path: &Path) -> io::Result<Self> {
        let bytes = fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    /// Writes the record atomically: a temporary file, then a rename.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, path)
    }
}

/// Seconds since the Unix epoch.
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, started_at: u64) -> HostRecord {
        HostRecord {
            id: id.to_owned(),
            protocol: crate::proto::PROTOCOL,
            host_pid: 1,
            child_pid: Some(2),
            profile: "generic".to_owned(),
            name: None,
            command: vec!["sh".to_owned()],
            cwd: PathBuf::from("/tmp"),
            started_at,
            size: Size { cols: 80, rows: 24 },
            exit: None,
        }
    }

    #[test]
    fn the_directory_is_private() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = HostDir::new(root.path().join("state/hosts"));
        dir.ensure().expect("ensure");
        let mode = fs::metadata(dir.path())
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn records_are_listed_oldest_first_and_dead_without_a_socket() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = HostDir::new(root.path());
        record("b", 20).write(&dir.record("b")).expect("write");
        record("a", 10).write(&dir.record("a")).expect("write");
        fs::write(dir.path().join("junk.json"), "not json").expect("write");
        let listed = dir.list().expect("list");
        let ids: Vec<_> = listed
            .iter()
            .map(|(r, live)| (r.id.as_str(), *live))
            .collect();
        assert_eq!(ids, [("a", false), ("b", false)]);
    }

    #[test]
    fn a_missing_directory_lists_nothing() {
        let dir = HostDir::new("/nonexistent/volee/hosts");
        assert_eq!(dir.list().expect("list"), Vec::new());
    }
}
