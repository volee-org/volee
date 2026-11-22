# Installation

## Requirements

- macOS (Apple Silicon or Intel) or Linux (x86_64 or ARM64).
- [Claude Code](https://code.claude.com) 2.1.330 or later. volee observes it
  through its hooks and `claude agents --json`; `claude update` brings it up to
  date.
- git 2.40 or later.
- A web browser: the decision inbox is a web page the daemon serves on
  127.0.0.1.
- Optional: the [GitHub CLI](https://cli.github.com) (`gh`), signed in with
  `gh auth login`. With it, volee shows the pull request opened from each
  agent's branch and the state of its checks. Without it, everything else
  works. volee uses your own sign-in and never sees a token.

`volee doctor` (see [First run](#first-run)) checks all of this and says what
is missing.

## With the installer

> [!NOTE]
> volee v2 is not released yet. Until `v2.0.0` is published, the commands in
> this section install the previous release; install v2 [from source](#from-source)
> or from [a local build](#test-the-installer-without-a-release).

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/volee-org/volee/releases/latest/download/volee-cli-installer.sh | sh
```

The script picks the archive for your machine, checks its SHA-256 checksum and
installs the `volee` command into `~/.local/bin` (or `$XDG_BIN_HOME` when it
is set). If that directory is not on your `PATH`, the script adds it to your
shell profile: open a new terminal afterwards. The web interface is inside the
binary: there is nothing else to install.

To install a specific version, put its tag in the URL:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/volee-org/volee/releases/download/v2.0.0/volee-cli-installer.sh | sh
```

To choose the directory, set `VOLEE_CLI_INSTALL_DIR` for the script
(`VOLEE_CLI_NO_MODIFY_PATH=1` leaves your shell profile alone):

```sh
curl --proto '=https' --tlsv1.2 -LsSf <url> | VOLEE_CLI_INSTALL_DIR=$HOME/tools/bin VOLEE_CLI_NO_MODIFY_PATH=1 sh
```

You can also download an archive from the release page yourself, check it
against its `.sha256` file and put `volee` anywhere on your `PATH`:

| Platform | Archive |
| --- | --- |
| macOS, Apple Silicon | `volee-cli-aarch64-apple-darwin.tar.xz` |
| macOS, Intel | `volee-cli-x86_64-apple-darwin.tar.xz` |
| Linux, x86_64 | `volee-cli-x86_64-unknown-linux-gnu.tar.xz` |
| Linux, ARM64 | `volee-cli-aarch64-unknown-linux-gnu.tar.xz` |

The Linux binaries need glibc 2.35 or later (Ubuntu 22.04, Debian 12,
Fedora 36 or newer). On other Linux systems, such as Alpine, install from
source.

## From source

You need Rust 1.90 or later and a recent Node (24 in CI): the web interface is built
with Node and embedded in the binary. `cargo install --git` is **not**
enough, because the built interface is not in the repository and the binary
would only show a page saying so.

```sh
git clone https://github.com/volee-org/volee
cd volee
(cd ui && npm ci && npm run build)
cargo install --locked --path crates/volee-cli
```

The `volee` command goes into `~/.cargo/bin`.

## Test the installer without a release

`scripts/install-local.sh [PREFIX]` builds the release archive for this
machine with dist, then runs the real dist installer against it, served from
`127.0.0.1`. Nothing is downloaded from GitHub and nothing is published. The
script checks the archive's SHA-256 first, then the installer puts `volee` in
`PREFIX` (default `~/.local/volee-v2/bin`) and leaves your shell profile alone.
It needs dist (`brew install cargo-dist`), Rust and Node.

## First run

Four commands take you from a fresh install to the inbox. Run them in a
terminal, in the repository you work in:

```sh
volee --version
volee doctor
volee daemon start
volee ui
```

1. `volee doctor` first prints what it checked: `ok` (found), `MISSING` (volee
   cannot work without it, with what to do) or `optional`. It then writes
   volee's hooks in **the project's** `.claude/settings.local.json`, the file
   Claude Code keeps out of version control, and says whether the daemon
   answers. It never touches `~/.claude/settings.json` (only
   `volee doctor --user` does, on purpose) and keeps every other setting and
   hook of the file. Run it again in each repository you want volee to watch,
   or give the directory: `volee doctor --project ~/code/my-repo`. It is safe
   to repeat.
2. `volee daemon start` starts the daemon in the background. It listens on
   127.0.0.1 only (port 47835 by default). Hook and interface calls carry the
   per-install token that `volee doctor` created in the state directory.
3. `volee ui` prints a link to the inbox (the token is in its fragment, so the
   link works on this machine only) and opens it in your browser. `--no-open`
   only prints it.
4. Start (or restart) `claude` in that repository: Claude Code reads its hooks
   when a session starts. When an agent finishes, asks a question or needs a
   permission, a card appears in the inbox.

Optional, once:

- `volee config init` writes a commented `config.lua` (repositories, the hook
  port, the permission mode, editors). `volee config check` validates it.
  Rules for risk and merging go in a `rules.lua`, see the
  [rules reference](../reference/rules.md).
- `volee daemon install` starts the daemon at login (a launchd agent on macOS,
  a systemd user unit on Linux).

Where volee keeps its files (each directory has an override):

| What | Default | Override |
| --- | --- | --- |
| Configuration | `~/.config/volee` | `VOLEE_CONFIG_DIR`, `XDG_CONFIG_HOME` |
| State: database, token, logs | `~/.local/state/volee` | `VOLEE_STATE_DIR`, `XDG_STATE_HOME` |
| Data: worktrees volee created | `~/.local/share/volee` | `VOLEE_DATA_DIR`, `XDG_DATA_HOME` |
| Daemon socket | `<state>/volee.sock` | `VOLEE_SOCKET` |

The overrides let a second copy of volee run beside the first, with its own
`hooks.port` in `config.lua`.

## Shell completions

`volee completions <shell>` prints a completion script for bash, elvish, fish,
PowerShell or zsh. Put it where your shell looks for completions:

```sh
volee completions fish > ~/.config/fish/completions/volee.fish
volee completions zsh > "${fpath[1]}/_volee"      # or any directory on your fpath
volee completions bash > ~/.local/share/bash-completion/completions/volee
```

Run it again after an upgrade, so new commands complete too.

## Update

Run the installer again, or build from source again. Then run `volee daemon
stop` and `volee daemon start`: a daemon keeps running the version that
started it.

## Uninstall

In each repository where you ran `volee doctor`, remove volee's hooks (the
other settings of the file stay):

```sh
volee doctor --remove                         # in the repository
volee doctor --remove --project ~/code/my-repo
volee doctor --user --remove                  # only if you ever used --user
```

Then stop the daemon, remove the login entry if you added it, and remove the
command:

```sh
volee daemon stop
volee daemon uninstall
rm ~/.local/bin/volee          # installed with the installer
cargo uninstall volee-cli      # installed from source
```

Last, remove volee's configuration, state (database, token, logs) and data:

```sh
rm -rf ~/.config/volee ~/.local/state/volee ~/.local/share/volee
```

The worktrees volee created stay in their repositories, under
`.claude/worktrees/`. Before you uninstall, remove the ones whose work landed
with `volee tidy --all`, and any other with `git worktree remove <path>` once
you no longer need its changes. Their branches stay in your repositories
either way.
