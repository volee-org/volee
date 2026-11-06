# 0011. Destructive actions require delivery evidence

- Status: Accepted
- Date: 2026-11-06

## Context

The prototype's cleanup removed a worktree as soon as its multiplexer session
had disappeared, which happens when the last window closes or while the
multiplexer restarts. It could also kill a whole shared session because one
agent in it had been adopted.

## Decision

- **Delivery evidence** is computed by code from git and the forge: commits
  since the agent started, branch pushed (tip present on the remote), pull
  request opened, merged or closed, checks state; for analysis work, a report
  message.
- `volee tidy` removes a worktree only when its work is delivered (merged into
  the base, or pull request merged, or report delivered) **and** the tree is
  clean. Otherwise it explains what is missing. `--force` is explicit and
  never the default.
- volee never deletes a branch.
- volee stops only sessions it launched, never a pane or a session it merely
  observes.
- An agent that disappears is marked **lost** with the evidence of its
  disappearance, never "done".

## Consequences

- Cleanup is slower to reach "all tidy", and always safe to run.
- Users see exactly why something is kept.

## Alternatives considered

- **Trust absence as completion** (the prototype). Lost work during restarts.
