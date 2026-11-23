# Roadmap

volee v2 is a restart on the `v2` branch of the same repository: a control
tower that observes Claude Code instead of owning terminals. The reasons, the
plan and the trials that checked what Claude Code allows are in
`docs/v2/framing.md` and the write-ups next to it (`rules-trial.md`,
`pre-review.md`, `actions-trial.md`). The decisions are
[ADRs 0036 to 0041](../adr/README.md). Nothing here is published yet: the latest
release, 0.18, is the old tool, and `main` still ships it.

## Done

Each step ended with something to demo and a proof run for real, with real
Claude Code sessions in a sandbox repository.

| Step | What it gave |
| --- | --- |
| 0 | Clean slate: the terminal-owning crates, the TUI and its journeys removed; the old ADRs kept as history and marked superseded |
| 1 | The hook server: the daemon on `127.0.0.1` with a token, every Claude Code `http` hook stored in SQLite, `volee doctor` writing the hook settings |
| 2 | Agents and finished work: `claude agents --json` joined to hooks and git facts, `volee agents` |
| 3 | The web interface, a Svelte app embedded in the binary, live over server-sent events, Rex adapter for "open this terminal" |
| 4 | Permission cards: held `PermissionRequest` hooks, allow, allow with a rule, deny, stale answers reconciled |
| 5 | Question cards: `AskUserQuestion` answered through `PreToolUse`, hold delay measured |
| 6 | The finished-work screen: summary, plan, open points, files and why, diagram, guided diff, from git and a tolerant transcript reader |
| 7 | The pre-review: Claude Code's `/code-review` as an opinion, inline in the diff |
| 8 | Lua: sandboxed `config.lua` and `rules.lua`, risk, layers and review plan from the rules, a broken file falls back to the safest rule |
| 9 | Actions: merge, automatic merge (opt-in per repository), ask for a fix, comment a line, open in your editor, the journal |

## Before v2.0.0

Step 10, the release, is under way:

- **Documentation**: this site, rebuilt around the inbox.
- **Installer**: an installer tested from a local artifact, and the release
  workflow producing the binary with the interface embedded.
- **Journeys**: the release gate. The 0.x journeys drove the TUI and were
  removed; the new ones are browser journeys with a real Claude Code and the
  real interface driven by Playwright (see `journeys/README.md`).
- **A day of use**: one real day of work with volee before the tag.
- **The tag and the merge into `main`** wait for the maintainer's explicit go. No
  tag, release, package or docs site update happens before.

Cleanup that goes with it:

- The `config.lua` keys of 0.x that nothing read are removed
  ([Configuration](../guide/configuration.md#keys-of-0x-that-are-gone)). The 0.x
  registry, the MCP server (`ask_human`) and the `msg`, `mission`, `summary`,
  `ls` and `hook` commands stay as they are, documented as carried over; what
  the inbox does not use of them may go later.
- Show the `ask_human` questions in the web inbox, next to the questions Claude
  Code asks through its hooks.

## Later

Not in v2.0.0, and each needs a decision first:

- **The Agent SDK as an opt-in module** for sessions volee itself would start.
  It works with the subscription and answers permissions and questions, but its
  sessions do not appear in `claude agents --json` and the terms for a
  third-party tool are to be confirmed. It must never offer or handle login.
- **More terminal adapters** (tmux, `claude attach`) beside Rex. They only
  locate and focus; they never read or type.
- **A desktop shell** (Tauri) around the same page. The interface must not depend
  on it.
- **An agent graph** and messages between agents, if the inbox shows a need.
- **Review by another model** when `/code-review` alone is not enough. Open Code
  Review was tried in delegation mode and gave no net gain at equal model.

## Not planned

- Launching agents, holding their terminals or typing into them. volee observes.
- A terminal interface. The page is where reading a diff, a plan or a schema
  belongs.
- Merging when nobody decided, in a repository you did not list, or a high risk
  change.

## 0.x, for the record

volee 0.1 to 0.18 (November 2026) was a terminal tool in Rust: a ratatui
interface, one host process per agent holding its terminal, an embedded terminal
emulator, a wall of screens, missions run by a lead, messages between agents.
Judged badly designed for three reasons (it owned terminals, it redid what
Claude Code now provides, and it put in a terminal what needs a graphical
page), it was replaced by v2. Its decisions are in the
[ADR list](../adr/README.md); its releases stay on GitHub.
