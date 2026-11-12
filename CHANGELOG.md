# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
It is generated from the commit history with [git-cliff](https://git-cliff.org):
see [RELEASING.md](RELEASING.md).

## [0.17.0] - 2026-11-12

### Added

- **tui**: Bring the tree to life like the cockpit, add a large zoom and a vim-like status line ([#171](https://github.com/volee-org/volee/pull/171))

### Documentation

- **roadmap**: Record 0.16, the cockpit's main screen ([#170](https://github.com/volee-org/volee/pull/170))

## [0.16.0] - 2026-11-11

### Added

- **tui**: Show the terminal's own background under every theme ([#162](https://github.com/volee-org/volee/pull/162))
- **tui**: Rebuild the main screen after the cockpit: the selected agent's tree, colored cards, For you and a journal ([#164](https://github.com/volee-org/volee/pull/164))
- **daemon**: Show how much of its context an agent uses, from its status line ([#165](https://github.com/volee-org/volee/pull/165))

### Fixed

- **tui**: Say in a window where to go and which key to press to open a session in a terminal ([#161](https://github.com/volee-org/volee/pull/161))
- **mcp**: Tell an agent which row of the agent list is itself ([#163](https://github.com/volee-org/volee/pull/163))
- **tui**: Keep a coordinator's whole name in the list next to the right column ([#166](https://github.com/volee-org/volee/pull/166))
- **core**: Judge a mission's worker only on evidence read since it stopped ([#168](https://github.com/volee-org/volee/pull/168))
- **tui**: Send what is typed in edit mode before the agent's host answers, instead of dropping it ([#169](https://github.com/volee-org/volee/pull/169))

### Documentation

- **roadmap**: Record 0.15, a session typed in any terminal ([#160](https://github.com/volee-org/volee/pull/160))

## [0.15.0] - 2026-11-11

### Added

- **tui**: Open a Claude Code session typed in a terminal once it left it ([#158](https://github.com/volee-org/volee/pull/158))

### Documentation

- **roadmap**: Record what 0.14.1 fixed ([#157](https://github.com/volee-org/volee/pull/157))

## [0.14.1] - 2026-11-11

### Fixed

- **daemon**: Have a lead act on what you write to it through volee ([#155](https://github.com/volee-org/volee/pull/155))

### Documentation

- **roadmap**: Record what 0.14 delivered ([#153](https://github.com/volee-org/volee/pull/153))

## [0.14.0] - 2026-11-11

### Added

- **mcp**: Let an agent ask you a question with the answers to choose from ([#149](https://github.com/volee-org/volee/pull/149))
- **daemon**: Merge a mission's work once you approved its report ([#150](https://github.com/volee-org/volee/pull/150))
- **cli**: Start a lead that launches agents and missions for you ([#151](https://github.com/volee-org/volee/pull/151))

### Documentation

- **roadmap**: Record what 0.13 delivered ([#148](https://github.com/volee-org/volee/pull/148))

## [0.13.0] - 2026-11-11

### Added

- **daemon**: Open an agent of another AI where it runs, in its pane ([#144](https://github.com/volee-org/volee/pull/144))
- **cli**: Run any program as an agent with volee run ([#145](https://github.com/volee-org/volee/pull/145))
- **daemon**: List the agents tuios recognizes in its panes (Codex CLI, Gemini CLI, Aider) ([#146](https://github.com/volee-org/volee/pull/146))

## [0.12.3] - 2026-11-11

### Fixed

- **tui**: Say what the preview and the wall fold of a replay, and show every row full size ([#142](https://github.com/volee-org/volee/pull/142))

## [0.12.2] - 2026-11-11

### Fixed

- **tui**: Keep the screen of an agent at rest up to date in the preview and on the wall ([#137](https://github.com/volee-org/volee/pull/137))
- **tui**: Show the screen of a pane and of a volee host as it is, row for row ([#138](https://github.com/volee-org/volee/pull/138))

### Documentation

- The replay of a background session is its screen, and what volee changes of it ([#136](https://github.com/volee-org/volee/pull/136))

## [0.12.1] - 2026-11-11

### Fixed

- The preview and the wall show an agent's cursor and every row of its screen, as the full-screen view does ([#133](https://github.com/volee-org/volee/pull/133))

## [0.12.0] - 2026-11-11

### Added

- With VOLEE_LOG the interface logs to a file, and says what becomes of each key in edit mode ([#131](https://github.com/volee-org/volee/pull/131))
- The wall shows every agent that runs, the preview is live, and the mouse reaches an opened agent ([#129](https://github.com/volee-org/volee/pull/129))

## [0.11.0] - 2026-11-10

### Added

- Enter opens any agent where it runs, and the list shows what runs ([#127](https://github.com/volee-org/volee/pull/127))

## [0.10.0] - 2026-11-10

### Added

- Edit mode types into an agent inside volee, whose full-screen view is live ([#124](https://github.com/volee-org/volee/pull/124))

## [0.9.1] - 2026-11-09

### Fixed

- Pressing a on a background session adopts it and takes it over, and an untrusted repository asks for trust ([#121](https://github.com/volee-org/volee/pull/121))

## [0.9.0] - 2026-11-09

### Added

- **tui**: **Breaking:** Add which-key popups, a Space leader, vim keys, a command palette and a theme picker; make every key configurable ([#118](https://github.com/volee-org/volee/pull/118))

## [0.8.1] - 2026-11-09

### Fixed

- Adopted agents receive messages, stop and resume like launched ones, and a failed adoption says why ([#116](https://github.com/volee-org/volee/pull/116))

## [0.8.0] - 2026-11-09

### Added

- **Breaking:** Run every agent in a volee host, with exact screens, Ctrl+Q take over, a wall of screens, adoption, resume after reboot and OpenCode agents ([#114](https://github.com/volee-org/volee/pull/114))

## [0.7.0] - 2026-11-08

### Added

- **daemon**: Preview a background session from the pane that shows it ([#112](https://github.com/volee-org/volee/pull/112))

## [0.6.2] - 2026-11-08

### Fixed

- **tui**: Let an independent session's name take the room of its role ([#110](https://github.com/volee-org/volee/pull/110))

### Documentation

- Say where an agent's screen comes from and what a replay can miss ([#109](https://github.com/volee-org/volee/pull/109))

## [0.6.1] - 2026-11-08

### Fixed

- **tui**: Follow relative moves and saved cursors when sizing a preview ([#106](https://github.com/volee-org/volee/pull/106))
- **tui**: Break wrapped full-screen lines between words ([#107](https://github.com/volee-org/volee/pull/107))

### Documentation

- Add the 0.4 releases, 0.5 and 0.6 to the roadmap ([#105](https://github.com/volee-org/volee/pull/105))

## [0.6.0] - 2026-11-08

### Added

- **tui**: Wrap long lines in the full-screen view of an agent ([#103](https://github.com/volee-org/volee/pull/103))

## [0.5.1] - 2026-11-08

### Fixed

- **tui**: Replay wide Claude Code screens without garbling the preview ([#101](https://github.com/volee-org/volee/pull/101))

### Documentation

- Show the full-screen view in the demo recording ([#100](https://github.com/volee-org/volee/pull/100))

## [0.5.0] - 2026-11-08

### Added

- **tui**: Show an agent screen full size ([#98](https://github.com/volee-org/volee/pull/98))

### Fixed

- **tui**: Let a launch go on when its dialog closes, and say how it went ([#97](https://github.com/volee-org/volee/pull/97))

## [0.4.1] - 2026-11-08

### Fixed

- **daemon**: Fail a launch that can never start its session ([#93](https://github.com/volee-org/volee/pull/93))
- **tui**: Say that a session started outside volee reports no activity ([#95](https://github.com/volee-org/volee/pull/95))

### Documentation

- Add setup recipes and say what package-manager scripts change ([#94](https://github.com/volee-org/volee/pull/94))

## [0.4.0] - 2026-11-07

### Added

- **mux**: Show and reach Claude Code sessions in tmux panes ([#87](https://github.com/volee-org/volee/pull/87))
- **daemon**: Show the setup log as the preview of an agent with no session yet ([#91](https://github.com/volee-org/volee/pull/91))

### Fixed

- **cli**: Align volee config check on the longest repository name ([#88](https://github.com/volee-org/volee/pull/88))
- **mux**: Say that no tmux server is running in volee daemon status ([#89](https://github.com/volee-org/volee/pull/89))

### Documentation

- Present setup commands and tuios and tmux panes in the README ([#90](https://github.com/volee-org/volee/pull/90))

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

[0.17.0]: https://github.com/volee-org/volee/compare/v0.16.0...v0.17.0
[0.16.0]: https://github.com/volee-org/volee/compare/v0.15.0...v0.16.0
[0.15.0]: https://github.com/volee-org/volee/compare/v0.14.1...v0.15.0
[0.14.1]: https://github.com/volee-org/volee/compare/v0.14.0...v0.14.1
[0.14.0]: https://github.com/volee-org/volee/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/volee-org/volee/compare/v0.12.3...v0.13.0
[0.12.3]: https://github.com/volee-org/volee/compare/v0.12.2...v0.12.3
[0.12.2]: https://github.com/volee-org/volee/compare/v0.12.1...v0.12.2
[0.12.1]: https://github.com/volee-org/volee/compare/v0.12.0...v0.12.1
[0.12.0]: https://github.com/volee-org/volee/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/volee-org/volee/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/volee-org/volee/compare/v0.9.1...v0.10.0
[0.9.1]: https://github.com/volee-org/volee/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/volee-org/volee/compare/v0.8.1...v0.9.0
[0.8.1]: https://github.com/volee-org/volee/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/volee-org/volee/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/volee-org/volee/compare/v0.6.2...v0.7.0
[0.6.2]: https://github.com/volee-org/volee/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/volee-org/volee/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/volee-org/volee/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/volee-org/volee/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/volee-org/volee/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/volee-org/volee/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/volee-org/volee/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/volee-org/volee/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/volee-org/volee/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/volee-org/volee/releases/tag/v0.1.0
