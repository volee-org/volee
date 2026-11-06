# 0012. Rust, dual MIT/Apache-2.0, Unix first

- Status: Accepted
- Date: 2026-11-06

## Context

volee is a long-running daemon plus an interactive terminal UI, distributed
to developers who expect a single binary installed in seconds. The prototype
was written in TypeScript on Bun and required the Bun runtime.

## Decision

- **Rust**, edition 2024, with ratatui for the interface, tokio for IO,
  rusqlite (bundled SQLite) for storage and rmcp for the MCP server. The MSRV
  is declared in `Cargo.toml` and tested in CI.
- **License**: MIT OR Apache-2.0, the usual dual license of the Rust
  ecosystem.
- **Platforms**: macOS and Linux. Windows is out of scope for now (Unix
  sockets, harness support); contributions welcome later.
- **Language**: English for code, documentation and interface.

## Consequences

- One static binary per platform, prebuilt by CI, installable with Homebrew,
  a shell installer or Cargo.
- Contributors need a Rust toolchain; `rust-toolchain.toml` pins it.

## Alternatives considered

- **Go with Bubble Tea.** Faster to write, an excellent TUI ecosystem (lazygit,
  k9s). Rust won on control, memory use for the daemon, and the ratatui
  ecosystem around tools volee's users already run (gitui, yazi, zellij).
- **Keep TypeScript.** Requires a JavaScript runtime on every machine.
