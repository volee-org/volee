# Day of use: volee v2 next to volee 0.18

For Jeff's machine, where volee 0.18 runs every day (`~/.local/bin/volee`, its
daemon, `~/.local/state/volee`, `~/.config/volee/config.toml`). Nothing here
publishes anything: v2 is built from a checkout and installed locally.

## What clashes, measured on copies (2026-11-23)

Tried with copies of the 0.18 database and configuration in scratch
directories, never on the real ones, never against the real 0.18 daemon.

| Shared by default | What happened |
| --- | --- |
| **The database** `~/.local/state/volee/volee.db` | A v2 command that opens it (the daemon, `volee events`...) **migrated it silently** from schema 1 to 6, keeping the 0.x `events` and `meta` tables untouched and adding empty v2 tables. The 0.18 binary then **refuses to open it**: `the database schema is version 6, newer than the version 1 this volee supports`. |
| **The socket and the lock** `~/.local/state/volee/volee.sock`, `daemon.lock` | v2 and 0.18 speak to the same socket: `volee daemon status` of v2 showed the 0.18 daemon, `volee daemon start` said "already running" (so v2's own daemon never started and `volee ui` had nothing to talk to), and **`volee daemon stop` of v2 stopped the 0.18 daemon**. |
| The configuration | No clash: v2 reads `config.lua`, 0.18 `config.toml`. v2 only prints a warning that the `.toml` is ignored. |
| The hook port | No clash: 0.18 uses no TCP port; v2 listens on 127.0.0.1:47835. |

Two changes make this safe:

1. **v2 refuses a 0.x database** (schema version 1): no silent migration, ever.
   The daemon says `this database belongs to volee 0.x (N events) and v2 does not
   migrate it`, and `volee doctor` prints a `MISSING` line for it. Tested: the
   file stays byte for byte identical.
2. **`volee2`**, written next to the binary by `scripts/install-local.sh`, is the
   same program with its own directories (`~/.local/volee-v2/home/{config,state,data}`,
   its own socket). With it, v2 and 0.18 never meet; both daemons were run side by
   side, and `volee2 daemon stop` left the 0.x daemon running.

**Use `volee2`, never plain `volee`, for v2.** `~/.local/bin/volee` stays 0.18.

## Install

From a checkout of the `v2` branch (needs Rust, Node, `dist`: `brew install
cargo-dist`; `rustup` is keg-only, see below):

```sh
cd ~/Developer/Labs/volee
git switch v2 && git pull --ff-only         # or the worktree of the v2 branch
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH   # fish: fish_add_path ...
scripts/install-local.sh                    # builds the release archive, runs the dist installer on 127.0.0.1
~/.local/volee-v2/bin/volee2 --version
```

It installs `~/.local/volee-v2/bin/volee` (the program) and `volee2` (the
wrapper). It touches neither `~/.local/bin` nor your shell profile. To type
`volee2` without the path, add the directory in fish:
`fish_add_path ~/.local/volee-v2/bin` (the name `volee2` does not shadow
`volee`; do not put that directory before `~/.local/bin` and call plain `volee`).

## Set up, start, open

```sh
volee2 doctor --project ~/Developer/Labs/slapeo      # once per repository you want watched
volee2 doctor --project ~/Developer/Labs/volee       # ... and so on
volee2 daemon start
volee2 ui                                             # prints a link and opens the inbox
```

- `doctor` first prints its checks (`ok`, `MISSING`, `optional`), then writes volee's
  hooks in that project's `.claude/settings.local.json` (the file Claude Code keeps
  out of git) and nowhere else. It never touches `~/.claude/settings.json`.
- Claude Code reads hooks when a session starts: **start new `claude` sessions in
  those repositories** to see them. Sessions already running report nothing. Agents
  that volee 0.18 launches use their own `--settings`; they are unaffected.
- Optional: `volee2 config init` writes `~/.local/volee-v2/home/config/config.lua`
  (hook port, repositories, `claude.command`); rules go in `rules.lua` next to it or in
  `<repo>/.volee/rules.lua` ([rules reference](../src/reference/rules.md)). For automatic
  merge, list a repository in `auto_merge_repos` in the **personal** `rules.lua` only.
- The inbox is at `http://127.0.0.1:47835/` with a token in the link `volee2 ui` prints.
  Bookmark nothing: the token is the key; run `volee2 ui` again for a fresh link.

## Update (a new build of v2)

```sh
scripts/install-local.sh
volee2 daemon stop && volee2 daemon start      # a daemon keeps the build that started it
```

## Stop, uninstall, go back to 0.18 only

```sh
volee2 doctor --remove --project ~/Developer/Labs/slapeo   # each repository you set up
volee2 daemon stop
rm -rf ~/.local/volee-v2                                    # binary, wrapper, v2's own config, state and data
```

That is all: v2 never used 0.18's files, and it launches nothing, so no worktree or
branch is left behind.

## The 0.18 database is already at schema 6: Jeff decides

On 2026-11-23 the real `~/.local/state/volee/volee.db` was found at **schema version 6,
with v2's tables in it** (all empty: `hook_events`, `journal`, `reviews`,
`risk_overrides`, `line_comments`, `branch_memory`, `pull_request_cache`,
`review_findings`), last written 2026-11-22 04:27. Its 0.x tables hold 8279 events
(2026-11-08 to 2026-11-21), unchanged: the migrations 2 to 6 only add tables
(`CREATE TABLE`/`CREATE INDEX`, no `ALTER`, `DROP`, `UPDATE` or `DELETE`). A v2 build
from the step 9 work (migration 6 is its commit, 04:24) opened the default state
directory once, with nothing written to it: a command or a test run without
`VOLEE_STATE_DIR`, not a daemon (no hook event was stored, and `logs/daemon.log` is
0.18's, last written 2026-11-21).

Consequence: **volee 0.18 cannot open that file**, so its daemon will not start, and
there is no `volee.sock` in the directory now. Nothing was changed there by this work
(besides the harmless `-shm`/`-wal` that one read-only look created).

**Proposed repair, not done: Jeff decides.** Checked on a copy: setting the schema
version back to 1 makes 0.18 open the database again (`volee daemon status` lists
its agents and its events, `volee ls` lists the agents); the extra tables stay,
empty and ignored.

```sh
# 0.18's daemon must not be running (it is not, there is no socket)
cp ~/.local/state/volee/volee.db ~/.local/state/volee/volee.db.before-v2-schema
sqlite3 ~/.local/state/volee/volee.db "PRAGMA user_version=1;"
~/.local/bin/volee daemon status          # or `volee ls`: starts the 0.18 daemon again
```

A restore from `volee.db.before-0.16.0` would be worse: it is older than the 0.18 events.
After that, v2 refuses this file (schema 1) and `volee doctor` says so; use `volee2`.
