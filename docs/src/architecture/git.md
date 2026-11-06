# Git

volee reads repositories through the `Git` trait of the `volee-git` crate
([ADR 0010](../adr/0010-git-access.md)). Its implementation runs your own
`git` executable, so volee sees what you see, your configuration included.
Code that depends on git is tested against a fake implementation of the
trait; the implementation itself is tested against real temporary
repositories.

## How volee runs git

- Every command runs as `git -C <dir> --no-optional-locks ...` with standard
  input closed. Queries never change a repository, and never refresh the
  index of a worktree an agent is working in.
- git never waits for you: terminal prompts are disabled
  (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`), so a fetch that needs
  credentials fails instead of hanging. Use a credential helper or an SSH
  agent.
- Messages are read in English (`LC_ALL=C`) and only machine-readable formats
  are parsed.
- Each command has a timeout (30 seconds, 5 minutes for a fetch), and at most
  four run at the same time.
- A background fetch does not overwrite `FETCH_HEAD`.

## The base branch

Delivery evidence compares a worktree with its base branch (`main`,
`develop`...). volee uses `origin/<base>` when that remote-tracking branch
exists, because agents branch from it and pull requests merge into it, and the
local branch otherwise.

## Delivery evidence

[ADR 0011](../adr/0011-safety-and-delivery-evidence.md) lets volee remove a
worktree only on evidence. Here is how each fact is computed:

| Fact | How |
| --- | --- |
| Commits | Commits since the agent's start commit. For an agent volee did not launch, commits since the merge base with the base branch. |
| Uncommitted changes | Any change to a tracked file, or any untracked file that is not ignored. |
| Diff size | The working tree against the merge base: committed and uncommitted changes to tracked files. |
| Push state | Commits missing from the branch's upstream, or from `origin/<branch>` when the branch was pushed without `-u`. Neither: not pushed. |
| Merged | The branch has at least one commit, and its tip is in the base branch. |

Some consequences are worth knowing:

- A branch without commits is never "merged", even though its tip is in the
  base branch.
- After a fast-forward merge, nothing tells an agent's commits from the base
  branch's. Without a start commit, volee therefore reports the branch as not
  merged and keeps the worktree. A merged pull request is evidence too.
- A squash-merged branch is never an ancestor of its base: its evidence is the
  merged pull request, which comes from the forge.
- An upstream deleted on the remote counts as not pushed.

## Worktrees volee creates

- The branch starts from `origin/<base>` (volee fetches first) and has no
  upstream until the agent pushes it, so its push state is about the agent's
  own pushes.
- volee refuses to reuse an existing branch or directory, and checks both
  before it touches the repository.
- Removing a worktree is never forced: git refuses when the worktree has
  uncommitted or untracked files, or is locked, and volee reports why. The
  branch is always kept.
