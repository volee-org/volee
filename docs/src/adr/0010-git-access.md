# 0010. Git is read through the git CLI behind a trait

- Status: Accepted
- Date: 2026-11-06

## Context

volee needs worktrees, branches, upstream tracking, ahead and behind counts,
dirty state, diff statistics against a merge base, commit lists and full
diffs, for many repositories. `gix` (gitoxide) reads most of this natively and
fast, but some porcelain operations and worktree writes are incomplete, and
behaviour must match what users see in their own git.

## Decision

- All git access goes through a `Git` trait in `volee-git`.
- The first implementation shells out to the `git` CLI with porcelain
  formats, a timeout and a concurrency limit.
- Results are cached per worktree and invalidated by events, so git runs at
  most once a minute per worktree unless something changed.
- Tests use real temporary repositories.

## Consequences

- Identical results to the user's git, including their configuration.
- A `gix` implementation can replace hot paths later without touching
  callers.

## Alternatives considered

- **gix everywhere now.** Faster, but more code and edge cases for little gain
  at volee's polling rate.
- **libgit2 (`git2`).** C dependency, lags behind git features.
