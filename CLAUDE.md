@AGENTS.md

## Claude Code specifics

- Project permissions live in `.claude/settings.json`; put personal overrides
  in `.claude/settings.local.json` (ignored by git).
- Work on a branch in a git worktree, never directly on `main`.
- When you change the TUI, run it once in a real terminal (`cargo run -p
  volee-cli`) in addition to the snapshot tests, and say what you saw.
