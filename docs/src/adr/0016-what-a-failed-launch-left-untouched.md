# 0016. Reuse or tidy what a failed launch left untouched

- Status: Accepted
- Date: 2026-11-07
- Refines: [0011](0011-safety-and-delivery-evidence.md)

## Context

`volee spawn` creates the agent's worktree and its new branch, then starts
the session. The most common first-run failure comes after that: Claude Code
refuses a background session in a repository it does not trust yet. Both
the worktree and the branch stay, because volee deletes nothing without
evidence and never deletes a branch (ADR 0011). The person trusts the
repository and runs the same command again, which is refused: the branch
exists. They had to remove the worktree and the branch with git first.

What such a failure leaves holds no work: the agent never ran, so nothing
was written in the worktree and nothing was committed on the branch. That
is evidence too, and it can be checked.

## Decision

- A launch on a branch that exists **reuses** what a failed launch left,
  only when all of these hold:
  - an agent volee launched on that branch, in the worktree this launch
    would use (`<repository>/.claude/worktrees/<branch>`), from the same
    base, **failed without ever having a session**;
  - git registers that worktree for that branch, unlocked;
  - the worktree is **clean**: no change and no untracked file;
  - the branch holds **no commit of its own**: none beyond its merge base
    with the base branch.

  The failed agent is then forgotten, and the launch goes on as usual in
  that worktree, from the branch's tip. In every other case the existing
  refusal stands: the branch or the directory exists.
- The check runs again, under the state lock, right before the failed agent
  is forgotten, so two launches cannot claim the same worktree.
- `volee tidy` accepts the same evidence for removing such a worktree: an
  agent whose launch failed before any session started, with fresh delivery
  evidence showing a clean tree and no commit of its own. git still refuses
  any change or untracked file when it removes the worktree. The branch is
  kept.
- The message of a failed launch says that the same command can simply run
  again once the cause is fixed.

ADR 0011 stands otherwise: volee never deletes a branch, and removes or
reuses nothing that could hold work.

## Consequences

- Trusting the repository and running the same `volee spawn` again works,
  without a git step. So does a retry after any other launch failure that
  happened before the session started.
- A worktree in which anything happened (a file written, a commit, a
  session started) is never reused or tidied by this rule.
- A failed agent that the person forgot (`volee forget`) leaves nothing
  that proves where the worktree came from: the branch is refused, as
  before. A Claude Code worktree of the same name is never taken for volee's
  leftover.
- The reused branch starts where the failed launch started it. If the base
  moved since, the new agent starts from the older commit; it can rebase.

## Alternatives considered

- **Roll back a failed launch, branch included.** Leaves nothing behind, but
  needs an exception to "volee never deletes a branch", and a rollback that
  fails halfway leaves the same state anyway.
- **Accept any existing branch that holds no commit of its own.** Simpler,
  but a branch someone created by hand, or a worktree Claude Code made, would
  be taken over. Requiring the failed agent ties the reuse to volee's own
  leftover.
- **Ask the person to run `volee tidy` first.** Safe, but it is one more
  step for the most common first-run failure, and the person would still
  have to know about it.
