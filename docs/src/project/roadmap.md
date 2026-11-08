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
| Setup commands | Commands each new worktree runs before its agent starts, such as `bun install` ([ADR 0017](../adr/0017-setup-commands-for-new-worktrees.md)) | Done |
| Forget from the interface | The interface forgets an agent that no longer runs, like `volee forget` | Done |
| Fixes from the 0.2 review | The forge line of `volee daemon status`, focus steps for tuios panes on the daemon's server | Done |

## 0.4: more places agents run

| Milestone | Content | Status |
| --- | --- | --- |
| tmux panes | Claude Code sessions in tmux panes are shown, previewed and reached from the interface, like tuios panes | Done: v0.4.0 |
| Setup in sight | The preview of an agent whose session has not started shows the end of its setup log | Done: v0.4.0 |
| Stalled launches | A launch whose session can never start ends as failed, ready to run again, instead of launching forever | Done: v0.4.1 |

## 0.5 and 0.6: reading an agent's conversation

| Milestone | Content | Status |
| --- | --- | --- |
| Full screen | `z` shows an agent's screen across the whole window, scrolled with `j` and `k`; `a` takes it over from there | Done: v0.5.0 |
| Wide sessions | The preview of a Claude Code session wider than 200 columns replays without garbling, and the end of its conversation stays in view | Done: v0.5.1 |
| Long lines | The full-screen view wraps lines wider than the window; the preview keeps cutting them | Done: v0.6.0 |

## Later

- A location adapter for Zellij (jump to a pane-hosted agent); tuios and tmux
  have one ([ADR 0015](../adr/0015-tuios-location-adapter.md)).
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
