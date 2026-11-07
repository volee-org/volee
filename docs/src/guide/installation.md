# Installation

## Requirements

- macOS (Apple Silicon or Intel) or Linux (x86_64 or ARM64).
- [Claude Code](https://code.claude.com) 2.1.319 or later, for the agent
  features (background sessions, `claude agents --json`, permission modes).
  `claude update` brings it up to date.
- git 2.40 or later.
- Optional: the [GitHub CLI](https://cli.github.com) (`gh`), signed in with
  `gh auth login`. With it, volee shows the pull request opened from each
  agent's branch and the state of its checks, and the mission supervisor can
  count an open or merged pull request as delivered work. Without it,
  everything else works and `volee daemon status` says why pull requests are
  not shown. volee uses your own sign-in and never sees a token.

## With the installer

The latest release of volee is **v0.2.0**. Every release on
[GitHub](https://github.com/volee-org/volee/releases) comes with prebuilt
binaries and an installer script. To install the latest release:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/volee-org/volee/releases/latest/download/volee-cli-installer.sh | sh
```

The script picks the archive for your machine, checks its SHA-256 checksum and
installs the `volee` command into `~/.local/bin` (or `$XDG_BIN_HOME` when it
is set). If that directory is not on your `PATH`, the script adds it to your
shell profile: open a new terminal afterwards.

To install a specific version, put its tag in the URL:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/volee-org/volee/releases/download/v0.1.0/volee-cli-installer.sh | sh
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

Install with Cargo (Rust 1.89 or later):

```sh
cargo install --locked --git https://github.com/volee-org/volee --tag v0.2.0 volee-cli
```

Replace `v0.2.0` with the release you want, or leave `--tag` out to build the
latest commit of `main`. The `volee` command goes into `~/.cargo/bin`.

## Check the installation

```sh
volee --version
```

## Update

Run the installer again, or the `cargo install` command with the new tag.

## Uninstall

Remove the `volee` command the way you installed it:

```sh
rm ~/.local/bin/volee          # installed with the installer
cargo uninstall volee-cli      # installed with Cargo
```

Then remove volee's configuration and state:

```sh
rm -rf ~/.config/volee ~/.local/state/volee
```

The worktrees volee created stay in their repositories, under
`.claude/worktrees/`. Before you uninstall, remove the ones whose work landed
with `volee tidy --all`, and any other with `git worktree remove <path>` once
you no longer need its changes. Their branches stay in your repositories
either way.
