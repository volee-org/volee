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

## When the daemon checks

The daemon keeps the workspace and evidence of every agent up to date by
itself, sessions volee did not launch included, so the interface and the
mission supervisor never wait for git:

- **Workspaces.** When an agent registers, or its working directory changes,
  the daemon reads where the directory sits: the repository (named after its
  `[repos.<name>]` entry in the configuration when one matches, otherwise
  after its folder), the main checkout, the worktree when it is a linked one,
  the branch, and the base branch (the one already known, else the configured
  one, else the repository's default branch). The branch an agent was
  launched on, or first seen on, stays its branch even when `HEAD` moves.
- **Evidence.** It is read again the moment an agent finishes a turn, stops
  or fails, when the supervisor judges it, and every minute for live agents.
  The previous evidence's `unmerged_tip` is passed back, so merges are
  proven. Each repository is fetched at most every five minutes, in batch
  mode. A `delivery-updated` event is recorded only when the evidence
  changed, or when it is the first check since the agent's state changed,
  which the supervisor waits for.
- git and the forge never run under the daemon's state lock or on a request
  path: they run on background threads, for four agents at a time and one
  job per agent, and triggers that arrive meanwhile are merged into the next
  job. A failure is logged and the next trigger tries again.

## Pull requests

With the [GitHub CLI](https://cli.github.com) installed and signed in, the
daemon looks up the pull request of each agent's branch, in the repository's
directory so `gh` finds the GitHub repository from its remotes, and maps its
state (draft, open, merged, closed) and its checks (passing, pending,
failing). A branch is looked up at most once a minute, and again right after
its agent stopped. When `gh` is missing or signed out, pull requests stay
unknown and `volee daemon status` says why. volee never sees a token: `gh`
uses your own sign-in.

A branch name alone does not identify a pull request: the name may have been
used before (an old pull request, merged long ago), and forks may have
branches of the same name. `gh pr list --head <branch> --state all` returns
the ten most recent pull requests with that name, and volee keeps the most
recent one that is really the branch's:

- its head repository is the one `origin` points to (`owner/name`, read from
  `git remote get-url origin`); when `origin` is not a repository on a forge,
  no pull request is the branch's;
- its head commit is the tip of the agent's branch, exactly. A pull request
  whose head is a later commit (someone pushed to it from elsewhere) counts
  once the agent's branch has that commit too.

The pull request records its head commit, and counts as delivery evidence
only while the branch is still at that commit: commits made after it are not
part of it, until the next look-up finds them in it.

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
