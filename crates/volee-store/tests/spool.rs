//! Integration tests of the hook spool.

use std::fs;
use std::io::Write;
use std::sync::Arc;
use std::thread;

use serde_json::json;
use volee_store::spool::MAX_RECORD_BYTES;
use volee_store::{Spool, StoreError};

fn spool_files(spool: &Spool) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(spool.dir())
        .expect("spool directory is readable")
        .map(|entry| {
            entry
                .expect("entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn records_round_trip_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path().join("spool"));
    let records: Vec<_> = (0..5).map(|n| json!({"n": n, "text": "é\nline"})).collect();
    for record in &records {
        spool.append(record).unwrap();
    }
    let drained = spool.drain().unwrap();
    assert_eq!(drained.records, records);
    assert_eq!((drained.truncated, drained.invalid), (0, 0));
    assert_eq!(spool_files(&spool).len(), 0);
}

#[test]
fn draining_a_missing_or_empty_spool_returns_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let missing = Spool::new(dir.path().join("never-created"));
    assert_eq!(missing.drain().unwrap().records.len(), 0);
    let empty = Spool::new(dir.path().to_path_buf());
    assert_eq!(empty.drain().unwrap().records.len(), 0);
}

#[test]
fn a_second_drain_finds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path().to_path_buf());
    spool.append(&json!({"a": 1})).unwrap();
    assert_eq!(spool.drain().unwrap().records.len(), 1);
    assert_eq!(spool.drain().unwrap().records.len(), 0);
}

#[test]
fn appends_after_a_drain_start_a_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path().to_path_buf());
    spool.append(&json!({"a": 1})).unwrap();
    spool.drain().unwrap();
    spool.append(&json!({"a": 2})).unwrap();
    assert_eq!(spool.drain().unwrap().records, vec![json!({"a": 2})]);
}

#[test]
fn oversized_records_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path().to_path_buf());
    let big = json!({"blob": "x".repeat(MAX_RECORD_BYTES)});
    let error = spool.append(&big).unwrap_err();
    assert!(
        matches!(error, StoreError::RecordTooLarge { .. }),
        "{error}"
    );
    assert_eq!(spool.drain().unwrap().records.len(), 0);
    // A record just under the limit is accepted.
    let fits = json!({"blob": "x".repeat(MAX_RECORD_BYTES - 64)});
    spool.append(&fits).unwrap();
    assert_eq!(spool.drain().unwrap().records, vec![fits]);
}

#[test]
fn concurrent_appends_produce_whole_lines() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Arc::new(Spool::new(dir.path().to_path_buf()));
    let threads: usize = 8;
    let per_thread: u32 = 200;
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let spool = Arc::clone(&spool);
            thread::spawn(move || {
                for n in 0..per_thread {
                    // Large enough to make interleaving visible if it happened.
                    spool
                        .append(&json!({"thread": t, "n": n, "pad": "p".repeat(2_000)}))
                        .unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let drained = spool.drain().unwrap();
    assert_eq!((drained.truncated, drained.invalid), (0, 0));
    assert_eq!(drained.records.len(), threads * per_thread as usize);
    for t in 0..threads {
        let seen: Vec<i64> = drained
            .records
            .iter()
            .filter(|r| r["thread"] == t)
            .map(|r| r["n"].as_i64().unwrap())
            .collect();
        let expected: Vec<i64> = (0..i64::from(per_thread)).collect();
        assert_eq!(seen, expected, "thread {t} lost or reordered records");
    }
}

#[test]
fn leftovers_of_a_crashed_drain_are_read_first() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path().to_path_buf());
    fs::write(
        dir.path().join("spool-1.0.draining"),
        "{\"from\":\"leftover\"}\n",
    )
    .unwrap();
    spool.append(&json!({"from": "fresh"})).unwrap();
    let drained = spool.drain().unwrap();
    assert_eq!(
        drained.records,
        vec![json!({"from": "leftover"}), json!({"from": "fresh"})]
    );
    assert_eq!(spool_files(&spool).len(), 0);
}

#[test]
fn a_leftover_never_clashes_with_a_new_claim() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path().to_path_buf());
    let pid = std::process::id();
    // A leftover named like the first claim of the current process's file.
    fs::write(
        dir.path().join(format!("spool-{pid}.0.draining")),
        "{\"from\":\"leftover\"}\n",
    )
    .unwrap();
    spool.append(&json!({"from": "fresh"})).unwrap();
    let drained = spool.drain().unwrap();
    assert_eq!(drained.records.len(), 2);
}

#[test]
fn a_truncated_last_line_is_skipped_and_counted() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path().to_path_buf());
    spool.append(&json!({"ok": 1})).unwrap();
    let file = dir
        .path()
        .join(format!("spool-{}.ndjson", std::process::id()));
    fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(b"{\"cut\":")
        .unwrap();
    let drained = spool.drain().unwrap();
    assert_eq!(drained.records, vec![json!({"ok": 1})]);
    assert_eq!(drained.truncated, 1);
    assert_eq!(drained.invalid, 0);
}

#[test]
fn invalid_complete_lines_are_skipped_and_counted() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path().to_path_buf());
    fs::write(
        dir.path().join("spool-42.ndjson"),
        "{\"a\":1}\ngarbage\n{\"b\":2}\n",
    )
    .unwrap();
    let drained = spool.drain().unwrap();
    assert_eq!(drained.records, vec![json!({"a": 1}), json!({"b": 2})]);
    assert_eq!(drained.invalid, 1);
}

#[test]
fn unrelated_files_are_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path().to_path_buf());
    fs::write(dir.path().join("notes.txt"), "keep me").unwrap();
    spool.append(&json!({"a": 1})).unwrap();
    spool.drain().unwrap();
    assert_eq!(spool_files(&spool), vec!["notes.txt".to_owned()]);
}

#[cfg(unix)]
#[test]
fn spool_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path().to_path_buf());
    spool.append(&json!({"a": 1})).unwrap();
    let file = dir
        .path()
        .join(format!("spool-{}.ndjson", std::process::id()));
    let mode = fs::metadata(file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}
