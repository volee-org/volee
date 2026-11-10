# Summary

[Introduction](introduction.md)

# User guide

- [Installation](guide/installation.md)
- [Getting started](guide/getting-started.md)
- [Concepts](guide/concepts.md)
- [The interface](guide/interface.md)
- [Themes](guide/themes.md)
- [Command line](guide/cli.md)
- [Configuration](guide/configuration.md)
- [Agents of other harnesses](guide/other-harnesses.md)

# Reference

- [Daemon protocol](reference/protocol.md)
- [MCP tools for agents](reference/mcp.md)

# Architecture

- [Overview](architecture/overview.md)
- [Domain model](architecture/domain.md)
- [Hosts](architecture/hosts.md)
- [Claude Code adapter](architecture/claude-code.md)
- [Git](architecture/git.md)
- [Decisions](adr/README.md)
  - [0000. Record architecture decisions](adr/0000-record-architecture-decisions.md)
  - [0001. Agent identity](adr/0001-agent-identity.md)
  - [0002. The harness owns processes](adr/0002-harness-owns-processes.md)
  - [0003. Daemon and event log](adr/0003-daemon-and-event-log.md)
  - [0004. Sources and reconciliation](adr/0004-sources-and-reconciliation.md)
  - [0005. Harness-neutral agent protocol](adr/0005-harness-neutral-agent-protocol.md)
  - [0006. Messaging](adr/0006-messaging.md)
  - [0007. Missions](adr/0007-missions.md)
  - [0008. Multiplexers are optional](adr/0008-multiplexers-are-optional.md)
  - [0009. TUI architecture](adr/0009-tui-architecture.md)
  - [0010. Git access](adr/0010-git-access.md)
  - [0011. Safety and delivery evidence](adr/0011-safety-and-delivery-evidence.md)
  - [0012. Language, license and scope](adr/0012-language-license-and-scope.md)
  - [0013. Agent worktrees live in their repository](adr/0013-worktrees-live-in-their-repository.md)
  - [0014. Agents of other harnesses join through MCP](adr/0014-agents-of-other-harnesses-join-through-mcp.md)
  - [0015. tuios is the first location adapter](adr/0015-tuios-location-adapter.md)
  - [0016. Reuse or tidy what a failed launch left untouched](adr/0016-what-a-failed-launch-left-untouched.md)
  - [0017. Setup commands prepare a new worktree before its agent starts](adr/0017-setup-commands-for-new-worktrees.md)
  - [0018. A pane that shows a background session is its preview](adr/0018-panes-that-show-background-sessions.md)
  - [0019. A host process holds each agent's terminal](adr/0019-a-host-process-holds-each-agent-terminal.md)
  - [0020. libghostty-vt emulates terminals](adr/0020-libghostty-vt-emulates-terminals.md)
  - [0021. Take over, watch, and the wall of screens](adr/0021-take-over-watch-and-the-wall-of-screens.md)
  - [0022. Harness profiles](adr/0022-harness-profiles.md)
  - [0023. Every agent on the machine](adr/0023-every-agent-on-the-machine.md)
  - [0024. Agents come back after a reboot](adr/0024-agents-come-back-after-a-reboot.md)
  - [0025. Key sequences, a leader and which-key popups](adr/0025-key-sequences-a-leader-and-which-key-popups.md)
  - [0026. Edit mode inside volee](adr/0026-edit-mode-types-into-an-agent-inside-volee.md)
  - [0027. Open without owning](adr/0027-open-without-owning.md)
  - [Template](adr/template.md)

# Project

- [Roadmap](project/roadmap.md)
- [Contributing](project/contributing.md)
