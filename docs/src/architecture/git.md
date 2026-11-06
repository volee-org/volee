# Git

volee reads repositories through the `Git` trait of the `volee-git` crate
([ADR 0010](../adr/0010-git-access.md)). Its implementation runs your own
`git` executable, so volee sees what you see, your configuration included.
Code that depends on git is tested against a fake implementation of the
trait; the implementation itself is tested against real temporary
repositories, also under a deliberately hostile git configuration.

## How volee runs git

- Every command runs as `git -C <dir> --no-optional-locks ...` with standard
  input closed. Queries never change a repository, and never refresh the
  index of a worktree an agent is working in.
- git never waits for you, and nothing it starts can read your keystrokes.
  Terminal prompts are disabled (`GIT_TERMINAL_PROMPT=0`,
  `GCM_INTERACTIVE=never`, `SSH_ASKPASS_REQUIRE=never`), ssh runs in batch
  mode for fetches and checkouts unless you chose your own ssh command
  (`GIT_SSH_COMMAND`, `GIT_SSH` or `core.sshCommand`), and each git runs in a
  process group of its own. A fetch that needs credentials fails instead of
  hanging: use a credential helper or an SSH agent.
- Messages are read in English (`LC_ALL=C`) and only machine-readable formats
  are parsed. Your configuration cannot change what volee reads: untracked
  files are always looked for, submodule changes always count, and diffs are
  produced without colors, external diff programs, relative paths or custom
  prefixes.
- Queries time out after 30 seconds, creating or removing a worktree after 10
  minutes, a fetch after 5 minutes. A command that times out is stopped with
  everything it started: SIGTERM first, so git can remove its lock files and
  half-made worktrees, then SIGKILL two seconds later. At most four commands
  run at the same time.
- A fetch forgets remote branches deleted on the remote (`--prune`) and does
  not overwrite `FETCH_HEAD`.

## The base branch

Delivery evidence compares an agent's branch with its base branch (`main`,
`develop`...). volee uses `origin/<base>` when that remote-tracking branch
exists, because agents branch from it and pull requests merge into it, and the
local branch otherwise.

## Delivery evidence

[ADR 0011](../adr/0011-safety-and-delivery-evidence.md) lets volee remove a
worktree only on evidence. The evidence describes the agent's branch, not
wherever its worktree happens to point: when `HEAD` is detached or another
branch is checked out, volee says so and still reports on the branch. Here is
how each fact is computed:

| Fact | How |
| --- | --- |
| Commits | Commits on the branch that the base branch does not have. Commits the agent pulled from the base branch do not count. |
| Uncommitted changes | Any change to a tracked file, or any untracked file that is not ignored, in the worktree. |
| Diff size | What the branch changes against its merge base with the base branch. When the branch is checked out, uncommitted changes to tracked files count too. |
| Push state | Commits missing from the branch's upstream, or from `origin/<branch>` when the branch was pushed without `-u`. An upstream that is a local branch does not count. Neither: not pushed. |
| Merged | A tip of the branch once seen with commits of its own is now in the base branch. |

Proving a merge needs memory. A branch whose commits were merged has no commit
left that its base branch lacks, exactly like a branch that never had any. So
the evidence records the last tip seen with commits of its own
(`unmerged_tip`), and the next check, given that memory, reports the work as
merged once that commit is part of the base branch. The daemon keeps the
memory in its `delivery-updated` events. Some consequences are worth knowing:

- A branch never seen with commits of its own is never "merged", even though
  its tip is in the base branch: fast-forwarding an empty branch to a newer
  base proves nothing.
- New commits after a merge are new work: the branch is not merged again
  until they are.
- A branch that was merged and then deleted still counts as merged.
- A squash-merged branch is never in its base's history: its evidence is the
  merged pull request, which comes from the forge.

## Worktrees volee creates

- The worktree is created detached at `origin/<base>` (volee fetches first),
  and the branch is created inside it, with no upstream until the agent pushes
  it. A failure therefore never leaves a stray branch; when the worktree
  exists but a later step failed (a hook, a checkout stopped at its timeout),
  volee says the worktree is there.
- volee refuses to reuse an existing branch or directory, and checks both
  before it touches the repository.
- Removing a worktree is never forced. volee refuses to remove the main
  checkout, a locked worktree, a worktree with uncommitted changes or
  untracked files, and a detached `HEAD` that no branch, tag or
  remote-tracking branch contains, since those commits would be lost with the
  worktree. Ignored files, such as build outputs, are deleted with it. The
  branch is always kept.
