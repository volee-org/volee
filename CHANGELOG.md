# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
It is generated from the commit history with [git-cliff](https://git-cliff.org):
see [RELEASING.md](RELEASING.md).

## [0.3.0] - 2026-11-07

### Added

- **cli**: Print shell completion scripts ([#78](https://github.com/volee-org/volee/pull/78))
- **tui**: Forget an agent from the interface ([#83](https://github.com/volee-org/volee/pull/83))
- **daemon**: Run setup commands in a new worktree before its agent starts ([#85](https://github.com/volee-org/volee/pull/85))

### Fixed

- **cli**: Say that gh is not checked yet in volee daemon status ([#81](https://github.com/volee-org/volee/pull/81))
- **mux**: Bring tuios panes forward on the server the daemon reads ([#82](https://github.com/volee-org/volee/pull/82))
- **daemon**: Say a failed launch's cause without a doubled period ([#84](https://github.com/volee-org/volee/pull/84))

### Documentation

- Mark 0.1 and 0.2 done in the roadmap and list what 0.3 brings ([#80](https://github.com/volee-org/volee/pull/80))

## [0.2.0] - 2026-11-07

### Added

- **cli**: Let people forget an agent that no longer runs ([#60](https://github.com/volee-org/volee/pull/60))
- **mcp**: Let agents of other harnesses join volee ([#68](https://github.com/volee-org/volee/pull/68))
- **mux**: Show and reach Claude Code sessions in tuios panes ([#69](https://github.com/volee-org/volee/pull/69))
- **daemon**: Run a failed launch again without a git step ([#70](https://github.com/volee-org/volee/pull/70))

### Fixed

- **tui**: Agree the top bar counters with their number ([#67](https://github.com/volee-org/volee/pull/67))
- **tui**: Make the zoom keys work from the agents list ([#71](https://github.com/volee-org/volee/pull/71))
- **daemon**: Forget the panes of a multiplexer the daemon no longer uses ([#72](https://github.com/volee-org/volee/pull/72))
- **daemon**: Refuse a mission without Claude Code before recording it ([#73](https://github.com/volee-org/volee/pull/73))
- **tui**: Name direct threads from your side and say how to see a tuios pane ([#74](https://github.com/volee-org/volee/pull/74))

## [0.1.0] - 2026-11-07

### Added

- **core**: Add the event-sourced domain model ([#2](https://github.com/volee-org/volee/pull/2))
- **store**: Add the SQLite event log and the hook spool ([#3](https://github.com/volee-org/volee/pull/3))
- **daemon**: Add the daemon, the protocol and the first commands ([#4](https://github.com/volee-org/volee/pull/4))
- **mcp**: Add the MCP server agents use to talk to each other ([#6](https://github.com/volee-org/volee/pull/6))
- **core**: Add the mission supervisor rules ([#9](https://github.com/volee-org/volee/pull/9))
- **config**: Add the configuration file and `volee config` ([#10](https://github.com/volee-org/volee/pull/10))
- **daemon**: Run missions and their supervisor ([#14](https://github.com/volee-org/volee/pull/14))
- **git**: Add repository, worktree and delivery queries ([#12](https://github.com/volee-org/volee/pull/12))
- **harness**: Add the Claude Code adapter ([#11](https://github.com/volee-org/volee/pull/11))
- **daemon**: Discover Claude Code sessions and read their hooks ([#16](https://github.com/volee-org/volee/pull/16))
- **tui**: Add the interface foundation and the hierarchy view ([#17](https://github.com/volee-org/volee/pull/17))
- **daemon**: Keep workspaces and delivery evidence fresh ([#18](https://github.com/volee-org/volee/pull/18))
- **daemon**: Launch agents in their own worktrees ([#19](https://github.com/volee-org/volee/pull/19))
- **daemon**: Resume agents whose session ended to deliver their messages ([#20](https://github.com/volee-org/volee/pull/20))
- **daemon**: Preview, diff, take over and stop agents ([#21](https://github.com/volee-org/volee/pull/21))
- **daemon**: Summarize threads with a model, when the person opts in ([#26](https://github.com/volee-org/volee/pull/26))
- **tui**: Open the interface on the daemon's agents ([#27](https://github.com/volee-org/volee/pull/27))
- **daemon**: Tidy delivered worktrees with their evidence ([#28](https://github.com/volee-org/volee/pull/28))
- **daemon**: List sessions often only while someone watches ([#32](https://github.com/volee-org/volee/pull/32))
- **tui**: Add the network, repositories and timeline views ([#33](https://github.com/volee-org/volee/pull/33))
- **daemon**: Let people turn off background fetches ([#46](https://github.com/volee-org/volee/pull/46))
- **daemon**: Let volee ls ask for a fresh listing of sessions ([#48](https://github.com/volee-org/volee/pull/48))
- **tui**: Add messages, your own tools and launch dialogs ([#39](https://github.com/volee-org/volee/pull/39))
- **daemon**: List sessions less often while nothing changes ([#50](https://github.com/volee-org/volee/pull/50))

### Fixed

- **cli**: Start the daemon without the caller's session identity ([#15](https://github.com/volee-org/volee/pull/15))
- **core**: Never mark an agent lost while it starts a new session ([#23](https://github.com/volee-org/volee/pull/23))
- **core**: Keep a delivered worker delivered while it works again ([#22](https://github.com/volee-org/volee/pull/22))
- **daemon**: Adopt the session of an agent being launched or resumed ([#25](https://github.com/volee-org/volee/pull/25))
- **daemon**: Never resume an agent the human stopped or whose mission ended ([#34](https://github.com/volee-org/volee/pull/34))
- **cli**: Strip control characters from agent text ([#36](https://github.com/volee-org/volee/pull/36))
- **daemon**: Cap the agents a mission launches and match its repositories by path ([#37](https://github.com/volee-org/volee/pull/37))
- **cli**: Print agent names safely in msg, stop, logs, diff and attach ([#40](https://github.com/volee-org/volee/pull/40))
- **daemon**: Replace only a stale socket, never another file ([#41](https://github.com/volee-org/volee/pull/41))
- **daemon**: Launch agents in manual permission mode by default ([#38](https://github.com/volee-org/volee/pull/38))
- **cli**: Print the agent's last screen in volee logs, not its raw output ([#43](https://github.com/volee-org/volee/pull/43))
- **git**: Match pull requests by their head commit and repository ([#42](https://github.com/volee-org/volee/pull/42))
- **daemon**: Tell a resumed agent that its volee tools work again ([#44](https://github.com/volee-org/volee/pull/44))
- **tui**: Keep the demo within the routing volee enforces ([#45](https://github.com/volee-org/volee/pull/45))
- **cli**: Keep bidirectional formatting characters out of agent text ([#49](https://github.com/volee-org/volee/pull/49))
- **daemon**: Say what a failed launch kept and how to launch again ([#51](https://github.com/volee-org/volee/pull/51))
- **daemon**: Refuse a configured repository that is only a folder in another ([#53](https://github.com/volee-org/volee/pull/53))
- **cli**: Show what a waiting agent waits for in volee ls ([#54](https://github.com/volee-org/volee/pull/54))
- **harness**: Require Claude Code 2.1.319, the version volee was measured on ([#55](https://github.com/volee-org/volee/pull/55))
- **daemon**: Accept only a branch name as a launch's base ([#57](https://github.com/volee-org/volee/pull/57))
- **tui**: Never launch on Enter from a multi-line field ([#58](https://github.com/volee-org/volee/pull/58))

### Documentation

- Add the brand assets and polish the README header ([#5](https://github.com/volee-org/volee/pull/5))
- Fix the architecture diagram and the mermaid theme ([#29](https://github.com/volee-org/volee/pull/29))
- Describe what 0.1.0 actually does ([#35](https://github.com/volee-org/volee/pull/35))
- Add a getting started guide ([#24](https://github.com/volee-org/volee/pull/24))
- Present volee 0.1 in the README and the introduction ([#47](https://github.com/volee-org/volee/pull/47))
- Mark the interface, messaging and views milestones done ([#52](https://github.com/volee-org/volee/pull/52))
- Start the daemon in the first step of the guide, and say what tidy removes ([#56](https://github.com/volee-org/volee/pull/56))

[0.3.0]: https://github.com/volee-org/volee/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/volee-org/volee/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/volee-org/volee/releases/tag/v0.1.0
