# Decisions

volee records its significant design decisions as architecture decision
records (ADRs), in the [MADR](https://adr.github.io/madr/) style. An ADR is
never edited after it is accepted: a later ADR supersedes it.

| ADR | Decision | Status |
| --- | --- | --- |
| [0000](0000-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0001](0001-agent-identity.md) | An agent is a unit of work, not a pane or a process | Accepted |
| [0002](0002-harness-owns-processes.md) | The harness owns agent processes | Accepted |
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

## Not built yet

Some decisions describe more than volee 0.1.0 does. These parts are still to
come:

- [0004](0004-sources-and-reconciliation.md): a setting for how often volee
  lists Claude Code sessions. 0.1.0 lists them every two to ten seconds while
  the interface is open (two after a listing that changed something, slowing
  down while nothing does), and every thirty seconds otherwise.
- [0005](0005-harness-neutral-agent-protocol.md): CLI commands that give
  harnesses without MCP the operations of the MCP tools.
- [0006](0006-messaging.md): configurable routing policies, and a wake
  action for sessions volee did not launch, which cannot receive messages
  yet.
- [0008](0008-multiplexers-are-optional.md): location adapters. 0.1.0 has
  none: a session running in a pane is listed, and you switch to its pane
  yourself.
- [0011](0011-safety-and-delivery-evidence.md): `volee tidy --force`. 0.1.0
  cannot remove a worktree without delivery evidence.
- [0012](0012-language-license-and-scope.md): the Homebrew formula and the
  crates.io release.

To propose a new decision, copy [the template](template.md), number it, and
open a pull request.
