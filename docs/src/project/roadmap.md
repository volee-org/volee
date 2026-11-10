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

## 0.8: a host for every agent

| Milestone | Content | Status |
| --- | --- | --- |
| Hosts | Every agent volee launches runs in a volee host that holds its terminal and its exact screen, emulated by libghostty-vt, and survives the interface, the daemon and the terminal that started it ([ADR 0019](../adr/0019-a-host-process-holds-each-agent-terminal.md), [ADR 0020](../adr/0020-libghostty-vt-emulates-terminals.md)) | Done |
| Take over and watch | `a` and `volee attach` take an agent over from any terminal, `Ctrl+Q` comes back; the preview and `z` show the exact screen without resizing the agent ([ADR 0021](../adr/0021-take-over-watch-and-the-wall-of-screens.md)) | Done |
| Wall of screens | `5` shows the hosted agents of the list as tiles of live screens; `volee host wall` opens it from a shell | Done |
| OpenCode | `volee spawn --harness opencode` and the launch dialog launch OpenCode agents in hosts, follow their state from their screen and resume them ([ADR 0022](../adr/0022-harness-profiles.md)) | Done |
| Every agent on the machine | OpenCode agents started elsewhere are found in the process table; `A` or `volee adopt` moves an agent volee does not host into a host ([ADR 0023](../adr/0023-every-agent-on-the-machine.md)) | Done |
| Back after a reboot | The daemon resumes the agents a reboot interrupted, in new hosts; `volee daemon install` starts it at login ([ADR 0024](../adr/0024-agents-come-back-after-a-reboot.md)) | Done |
| Hosts by hand | `volee host spawn`, `ls`, `peek`, `attach`, `stop` and `wall` run any command line in a host, without the daemon | Done |
| Mirrors removed | The 0.7.0 previews of background sessions read from tuios panes by their title are gone; tuios and tmux still show the sessions started by hand in their panes | Done |
| Release | v0.8.0 | Done: v0.8.0 |

## 0.9 and 0.10: keys, and typing inside volee

| Milestone | Content | Status |
| --- | --- | --- |
| Keys | A popup of the keys of each screen, a leader key, vim keys, every key configurable, a theme picker ([ADR 0025](../adr/0025-key-sequences-a-leader-and-which-key-popups.md)) | Done: v0.9.0 |
| Edit mode | The full-screen view of an agent in a volee host is live, and typing goes to the agent inside volee's frame until `Ctrl+Q` ([ADR 0026](../adr/0026-edit-mode-types-into-an-agent-inside-volee.md)) | Done: v0.10.0 |

## 0.11: open every agent where it runs

An audit on 2026-11-10 found that volee owned the agents it showed, where it
should plug into them: typing into a session volee did not start needed an
adoption, and the list kept sessions that were gone. The plan that followed
has five steps, each released only once a person can make its trip by
keyboard ([journeys](https://github.com/volee-org/volee/tree/main/journeys)).

| Milestone | Content | Status |
| --- | --- | --- |
| Journeys | Scripts that drive the real interface by keyboard with a real agent are the gate of every release | Done |
| The list is what runs | One row per session the harness lists, finished background sessions included; a session that is gone leaves the list; a one-off run of a few seconds never shows ([ADR 0027](../adr/0027-open-without-owning.md)) | Done |
| Open without owning | `enter`, a double click or `i` opens any agent where it runs, inside volee: `claude attach` on a background session, `tuios pane-view` on a tuios pane; nothing is stopped or restarted ([ADR 0027](../adr/0027-open-without-owning.md)) | Done |
| Release | v0.11.0 | Next |

## Next

In this order, one release per step:

- One terminal component for the preview, the full-screen view, the wall
  and edit mode, emulated by libghostty-vt everywhere, with the mouse
  forwarded to an opened agent.
- Other harnesses: finding their agents wherever they run, a door on each,
  and `volee run -- <command>` to start any program in a host.
- Missions as a lead agent runs them, with the rules that make a lead a
  good one.

## Later

- A location adapter for Zellij (jump to a pane-hosted agent); tuios and tmux
  have one ([ADR 0015](../adr/0015-tuios-location-adapter.md)).
- Profiles for Codex CLI, Gemini CLI, Grok Build and others, each measured on
  the real program ([ADR 0022](../adr/0022-harness-profiles.md)). Until
  then, `volee host spawn` runs any of them in a host, and agents of any
  MCP-capable harness can [join](../guide/other-harnesses.md) through
  `volee mcp --join`.
- Finding agents of harnesses other than OpenCode in the process table.
- Leads that start missions and workers themselves.
- Messaging sessions started outside volee.
- A Homebrew tap and a crates.io release.

## Not planned

- Running agents inside tmux or another multiplexer: hosts hold their
  terminals ([ADR 0019](../adr/0019-a-host-process-holds-each-agent-terminal.md)).
- Windows support, until the harnesses and Unix-socket replacements make it
  reasonable.
- A web or desktop interface.
