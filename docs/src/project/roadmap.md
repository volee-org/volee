# Roadmap

volee is young. This page lists what exists, what comes next and what is
deliberately out of scope. Dates are targets, not promises.

## 0.1: see everything, launch and talk

| Milestone | Content | Status |
| --- | --- | --- |
| Bootstrap | Workspace, CI, documentation site, contributor guides, agent rules | Done |
| Foundation | Domain model, SQLite event store, daemon with JSON-RPC over a Unix socket, `volee ls`, discovery of every Claude Code session | Done |
| Launch and observe | `volee spawn` (worktree + `claude --bg` + per-agent hooks), hook ingestion, git delivery state, `volee diff` | Done |
| Interface | Hierarchy view, preview, take over with `claude attach`, messages and composer, changes, your own tools, launch dialogs, keymap and help, themes, semantic zoom | Done |
| Messaging | MCP server for agents, typed threads, delivery to working, idle and stopped agents, inbox, message pulses on the graph | Done |
| Missions | Supervisor plus coordinator agent, change and analysis missions, delivery evidence, `volee tidy` | Done |
| More views | Network, repositories and worktrees, timeline | Done |
| Release | Prebuilt binaries for macOS and Linux, shell installer, demo recordings | Done: v0.1.0 |

## 0.2: other harnesses and tuios panes

| Milestone | Content | Status |
| --- | --- | --- |
| Other harnesses | Agents of any MCP-capable harness join volee and its missions with `volee mcp --join` ([ADR 0014](../adr/0014-agents-of-other-harnesses-join-through-mcp.md)) | Done |
| tuios panes | Claude Code sessions in tuios panes are shown, previewed and reached from the interface ([ADR 0015](../adr/0015-tuios-location-adapter.md)) | Done |
| Launch recovery | A failed launch runs again without a git step ([ADR 0016](../adr/0016-what-a-failed-launch-left-untouched.md)); `volee forget` removes an agent that no longer runs | Done |
| Release | v0.2.0 | Done: v0.2.0 |

## 0.3: setting up and tidying

| Milestone | Content | Status |
| --- | --- | --- |
| Shell completions | `volee completions <shell>` prints a completion script | Done |
| Setup commands | Commands each new worktree runs before its agent starts, such as `bun install` | In progress |
| Forget from the interface | The interface forgets an agent that no longer runs, like `volee forget` | In progress |
| Fixes from the 0.2 review | The forge line of `volee daemon status`, focus steps for tuios panes on the daemon's server | In progress |

## Later

- Location adapters for tmux and Zellij (jump to a pane-hosted agent); tuios
  has one ([ADR 0015](../adr/0015-tuios-location-adapter.md)).
- Other harnesses: Codex CLI, Gemini CLI (discovery and messaging first).
  Agents of any MCP-capable harness can already
  [join](../guide/other-harnesses.md) through `volee mcp --join`; discovering
  their sessions and handing them messages would come with adapters.
- Leads that start missions and workers themselves.
- Adopting and messaging sessions started outside volee.
- A Homebrew tap and a crates.io release.

## Not planned

- Owning agent processes or terminals (see [ADR 0002](../adr/0002-harness-owns-processes.md)).
- Windows support, until the harnesses and Unix-socket replacements make it
  reasonable.
- A web or desktop interface.
