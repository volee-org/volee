# Roadmap

volee is young. This page lists what exists, what comes next and what is
deliberately out of scope. Dates are targets, not promises.

## 0.1: see everything, launch and talk

| Milestone | Content | Status |
| --- | --- | --- |
| Bootstrap | Workspace, CI, documentation site, contributor guides, agent rules | Done |
| Foundation | Domain model, SQLite event store, daemon with JSON-RPC over a Unix socket, `volee ls`, discovery of every Claude Code session | Done |
| Launch and observe | `volee spawn` (worktree + `claude --bg` + per-agent hooks), hook ingestion, git delivery state, `volee diff` | Done |
| Interface | Hierarchy view, preview, take over with `claude attach`, messages and composer, changes, your own tools, launch dialogs, keymap and help, themes, semantic zoom | In progress: `volee` runs on the daemon's agents, with previews, changes, take over, messages, tools and launches |
| Messaging | MCP server for agents, typed threads, delivery to working, idle and stopped agents, inbox, message pulses on the graph | In progress: MCP server, threads and delivery are done |
| Missions | Supervisor plus coordinator agent, change and analysis missions, delivery evidence, `volee tidy` | Done |
| More views | Network, repositories and worktrees, timeline | Done: the three views run on `volee demo`, message pulses included |
| Release | Prebuilt binaries for macOS and Linux, shell installer, demo recordings | Planned |

## Later

- Location adapters for tmux, Zellij and tuios (jump to a pane-hosted agent).
- Other harnesses: Codex CLI, Gemini CLI (discovery and messaging first).
- Setup commands for new worktrees: a first step, such as `bun install`,
  that each new worktree runs.
- Leads that start missions and workers themselves.
- Adopting and messaging sessions started outside volee.
- Opening a diff or a worktree in your editor and other tools you configure.
- A Homebrew tap and a crates.io release.

## Not planned

- Owning agent processes or terminals (see [ADR 0002](../adr/0002-harness-owns-processes.md)).
- Windows support, until the harnesses and Unix-socket replacements make it
  reasonable.
- A web or desktop interface.
