# Roadmap

volee is young. This page lists what exists, what comes next and what is
deliberately out of scope. Dates are targets, not promises.

## 0.1: see everything, launch and talk

| Milestone | Content | Status |
| --- | --- | --- |
| Bootstrap | Workspace, CI, documentation site, contributor guides, agent rules | In progress |
| Foundation | Domain model, SQLite event store, daemon with JSON-RPC over a Unix socket, `volee ls`, discovery of every Claude Code session | Planned |
| Launch and observe | `volee spawn` (worktree + `claude --bg` + per-agent hooks), hook ingestion, git delivery state, `volee diff` | Planned |
| Interface | Hierarchy view, preview, take over with `claude attach`, keymap and help, themes, semantic zoom | Planned |
| Messaging | MCP server for agents, typed threads, delivery to working, idle and stopped agents, inbox, message pulses on the graph | Planned |
| Missions | Supervisor plus coordinator agent, change and analysis missions, delivery evidence, `volee tidy` | Planned |
| More views | Network, repositories and worktrees, timeline | Planned |
| Release | Prebuilt binaries for macOS and Linux, shell installer, demo recordings | Planned |

## Later

- Location adapters for tmux, Zellij and tuios (jump to a pane-hosted agent).
- Other harnesses: Codex CLI, Gemini CLI (discovery and messaging first).
- Thread summaries by an LLM (opt-in).
- A Homebrew tap and a crates.io release.

## Not planned

- Owning agent processes or terminals (see [ADR 0002](../adr/0002-harness-owns-processes.md)).
- Windows support, until the harnesses and Unix-socket replacements make it
  reasonable.
- A web or desktop interface.
