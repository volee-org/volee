//! The hook spool: where `volee hook` writes when the daemon is unreachable.
//!
//! A spool is a directory of newline-delimited JSON files, one per writing
//! process (`spool-<pid>.ndjson`). Writers only append; the daemon drains the
//! directory at start (ADR 0003).
//!
//! # Why a line is never torn
//!
//! Each record is written with a single `write_all` on a file opened with
//! `O_APPEND`, so the kernel places it at the end of the file in one step.
//! Within one process that makes lines whole. Across processes the guarantee
//! holds for writes up to `PIPE_BUF` on pipes and, in practice, for regular
//! local files too, but POSIX only promises it for pipes. Records are
//! therefore small (at most [`MAX_RECORD_BYTES`]) and each process writes to
//! its own file, so concurrent hook processes never share one.
//!
//! # Crash behavior
//!
//! A writer killed mid-write can leave a last line without its newline.
//! Draining skips and counts it rather than failing, because it was never
//! acknowledged to the hook's caller.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::error::StoreError;

/// The largest accepted record, newline included. Keeps every line within one
/// atomic append.
pub const MAX_RECORD_BYTES: usize = 32 * 1024;

const SPOOL_PREFIX: &str = "spool-";
const SPOOL_SUFFIX: &str = ".ndjson";
const DRAINING_SUFFIX: &str = ".draining";

/// An append-only directory of JSON records.
#[derive(Clone, Debug)]
pub struct Spool {
    dir: PathBuf,
}

/// What a drain found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drained {
    /// The complete records, per file in write order. Files are visited in
    /// name order, leftovers of a crashed drain first.
    pub records: Vec<serde_json::Value>,
    /// Final lines that lack their newline (a writer died mid-write).
    pub truncated: usize,
    /// Complete lines that are not valid JSON.
    pub invalid: usize,
}

impl Spool {
    /// A spool in `dir`. The directory is created on the first append.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The directory of the spool.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Appends one record as a line of `spool-<pid>.ndjson`.
    ///
    /// Fails with [`StoreError::RecordTooLarge`] when the encoded line is
    /// over [`MAX_RECORD_BYTES`]. No `fsync`: the spool survives a crash of
    /// volee, and a power loss may cost the last records.
    pub fn append(&self, record: &serde_json::Value) -> Result<(), StoreError> {
        let mut line = serde_json::to_vec(record).map_err(StoreError::Serialize)?;
        line.push(b'\n');
        if line.len() > MAX_RECORD_BYTES {
            return Err(StoreError::RecordTooLarge {
                size: line.len(),
                max: MAX_RECORD_BYTES,
            });
        }
        fs::create_dir_all(&self.dir)
            .map_err(|source| StoreError::io_at("create directory", &self.dir, source))?;
        let path = self.dir.join(format!(
            "{SPOOL_PREFIX}{}{SPOOL_SUFFIX}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|source| StoreError::io_at("open spool file", &path, source))?;
        file.write_all(&line)
            .map_err(|source| StoreError::io_at("write to spool file", &path, source))
    }

    /// Takes every record out of the spool.
    ///
    /// Each `spool-*.ndjson` is first renamed to a `.draining` file, so new
    /// appends start a fresh file; `.draining` files left by a crashed drain
    /// are read first. A file is deleted only after it was read completely, so
    /// a crash during a drain loses nothing (records may be returned twice if
    /// the caller itself crashes before storing them, so ingestion should
    /// tolerate a repeated record).
    pub fn drain(&self) -> Result<Drained, StoreError> {
        let mut drained = Drained::default();
        let mut files = self.list_draining()?;
        for spool_file in self.list_spool_files()? {
            files.push(self.claim(&spool_file)?);
        }
        for path in files {
            read_all_stable(&path, &mut drained)?;
            fs::remove_file(&path)
                .map_err(|source| StoreError::io_at("delete drained file", &path, source))?;
        }
        if drained.truncated > 0 || drained.invalid > 0 {
            tracing::warn!(
                truncated = drained.truncated,
                invalid = drained.invalid,
                "skipped damaged spool lines"
            );
        }
        Ok(drained)
    }

    /// Renames a spool file out of the way under a name that cannot clash with
    /// a leftover (a reused pid after a crash would otherwise overwrite it).
    fn claim(&self, spool_file: &Path) -> Result<PathBuf, StoreError> {
        let stem = file_name(spool_file);
        let stem = stem.strip_suffix(SPOOL_SUFFIX).unwrap_or(&stem);
        for attempt in 0_u32.. {
            let target = self.dir.join(format!("{stem}.{attempt}{DRAINING_SUFFIX}"));
            if !target.exists() {
                fs::rename(spool_file, &target)
                    .map_err(|source| StoreError::io_at("claim spool file", spool_file, source))?;
                return Ok(target);
            }
        }
        unreachable!("an unused draining name always exists")
    }

    fn list_spool_files(&self) -> Result<Vec<PathBuf>, StoreError> {
        self.list(|name| name.starts_with(SPOOL_PREFIX) && name.ends_with(SPOOL_SUFFIX))
    }

    fn list_draining(&self) -> Result<Vec<PathBuf>, StoreError> {
        self.list(|name| name.starts_with(SPOOL_PREFIX) && name.ends_with(DRAINING_SUFFIX))
    }

    /// Files of the spool directory whose name matches, sorted by name. A
    /// missing directory is an empty spool.
    fn list(&self, wanted: impl Fn(&str) -> bool) -> Result<Vec<PathBuf>, StoreError> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(StoreError::io_at("read directory", &self.dir, source)),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|source| StoreError::io_at("read directory", &self.dir, source))?;
            if wanted(&file_name(&entry.path())) {
                paths.push(entry.path());
            }
        }
        paths.sort();
        Ok(paths)
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Reads a claimed file. A writer that opened the file just before it was
/// renamed may still append to it, so the file is read again until its size
/// stops changing; only the records of the final read are kept.
fn read_all_stable(path: &Path, drained: &mut Drained) -> Result<(), StoreError> {
    let mut bytes = read_file(path)?;
    loop {
        let size = fs::metadata(path)
            .map_err(|source| StoreError::io_at("inspect", path, source))?
            .len();
        if size == bytes.len() as u64 {
            break;
        }
        bytes = read_file(path)?;
    }
    parse_lines(&bytes, drained);
    Ok(())
}

fn read_file(path: &Path) -> Result<Vec<u8>, StoreError> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|source| StoreError::io_at("read spool file", path, source))?;
    Ok(bytes)
}

fn parse_lines(bytes: &[u8], drained: &mut Drained) {
    let mut rest = bytes;
    while !rest.is_empty() {
        let Some(end) = rest.iter().position(|&byte| byte == b'\n') else {
            // No newline: the writer died mid-write.
            drained.truncated += 1;
            return;
        };
        let (line, tail) = rest.split_at(end);
        rest = &tail[1..];
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(record) => drained.records.push(record),
            Err(_) => drained.invalid += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parse_lines_separates_whole_truncated_and_invalid_lines() {
        let mut drained = Drained::default();
        parse_lines(b"{\"a\":1}\nnot json\n\n{\"b\":2}\n{\"c\":", &mut drained);
        assert_eq!(drained.records, vec![json!({"a": 1}), json!({"b": 2})]);
        assert_eq!(drained.invalid, 1);
        assert_eq!(drained.truncated, 1);
    }
}
