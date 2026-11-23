# Journeys

A journey is what a person does with volee, done for real: the **installed**
`volee` binary, an isolated daemon, the real web interface driven by
Playwright (the Chrome channel), and real `claude --model haiku` sessions in a
sandbox repository. Each script prints its checks as `PASS` or `FAIL` and says
which one failed.

The test suite checks the parts. The journeys check the trip: a release whose
parts all pass can still fail a person at the first click. No version of volee
is released unless the whole set passes on a real machine
([RELEASING.md](../RELEASING.md)).

They never run in CI: they need a logged-in Claude Code, a few short prompts on
a small model, and Chrome.

## Rules of the journeys

- **Delivery is proved, never read off the screen or out of an agent's words.**
  The proof is the git state (a commit exists, a tip is part of the base, a file
  has its content), the session's transcript (a tool result, the answers that
  reached the model), Claude Code's own `claude agents --json`, or the hook
  events volee stored. A journey never searches for a word in what an agent
  says: a small model answers in French or in English from one run to the next.
- **Isolated.** The binary is installed by `scripts/install-local.sh` into
  `~/.local/volee-v2/bin`; its configuration, state and data live in
  `~/.local/volee-v2/{config,state,data}` with its own socket and hook port
  (47911). `~/.local/bin`, `~/.local/state/volee`, `~/.config/volee` (an older
  install) are never touched. Neither are `~/.claude.json` and
  `~/.claude/settings.json`: hooks go into each throwaway worktree's own
  `.claude/settings.local.json`, by `volee doctor --project`.
- **Throwaway work only.** Agents run in worktrees of a sandbox repository
  (default `~/Developer/Labs/cockpit-sandbox/alpha`, which Claude Code already
  trusts) on branches named `journey/*`, off a base branch `journey/base` that
  is checked out in the sandbox (a local merge needs its base checked out).
  Each journey removes what it made, and the runner puts the sandbox back on
  `main` at the end.
- **Element-only screenshots** go to `docs/v2/screenshots/journey-<name>-<n>-<what>.png`:
  each one is one element of the page, so no other agent of the machine shows.
  The daemon sees every Claude Code session of the machine; the journeys only
  look at their own branches.
- **Real models vary.** Agents are told to do exactly one small thing and not to
  ask. When a run fails for a reason that is the model's, say so and rerun it;
  when it is volee's, fix volee.

## Running

```sh
node journeys/run.mjs                    # every journey, installing first (a release build, a few minutes)
node journeys/run.mjs --skip-install     # reuse the install already in the prefix
node journeys/run.mjs --skip-install j05 j07   # only those
```

Needs Node, Chrome, dist (`brew install cargo-dist`), Rust, `claude` signed in,
and the sandbox repository. Playwright comes from `ui/node_modules` (`npm ci` in
`ui/`); no browser is downloaded. The exit code is 0 only if every check passed.
Variables: `VOLEE_JOURNEY_HOME` (the prefix), `VOLEE_JOURNEY_PORT`,
`VOLEE_JOURNEY_SANDBOX`, `VOLEE_JOURNEY_MODEL`.

| Journey | What a person does | Proof |
| --- | --- | --- |
| `j01-install` | Installs from a local artifact, sets a repository up with `volee doctor`, starts the daemon, opens `volee ui` | The installed binary, the settings file written, the health check, the page served from the binary |
| `j02-permission` | A card for a tool call: allows one in the browser, denies another with a reason | The file exists or not on disk; the tool result in the transcript |
| `j03-question` | One `AskUserQuestion` with a single choice, a multiple choice and a free text answered in the browser | The tool result of the question in the transcript |
| `j04-timeout` | A question nobody answers goes back to the terminal after its delay (a rule, 20 s here) | `claude agents` says the session waits; no answer in the transcript |
| `j05-work` | Reads finished work in layers, runs the AI pre-review | Git for the commit and the files; the stored review |
| `j06-risk` | The risk comes from `rules.lua`, follows an edit, is changed by hand and reset; a broken file shows a banner | The work API; `volee doctor` prints the same error |
| `j07-actions` | Opens a file in an editor (a stand-in), comments a line, asks a live agent for a fix, merges in one click | The stand-in's log; git (second commit, tip merged into the base); the hook events |
| `j08-resume` | Asks a fix of a session that was stopped: volee resumes it | Git; the same session id with a new process in `claude agents --all` |
| `j09-automerge` | A repository not listed does not merge by itself; listed in `auto_merge_repos`, a docs-only change merges alone | Git; the journal marks the merge as a rule's |
| `j10-remove` | `volee doctor --remove`, stops the daemon | The settings file is back as it was; the port is closed |

`j02` to `j09` need the daemon of `j01` running (the runner starts it when it
is not).

## Leftovers

Deleting the install (`rm -rf ~/.local/volee-v2`) is left to the person: the
journeys do not delete their own prefix.
