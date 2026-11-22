//! Embeds the web interface's bundle (`ui/dist`) in the daemon.
//!
//! The bundle is built by `npm run build` in `ui/`. Rust tests and plain
//! `cargo build` must not need Node, so when `ui/dist/index.html` is missing a
//! small stub page is embedded in its place (it says the interface was not
//! built). Release builds set `VOLEE_REQUIRE_UI=1`, which turns a missing
//! bundle into a build error instead, so a release can never ship the stub.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

const STUB: &str = "stub/index.html";

#[allow(clippy::print_stdout)]
fn main() {
    println!("cargo:rerun-if-env-changed=VOLEE_REQUIRE_UI");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={STUB}");
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let dist = manifest.join("../../ui/dist");
    // Watched by cargo: the whole folder when it exists, so that a new build
    // of the interface is picked up by the next `cargo build`.
    println!("cargo:rerun-if-changed={}", dist.display());

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let bundled = dist.join("index.html").is_file();
    if bundled {
        walk(&dist, &dist, &mut files);
    } else {
        assert!(
            env::var_os("VOLEE_REQUIRE_UI").is_none_or(|value| value.is_empty()),
            "VOLEE_REQUIRE_UI is set but ui/dist/index.html is missing: run `npm ci && npm run build` in ui/ first"
        );
        files.push(("index.html".to_owned(), manifest.join(STUB)));
    }
    files.sort();

    let mut code = format!(
        "/// Whether the real bundle is embedded (not the stub).\npub(super) const BUNDLED: bool = {bundled};\n"
    );
    code.push_str("/// Every embedded file: its path in the bundle and its bytes.\n");
    code.push_str("pub(super) static FILES: &[(&str, &[u8])] = &[\n");
    for (name, path) in &files {
        let path = path.canonicalize().expect("a file found while walking");
        writeln!(
            code,
            "    ({name:?}, include_bytes!({:?})),",
            path.to_string_lossy()
        )
        .expect("writing to a string");
    }
    code.push_str("];\n");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("set by cargo")).join("ui_assets.rs");
    fs::write(out, code).expect("cannot write the generated asset list");
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, files);
        } else if let Ok(relative) = path.strip_prefix(root) {
            files.push((relative.to_string_lossy().replace('\\', "/"), path));
        }
    }
}
