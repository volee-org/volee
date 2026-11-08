//! A real host, with real programs in a real pseudo-terminal.
#![allow(missing_docs)]

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use volee_host::{Connection, Event, HostConfig, HostDir, Role, Size, Update, Watch, run};

const SIZE: Size = Size { cols: 60, rows: 12 };

/// Starts a host running `command` in a temporary directory, and waits for
/// its socket.
fn start(dir: &HostDir, id: &str, command: &[&str]) -> thread::JoinHandle<()> {
    let config = HostConfig {
        id: id.to_owned(),
        dir: dir.clone(),
        command: command.iter().map(|part| (*part).to_owned()).collect(),
        cwd: dir.path().to_path_buf(),
        env: Vec::new(),
        size: SIZE,
        profile: "generic".to_owned(),
        scrollback: 100,
        linger: Duration::from_millis(300),
    };
    let handle = thread::spawn(move || {
        run(config).expect("host runs");
    });
    wait_until(|| std::os::unix::net::UnixStream::connect(dir.socket(id)).is_ok());
    handle
}

fn wait_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

fn watch_until(watch: &mut Watch, mut done: impl FnMut(&mut Watch) -> bool) {
    wait_until(|| {
        watch.pump();
        done(watch)
    });
}

fn screen_has(watch: &mut Watch, text: &str) -> bool {
    watch.text().is_some_and(|screen| screen.contains(text))
}

fn control(socket: &Path, size: Size) -> Connection {
    Connection::open(socket, Role::Control, size, "test").expect("connect")
}

#[test]
fn a_watcher_sees_what_the_owner_types() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = HostDir::new(root.path().join("hosts"));
    let host = start(&dir, "typing", &["cat"]);

    let mut watch = Watch::start(&dir.socket("typing"), "test").expect("watch");
    assert!(!watch.welcome().input, "a watcher never gets the input");
    let owner = control(&dir.socket("typing"), SIZE);
    assert!(owner.welcome.input);
    owner
        .sender()
        .expect("sender")
        .input(b"hello host\r")
        .expect("input");
    watch_until(&mut watch, |w| screen_has(w, "hello host"));

    // A late watcher gets the whole screen from its first snapshot.
    let mut late = Watch::start(&dir.socket("typing"), "test").expect("watch");
    watch_until(&mut late, |w| screen_has(w, "hello host"));

    owner.sender().expect("sender").stop().expect("stop");
    watch_until(&mut watch, |w| w.exit().is_some());
    host.join().expect("host thread");
    let records = dir.list().expect("list");
    assert_eq!(records.len(), 1);
    assert!(records[0].0.exit.is_some(), "the record says it ended");
    assert!(!records[0].1, "the socket is gone");
}

#[test]
fn keys_from_a_watcher_are_ignored() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = HostDir::new(root.path().join("hosts"));
    let host = start(&dir, "ignored", &["cat"]);

    let watcher =
        Connection::open(&dir.socket("ignored"), Role::Watch, SIZE, "test").expect("connect");
    watcher
        .sender()
        .expect("sender")
        .input(b"sneaky\r")
        .expect("input");
    let owner = control(&dir.socket("ignored"), SIZE);
    owner
        .sender()
        .expect("sender")
        .input(b"allowed\r")
        .expect("input");
    let mut watch = Watch::start(&dir.socket("ignored"), "test").expect("watch");
    watch_until(&mut watch, |w| screen_has(w, "allowed"));
    assert!(!screen_has(&mut watch, "sneaky"));

    owner.sender().expect("sender").stop().expect("stop");
    host.join().expect("host thread");
}

#[test]
fn the_owner_sets_the_size_and_watchers_follow() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = HostDir::new(root.path().join("hosts"));
    let host = start(
        &dir,
        "sizes",
        &["sh", "-c", "while :; do stty size; sleep 0.1; done"],
    );

    let mut watch = Watch::start(&dir.socket("sizes"), "test").expect("watch");
    watch_until(&mut watch, |w| screen_has(w, "12 60"));
    // A watcher with another size changes nothing.
    let _other = Connection::open(
        &dir.socket("sizes"),
        Role::Watch,
        Size {
            cols: 200,
            rows: 50,
        },
        "test",
    )
    .expect("connect");
    let wide = Size { cols: 90, rows: 20 };
    let owner = control(&dir.socket("sizes"), wide);
    watch_until(&mut watch, |w| w.size() == wide && screen_has(w, "20 90"));
    assert_eq!(dir.list().expect("list")[0].0.size, wide);

    owner.sender().expect("sender").stop().expect("stop");
    host.join().expect("host thread");
}

#[test]
fn a_second_owner_takes_the_input() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = HostDir::new(root.path().join("hosts"));
    let host = start(&dir, "takeover", &["cat"]);

    let mut first = control(&dir.socket("takeover"), SIZE);
    let second = control(&dir.socket("takeover"), SIZE);
    assert!(second.welcome.input);
    let mut taken = false;
    while let Ok(Some(update)) = first.next_update() {
        if update == Update::Event(Event::InputTaken) {
            taken = true;
        }
    }
    assert!(taken, "the first owner was told, then disconnected");

    second.sender().expect("sender").stop().expect("stop");
    host.join().expect("host thread");
}

#[test]
fn a_query_is_answered_while_nobody_owns_the_input() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = HostDir::new(root.path().join("hosts"));
    // Asks for the cursor position, then prints the answer it read back.
    let script = "stty raw -echo; printf '\\033[6n'; reply=$(dd bs=1 count=6 2>/dev/null); \
                  stty sane; printf '\\ngot %s\\n' \"$reply\" | tr -d '\\033'; sleep 5";
    let host = start(&dir, "query", &["sh", "-c", script]);

    let mut watch = Watch::start(&dir.socket("query"), "test").expect("watch");
    watch_until(&mut watch, |w| screen_has(w, "got [1;1R"));

    let owner = control(&dir.socket("query"), SIZE);
    owner.sender().expect("sender").stop().expect("stop");
    host.join().expect("host thread");
}
