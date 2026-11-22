@AGENTS.md

## Claude Code specifics

- Project permissions live in `.claude/settings.json`; put personal overrides
  in `.claude/settings.local.json` (ignored by git).
- Work on a branch in a git worktree, never directly on `main`.
- When you change the web interface, run it once for real: build `ui/`, start
  an isolated daemon (own config, state and data directories, own hook port),
  open the page and say what you saw, in addition to the Vitest and Rust tests.
- Never touch `~/.claude/settings.json` or `~/.claude.json`: trials use
  `--settings` with an isolated daemon.
