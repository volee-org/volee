# 0013. Agent worktrees live in their repository

- Status: Accepted
- Date: 2026-11-07

## Context

volee gives every worker its own git worktree, on its own branch, so that
agents never edit the same files. The first design put worktrees under
volee's data directory (`~/.local/share/volee/worktrees/<repo>/<branch>`) and
started each agent there with `claude --bg`.

Claude Code refuses to start a background session in a directory the user has
not trusted. We measured it with Claude Code 2.1.319, with the clean
environment the daemon uses:

- Trust is recorded per path. A repository nested in a trusted folder does not
  inherit its trust (since 2.1.232).
- `claude --bg` in an untrusted directory prints `Workspace not trusted. Run
  claude in <dir> once and accept the trust prompt, then retry.` and starts
  nothing. A worktree created with `git worktree add` is untrusted wherever it
  lives, even under a trusted repository's `.claude/worktrees/`.
- From a trusted repository's main checkout, `claude --bg --worktree=<name>`
  works. When `.claude/worktrees/<name>` already exists, Claude Code reuses
  it, on whatever branch it has checked out, and runs the session there.
- Claude Code locks the worktrees it runs in. `claude rm` deletes a worktree
  it created itself, but keeps a pre-existing one whose commits it cannot see
  on a remote (`kept <id>`).

Making volee's own worktrees trusted would mean editing Claude Code's
configuration file, which volee never does (see the agent rules).

## Decision

- volee creates each worktree itself, with git, at
  `<repository>/.claude/worktrees/<branch with / replaced by ->`, on the
  agent's branch from the chosen base (ADR 0010). It adds `.claude/worktrees/`
  to the repository's local exclude file, as Claude Code does.
- The session starts from the repository's main checkout with
  `--worktree=<name>`, so Claude Code checks trust where the user gave it and
  reuses volee's worktree.
- The location is not configurable. A repository volee launches agents in must
  be trusted once by the user (run `claude` there and accept). volee reports
  an untrusted repository with that instruction and starts nothing.
- Cleaning a worktree up stays volee's job, with delivery evidence (ADR
  0011): it unlocks the worktree only after the evidence allows it.

## Consequences

- Launching works for every repository the user already uses Claude Code in,
  which is the usual case, with no change to the user's configuration.
- Worktrees sit next to Claude Code's own, so tools that know Claude Code's
  layout (and `claude rm`) see them where they expect them.
- People who wanted worktrees on another disk cannot have them, as long as
  Claude Code's trust model stays per path.

## Alternatives considered

- **Worktrees under volee's data directory.** Simple and tidy, but every
  launch fails on trust.
- **Writing trust entries into Claude Code's configuration.** Works, but edits
  a file volee does not own and bypasses a security prompt the user should
  answer.
- **Letting Claude Code create the worktree** (`--worktree` with a fresh
  name). Trusted, but the branch is named `worktree-<name>` and starts from
  `HEAD`, so volee would lose control of the branch name and its base.
- **Starting the agent in the main checkout and telling it to work in the
  worktree.** Fragile: tools and hooks would see the main checkout as the
  project, and one wrong path would edit the user's own checkout.
