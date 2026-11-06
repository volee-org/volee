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

To propose a new decision, copy [the template](template.md), number it, and
open a pull request.
