# Decisions

volee records its significant design decisions as architecture decision
records (ADRs), in the [MADR](https://adr.github.io/madr/) style. An ADR is
never edited after it is accepted: a later ADR supersedes it.

| ADR | Decision | Status |
| --- | --- | --- |
| [0000](0000-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0001](0001-agent-identity.md) | An agent is a unit of work, not a pane or a process | Accepted |
| [0002](0002-harness-owns-processes.md) | The harness owns agent processes | Superseded by 0019 |
| [0003](0003-daemon-and-event-log.md) | A daemon with an event log is the single source of truth | Accepted |
| [0004](0004-sources-and-reconciliation.md) | Hooks, harness listing, git and forge are reconciled into one model | Accepted |
| [0005](0005-harness-neutral-agent-protocol.md) | Agents talk to volee through MCP and the CLI | Accepted |
| [0006](0006-messaging.md) | Typed, threaded messages delivered through documented hooks | Accepted |
| [0007](0007-missions.md) | Missions are run by a supervisor program and a coordinator agent | Accepted |
| [0008](0008-multiplexers-are-optional.md) | Multiplexers are optional location adapters | Accepted |
| [0009](0009-tui-architecture.md) | The TUI follows the Elm architecture with one keymap table | Accepted |
| [0010](0010-git-access.md) | Git is read through the git CLI behind a trait | Accepted |
| [0011](0011-safety-and-delivery-evidence.md) | Destructive actions require delivery evidence | Accepted |
| [0012](0012-language-license-and-scope.md) | Rust, dual MIT/Apache-2.0, Unix first | Accepted |
| [0013](0013-worktrees-live-in-their-repository.md) | Agent worktrees live in their repository | Accepted |
| [0014](0014-agents-of-other-harnesses-join-through-mcp.md) | Agents of other harnesses join through MCP | Accepted |
| [0015](0015-tuios-location-adapter.md) | tuios is the first location adapter | Accepted |
| [0016](0016-what-a-failed-launch-left-untouched.md) | Reuse or tidy what a failed launch left untouched (refines 0011) | Accepted |
| [0017](0017-setup-commands-for-new-worktrees.md) | Setup commands prepare a new worktree before its agent starts | Accepted |
| [0018](0018-panes-that-show-background-sessions.md) | A pane that shows a background session is its preview | Superseded by 0023 |
| [0019](0019-a-host-process-holds-each-agent-terminal.md) | A volee-host process holds each agent's terminal (supersedes 0002) | Accepted |
| [0020](0020-libghostty-vt-emulates-terminals.md) | libghostty-vt emulates every terminal volee reads | Accepted |
| [0021](0021-take-over-watch-and-the-wall-of-screens.md) | Take over with `a` or `volee attach`, leave with Ctrl+Q, watch on a wall | Accepted |
| [0022](0022-harness-profiles.md) | A profile per harness says how to launch, resume, find and read it | Accepted |
| [0023](0023-every-agent-on-the-machine.md) | volee shows every agent on the machine, and adopts the ones it did not start (supersedes 0018) | Accepted |
| [0024](0024-agents-come-back-after-a-reboot.md) | Agents interrupted by a reboot come back in new hosts | Accepted |

## Not built yet

Some decisions describe more than volee does today. These parts are still
to come:

- [0004](0004-sources-and-reconciliation.md): a setting for how often volee
  lists Claude Code sessions. 0.1.0 lists them every two to ten seconds while
  the interface is open (two after a listing that changed something, slowing
  down while nothing does), and every thirty seconds otherwise.
- [0005](0005-harness-neutral-agent-protocol.md): CLI commands that give
  harnesses without MCP the operations of the MCP tools.
- [0006](0006-messaging.md): configurable routing policies, and a wake
  action for sessions volee did not launch, which cannot receive messages
  yet.
- [0008](0008-multiplexers-are-optional.md): a location adapter for
  Zellij. tuios ([0015](0015-tuios-location-adapter.md)) and tmux have one;
  a session running in a Zellij pane is listed, and you switch to its pane
  yourself.
- [0011](0011-safety-and-delivery-evidence.md): `volee tidy --force`. 0.1.0
  cannot remove a worktree without delivery evidence.
- [0012](0012-language-license-and-scope.md): the Homebrew formula and the
  crates.io release.
- [0020](0020-libghostty-vt-emulates-terminals.md): the interface still draws
  previews and the full-screen view with the `vt100` crate, fed with the
  host's snapshot; hosts and the wall already use
  libghostty-vt.
- [0021](0021-take-over-watch-and-the-wall-of-screens.md): the wall of
  screens is a full-screen view that `5` opens over the interface (it runs
  as `volee host wall`), not a tab of the top bar, and it shows up to nine
  tiles per page whatever the size of the terminal. Ctrl+Q cannot be moved
  to another key yet.
- [0022](0022-harness-profiles.md): profiles for Codex CLI, Gemini CLI,
  Grok Build and the others the ADR lists. Only Claude Code and OpenCode
  (measured on OpenCode 2.2.5) have one; other programs run in a host with
  the generic profile (`volee host spawn`), without a state, a resume or an
  agent in volee's list. ACP as a source of states for hosted agents is
  not built either.
- [0023](0023-every-agent-on-the-machine.md): finding agents in the process
  table for harnesses other than OpenCode (Claude Code sessions are still
  found through `claude agents --json`), and adopting them (`codex resume`,
  `gemini --resume`), which needs their profiles.
- [0024](0024-agents-come-back-after-a-reboot.md): with
  `resume_after_reboot = false`, interrupted agents show as stopped and
  resume when you write to them, rather than offering a resume action.

To propose a new decision, copy [the template](template.md), number it, and
open a pull request.
