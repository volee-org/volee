# Installation

## Requirements

- macOS or Linux.
- [Claude Code](https://code.claude.com) 2.1.300 or later, for the agent
  features (background sessions and `claude agents --json`).
- git 2.40 or later.
- Optional: the GitHub CLI (`gh`), to show pull requests and checks.

## From source

Prebuilt binaries arrive with the first release. Until then, install with
Cargo (a recent stable Rust toolchain is required):

```sh
cargo install --git https://github.com/volee-org/volee volee-cli
```

This installs the `volee` command into `~/.cargo/bin`.

## Check the installation

```sh
volee --version
```

## Uninstall

```sh
cargo uninstall volee-cli
rm -rf ~/.config/volee ~/.local/state/volee ~/.local/share/volee
```

Removing `~/.local/share/volee` also removes the worktrees volee created. Their
branches stay in your repositories.
