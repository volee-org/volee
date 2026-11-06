# 0004. Hooks, harness listing, git and forge are reconciled into one model

- Status: Accepted
- Date: 2026-11-06

## Context

No single source knows everything:

- Hooks give rich, immediate events, but only for sessions started with
  volee's settings.
- `claude agents --json --all` lists every Claude Code session on the machine
  with a coarse state, and is documented as the supported way to read it.
- Git knows worktrees, branches, commits and push state.
- The forge knows pull requests and checks.

Claude Code also keeps internal files (`~/.claude/jobs`, `~/.claude/sessions`,
transcripts) whose format is explicitly not a stable interface.

## Decision

- Each source is authoritative for one kind of fact (see the architecture
  overview). Conflicts are resolved by that rule, not by recency.
- Hooks are injected per agent with `claude --settings <file>`. volee never
  edits the user's `~/.claude/settings.json`; hooks passed this way are added
  to the user's own hooks.
- `claude agents --json --all` is polled (every few seconds while a client is
  connected, slower otherwise) and reconciled by session id.
- volee does not parse Claude Code internal files. Transcript paths received
  in hook payloads may be shown, never parsed for state.
- Git and forge queries are triggered by events (an edit, a commit, a push
  seen in a hook) and by a slow timer.

## Consequences

- Agents volee did not launch are visible with a coarse state; agents it
  launched get the fine-grained one.
- volee keeps working when Claude Code changes its internal files.
- Polling `claude agents` costs a process start every few seconds while the
  TUI is open; this is acceptable and configurable.

## Alternatives considered

- **Parse `~/.claude/sessions/*.json` and transcripts.** Cheaper and richer,
  but undocumented and unstable.
- **Global hooks in the user's settings.** Would see every session in detail,
  but edits a file the user owns and affects every session they start.
