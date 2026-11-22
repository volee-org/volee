# volee v2: framing note

Status: draft for Jeff, 2026-11-21. Branch `v2`. Nothing in this note is
applied yet: no code, doc or ADR of 0.18 is moved or deleted.

## 1. Why v2

volee 0.18 is judged badly designed, for three reasons:

- **It owns terminals instead of observing.** One `volee-host` process per
  agent, a terminal emulator (libghostty-vt), take-over, edit mode, a typist
  that types messages at a prompt. Most of the 0.18 defects come from there
  (see section 8).
- **It redoes what Claude Code now provides**: background sessions,
  `claude agents`, messaging between sessions, agent teams.
- **It puts in a TUI what needs a graphical interface**: reading a diff, a
  schema of a change or a plan is not a terminal job. The TUI is 56 000 lines
  and the largest crate by far.

Direction of 2026-11-21: a new major version, built after and with
rex-cockpit (the lab). How to restart is not decided: see section 10.

## 2. Goals and non-goals (v1 scope)

**Goal.** A control tower against AI fatigue. volee observes Claude Code,
tells the person what needs a decision, and lets them decide with as little
reading as the risk allows.

**v1 = the decision inbox.** Three kinds of cards: finished work, question,
permission. Everything else is out.

**Out of v1:** agent graph, messages between agents, multi-repo missions,
mobile, any terminal ownership. Terminal adapters (Rex, tmux,
`claude attach`) are optional and only serve "open this agent's terminal".
volee is not a multiplexer.

**Principle: each technology is picked on its merits for the need**, never
for consistency with rex-cockpit or with itself. The lab proves ideas, not
stacks (it is Bun and TypeScript; v2 is not bound to it).

## 3. Users and the AI fatigue problem

The user runs several Claude Code agents at once, across repos. The scarce
resource is no longer typing, it is attention: every agent produces work to
read, questions to answer and permissions to grant, at unpredictable moments,
with no sense of what matters. The failure modes are rubber-stamping (approve
everything) and context-switch overload (read everything).

Principles that follow:

- **Validate according to risk**, configurable per repo and per developer: a
  typo fix in docs is merged on a glance, a migration needs the full screen.
- **Small changes in layers**: one-line summary, then plan, then schema, then
  diff, each opened only if needed.
- **Show what the AI did and why**: plan followed, decisions taken, what was
  not done.
- **The AI review is an opinion, never a gate on its own.**

## 4. What Claude Code gives us (observe, do not own)

Claude Code 2.1.330 already provides:

| Need | Source |
| --- | --- |
| Which agents exist and their state | `claude agents --json [--all]` (id, sessionId, name, cwd, kind, startedAt, state, status, waitingFor, pid) |
| Events as they happen | hooks: Notification (`permission_prompt`, `idle_prompt`, `agent_needs_input`, `agent_completed`), Stop, SubagentStart/Stop, TaskCreated/Completed, PermissionRequest |
| Push to a local server | `http` hooks: JSON posted, decision read from the response |
| What the agent did | transcripts `~/.claude/projects/<encoded dir>/<session>.jsonl` |
| Answer permissions and questions programmatically | Agent SDK (TS, Python) `canUseTool` |
| Pre-review | native `/code-review` (Haiku by default, two reviewers when risky) |

Not provided: any public API to type into a session open in a terminal. v2
never types into a terminal. Where a decision must come back to an interactive
session, it goes through a hook response (T1, T2). The SDK is not used in v1 (T3).

AGENTS.md and ADR 0004 forbid parsing Claude Code internal files. Transcripts
are the one exception (decision 2, section 9): they are the only source of
"what the AI did and why". They are read through one tolerant module that
degrades to hooks-only when parsing fails, and ADR 0004 is to be amended.

Pre-review: Open Code Review in delegation mode was tested on PR #185, no net
gain at equal model. Decision: native `/code-review`.

## 5. Architecture draft

```mermaid
flowchart LR
  subgraph CC[Claude Code sessions]
    A1[agent 1]
    A2[agent 2]
  end
  subgraph D[volee daemon, Rust]
    H[hook server: http hooks]
    P[poller: claude agents --json]
    T[transcript reader]
    G[git: diff, worktrees, merge]
    L[Lua engine, mlua: config and rules]
    DB[(SQLite event log)]
    M[MCP: ask_human]
    API[HTTP + WebSocket API]
  end
  subgraph UI[Web UI, TypeScript]
    I[inbox]
    C[card: summary, plan, review, diagram, guided diff, editor]
  end
  SDK[optional SDK module, TS]
  AD[terminal adapters: Rex, tmux, claude attach]
  A1 -- http hooks --> H
  A2 -- http hooks --> H
  P --> A1
  T --> A1
  H --> DB
  P --> DB
  T --> DB
  G --> L
  L --> API
  DB --> API
  M --> API
  API <--> I
  I --- C
  SDK -. later option, not v1 .-> API
  API -. open this terminal .-> AD
```

- **Daemon (Rust, majority of the code).** Receives Claude Code `http` hooks,
  polls `claude agents --json`, reads transcripts, runs git (diffs, worktrees,
  merge), keeps the database, serves `ask_human` over MCP, serves the UI API
  (HTTP + WebSocket) on localhost.
- **Lua for configuration and rules (decided by Jeff, like Rex).** The daemon
  embeds Lua through the `mlua` crate. One Lua config file (repos, defaults,
  adapters). The risk-based validation and merge rules, per repo and per
  developer, are Lua functions: they receive a change (paths, size, tests,
  review findings, agent, repo) and return a risk level, the layers to show
  and what a merge requires. Rust owns the facts and the effects (git, merge,
  database); Lua only decides. To settle at design time: sandbox the Lua state
  (no `os`, `io`, no network, instruction and time limits), a stable versioned
  API, load errors shown in the UI and falling back to the safest rule
  (full read, no auto-merge), and a declarative default file so most people
  never write Lua.
- **UI (TypeScript web app served by the daemon).** A browser runs only
  JavaScript, so TypeScript here is forced by the platform, not by habit.
  diff2html (guided diff), no imposed editor (the user's own editor opens
  through a configured command; a small in-browser editor only as a fallback),
  Mermaid (diagram of the change), React Flow (later, agent graph, not v1).
  Tauri may wrap it later as a desktop app: the UI must not depend on it.
- **Hooks are enough for v1** (section 7): permissions through a
  `PermissionRequest` http hook, questions through a `PreToolUse` http hook on
  `AskUserQuestion`, finished work through `Notification` and `Stop`.
- **SDK module (TypeScript): not in v1, a later option.** The Agent SDK exists
  only in TypeScript and Python: no SDK in Lua or Rust. Its sessions are
  invisible to `claude agents`, and the subscription terms forbid offering
  claude.ai login in a third-party product. Never reimplement the SDK internal
  protocol in Rust.
- **Adapters (optional).** "Open this agent's terminal": Rex, tmux,
  `claude attach`. They locate and focus; they never read or type.

Security: the local server binds to 127.0.0.1, hook and UI calls carry a
per-install token, no secret in the database or in the Lua file.

## 6. The decision inbox

**Cards**

| Kind | Trigger | Decision |
| --- | --- | --- |
| Finished work | `agent_completed` / Stop, worktree with commits | merge, fix, comment |
| Question | `agent_needs_input`, AskUserQuestion | answer (proposed answers first) |
| Permission | `permission_prompt`, PermissionRequest | allow once, allow rule, deny |

**Card screen** (finished work): summary (what and why), plan followed (from
transcript), AI review opinion (`/code-review`, opinion only), Mermaid diagram
of the change, guided diff (ordered by meaning, not by file name, with the
review notes inline).

**Actions:** merge according to the rules, ask for a fix (sent back to the
agent), comment a line (becomes the fix request), fix in the user's own editor
(opened by a Lua-configured command, see decision 6).

**Risk-based validation.** Each change gets a risk level from the Lua rules
(paths touched, size, migrations, dependencies, tests changed or not, review
findings). The level picks the layers shown and whether a merge needs a full
read, a glance, or can be automatic. A per-repo rules file and a per-developer
override, so the same change can need different care for two people.

**Validated in the lab (rex-cockpit, `src/inbox*.ts`, `PLAN.md`), to reuse:**
agents joined by pid between `claude agents --json` and the terminal; groups
"waiting for an answer", "to review", "to tidy"; per-worker diff summary
(ahead, dirty, merged) with squash and cherry-pick recognition; the base of a
worker is its checkout branch or its remote, whichever is furthest ahead;
"tidy" only when the branch holds nothing unique; `/code-review` run in Haiku
from the card. What the lab could not do and v2 must: decide a permission or a
question without typing into the terminal.

## 7. Check results

Three technical checks, all done on 2026-11-21. Each result is marked verified
(measured on Claude Code 2.1.330) or assumed (from the docs).

**T1. PermissionRequest `http` hook.** Does it wait for the local server
answer, what is the maximum delay, are allow and deny applied?
- Result (2026-11-21, Claude Code 2.1.330): **yes, it waits.** An 8 s delay
  was measured, then the tool ran. `allow` and `deny` (`decision.behavior` plus
  a message shown to the model) are applied.
- The normal "Do you want to proceed?" dialog is shown immediately in the
  terminal during the wait: the hook is a concurrent decider. If the human
  answers in the terminal first, the tool runs and the server gets no signal,
  so an inbox decision may arrive stale.
- On timeout (configurable `timeout`, 5 s tested) it falls back to the normal
  dialog, not to a deny; a late allow is ignored. Timeout changes are picked up
  without restart. Assumed from the docs, not measured: default http timeout
  600 s, `"onFailure": "block"` to fail closed.
- Payload: `session_id`, `transcript_path`, `cwd`, `permission_mode`,
  `hook_event_name`, `tool_name`, `tool_input`, `permission_suggestions`. No
  `tool_use_id` and no request id.
- Design consequences: permission cards are handled by the daemon, no SDK for
  them. Set an explicit long timeout. Treat inbox answers as possibly stale and
  reconcile with the transcript (a card disappears when the tool ran). Correlate
  requests without an id by `session_id` plus `tool_input`. Falling back to the
  terminal dialog is the safe default when nobody answers.

**T2. PreToolUse hook on AskUserQuestion answering in place of the user, in an
interactive session.**
- Result (2026-11-21, Claude Code 2.1.330, interactive session, auto and
  default modes, verified): **yes**, as a command or http hook. Working body:
  `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"questions":[<original array echoed back>],"answers":{"<exact question text>":"<option label or free text>"}}}}`.
  No dialog appears and the model receives the answer. Free text works.
- `deny` plus `permissionDecisionReason`: no dialog, the model gets the reason
  as a hook error. `allow` without `updatedInput` fails ("Invalid tool
  parameters"), then the normal dialog shows.
- The hook input includes `tool_use_id` (unlike `PermissionRequest`).
- **Measured at step 5 (2026-11-22, Claude Code 2.1.330):** while a blocking
  http hook holds the question, the terminal shows no dialog, only the spinner
  line `running PreToolUse hooks... 1/2`; when the daemon answers nothing
  (delay expired, or the person chose "Ask in terminal") the normal dialog
  appears and works; a multi-select answered in the terminal is recorded as the
  labels joined by `, ` (`"Red, Blue"`), and the same string sent through the
  hook reaches the model unchanged. See ADR 0038.
- Design consequences: question cards are answered from the inbox for any
  session. Open point: while volee holds the question, the person at the
  terminal sees nothing, so volee must warn them (decision 4) and have a
  fast "let the terminal ask" answer (a timeout or no-decision response) that
  leaves the normal dialog. Echo the questions array back unchanged.

**T3. Agent SDK with the subscription.** Result (2026-11-21):
- It works with the subscription login, without API key, and `canUseTool`
  answers both permissions and AskUserQuestion.
- SDK sessions do **not** appear in `claude agents --json`: only their
  transcript exists. volee would have to track them itself and could not see
  them with the poller.
- Anthropic forbids third-party products from offering claude.ai login.
  Individual local use is allowed; an open source tool that each user runs
  themselves is a grey zone. volee is public and open source, so the SDK
  module must not offer or handle login: it would only use the login the user
  already has, and we should get the exact terms confirmed before shipping it.
- Consequence: the SDK is NOT needed for v1 (T1 and T2 passed). It stays a
  later option, opt-in and off by default, for sessions volee itself starts,
  once the terms are confirmed.

**Outcome.** Hooks are enough for v1: no SDK, no terminal typing, Rust daemon
plus web UI only.

| Card | Mechanism | Status |
| --- | --- | --- |
| Permission | `PermissionRequest` http hook, concurrent decider, explicit long timeout, stale-answer reconciliation | verified (T1) |
| Question | `PreToolUse` http hook on `AskUserQuestion`, `allow` with `updatedInput` | verified (T2), see open points |
| Finished work | `Notification` (`agent_completed`), `Stop`, plus git and transcript | documented, to prove in stage 2 |
| Agent list | `claude agents --json` | documented |

## 8. Keep, adapt, drop (from 0.18)

Sizes are lines of Rust in `crates/` at `origin/main`.

| Part | Lines | Verdict | Why |
| --- | --- | --- | --- |
| `volee-store` (SQLite event log, hook spool) | 1.5k | **Keep** | Matches ADR 0003, small, tested |
| `volee-mcp` (rmcp) | 2.2k | **Keep, adapt** | `ask_human` stays; drop agent-to-agent tools |
| `volee-config` (TOML) | 2k | **Replace** | Becomes the Lua config (mlua); the repo model carries over |
| `volee-git` | 8.5k | **Keep, trim** | Diffs, worktrees, delivery evidence (ADR 0011) are core |
| `volee-core` | 14k | **Adapt** | Events, agent identity, merge rules; drop host and screen concepts |
| `volee-proto` | 4.5k | **Adapt** | JSON-RPC over a socket becomes HTTP + WebSocket |
| `volee-daemon` | 23k | **Adapt, mostly rewrite** | Keep event log wiring, merge flow, ask flow; drop `typist.rs`, `views.rs`, `screens.rs`, `panes.rs`, `resume.rs`, launch |
| `volee-harness` | 10k | **Adapt** | Claude Code profile and hook settings stay; launch, preview, take over go |
| `volee-cli` | 7.4k | **Adapt** | Becomes thin: start the daemon, open the UI |
| `volee-mux` | 3.7k | **Adapt** | Becomes the optional "open this terminal" adapters |
| `volee-host`, `volee-term`, `volee-tui` | 65k | **Drop** | The terminal-owning design and the TUI |

Known 0.18 defects, all in what is dropped (`crates/volee-daemon/src/typist.rs`):
edit-mode take-over (an `owned` check, then a Control connection that always
takes the input), pasting into a draft or a dialog box, double sends. They are
a consequence of typing into terminals, not bugs to fix in v2.

**ADRs.** Keep as history, mark superseded: 0002, 0009, 0019 to 0028, 0034,
0035 (terminal ownership, TUI, host, emulator, edit mode). Keep: 0001, 0003,
0004, 0010, 0011, 0012, 0013. Adapt: 0005, 0006 (messaging reduced to
`ask_human`), 0007 and 0032 (merge rules survive, missions leave v1), 0008
(multiplexers optional becomes the founding principle), 0014, 0015. Drop or
archive: 0029 to 0031, 0033. New ADRs: Lua config and rules, observe-only,
web UI, SDK policy.

**Docs.** The mdBook site stays, rebuilt around the inbox; the architecture
overview is rewritten first.

**Journeys.** The idea stays: a release gate that does a person's trip for
real. The scripts are tied to the TUI and tuios, so they are rewritten as
browser journeys (real Claude Code through hooks, real UI driven by
Playwright). Keep `journey.py` structure and the sandbox convention.

**Release.** Keep dist, git-cliff, `RELEASING.md`, CI and cargo-deny. Add a UI
build step (the web bundle is embedded in the binary).

## 9. Decisions (Jeff, 2026-11-21)

1. **ADRs and docs:** as proposed in section 8. All ADRs are kept as history,
   superseded ones marked, the listed ones kept or adapted. New ADRs for Lua,
   observe-only, web UI and SDK policy. The mdBook site is kept and rebuilt
   around the inbox.
2. **Transcripts:** yes, read through one tolerant module that degrades to
   hooks-only. ADR 0004 is to be amended.
3. **Merge:** automatic merge is allowed at low risk when a Lua rule says so,
   and it is visible in the journal. Otherwise, one click.
4. **Held question:** volee holds an `AskUserQuestion` for a short delay,
   configurable in Lua (a few minutes by default), then hands it back to the
   normal terminal dialog. The right default is measured at step 5.
5. **Lua surface:** a readable settings table by default (thresholds,
   sensitive paths) plus Lua functions for special cases. One file per repo
   plus a personal file outside the repo.
6. **Editor:** no single embedded editor is imposed. The user chooses their
   own (vim, neovim, WebStorm, VS Code...), configured in Lua as commands with
   placeholders (`{file}`, `{line}`, `{dir}`, `{base}`). A TUI editor opens in
   a terminal through an adapter (Rex floating pane, tmux, or a new terminal
   window). A GUI editor opens as an external command (`open -a`,
   `code -g {file}:{line}`...). The UI shows the change back once the file is
   saved. A small in-browser editor may stay as a fallback only, to decide
   later. Reuse the `[tui.tools]` "Open with" of 0.18 (PR #198), same idea.

## 10. Open questions for Jeff

0. **Restart from scratch.** Not decided. Which of options A, B or C of
   section 12 (recommendation: A), and when. Until Jeff agrees explicitly, no
   old code, ADR or doc is removed or marked, and each removal is asked
   separately.
1. **Name and repo.** Keep `volee` and `volee-org/volee` for v2, or a new name
   to mark the break? (Recommendation: keep.)
2. **UI shell.** Plain browser tab first, Tauri later: confirm.
3. **Review model.** Haiku by default, two reviewers when risky: who decides
   "risky", the Lua rules or the person per card?
4. **First targets** for rules (slapeo, volee itself), and whether a Rex
   adapter alone is enough for v1.
5. **License and public status** unchanged (MIT OR Apache-2.0, public)?
6. **SDK module, later.** Do we ever ship an opt-in module, given the grey
   zone of the terms (T3)? Not for v1.

## 11. Staged plan

Small steps from the empty `v2` branch to the v1 inbox. Each ends with
something to demo and a proof that is run for real, in the spirit of the
journeys (a release is not done until the proof passes on a real machine).

| # | Step | Demo | Proof |
| --- | --- | --- | --- |
| 0 | **Not started, needs Jeff's agreement.** Clean slate: `legacy/0.x`, remove host, term, tui and TUI journeys, workspace builds, CI and release green | `cargo build` of a near-empty workspace, CI green | CI on the branch; `dist plan` still passes |
| 1 | Hook server: daemon on 127.0.0.1 with token, receives every Claude Code http hook, stores events in SQLite, `volee doctor` writes the per-install hook settings | `volee` run, a real Claude session, events listed by a CLI | Journey: real Claude in a sandbox repo, assert the stored event sequence |
| 2 | Agents and finished work: `claude agents --json` poller joined to hooks, worktree and git facts (ahead, dirty, merged) | `volee agents` shows state and finished work with a diff summary | Journey: real worker finishes in a sandbox, card data matches git |
| 3 | Minimal web UI: the inbox list over WebSocket, served by the daemon, bundle embedded in the binary | Browser tab shows live cards, `Open terminal` through one adapter (Rex) | Playwright journey against a real agent |
| 4 | Permission cards: `PermissionRequest` http hook, allow and deny from the UI, long explicit timeout, stale reconciliation with the transcript | Approve a real tool call from the browser, no terminal touch | Journey: allow, deny, timeout fallback to the terminal dialog, answered-in-terminal first (card disappears) |
| 5 | Question cards: `PreToolUse` on `AskUserQuestion`, answers and free text, hold delay from decision 4 (Lua, a few minutes by default) measured here | Answer a real question from the browser | Journey: answer, deny, timeout; multi-select format measured first |
| 6 | Finished-work card screen: summary, plan from transcript, guided diff (diff2html), Mermaid diagram of the change | Open a finished worker and read it in layers | Journey on a real change; transcript parser tested against recorded real files, degrades to hooks-only on failure |
| 7 | Pre-review: native `/code-review` as an opinion on the card | Review notes inline in the diff | Journey; review never blocks a merge by itself |
| 8 | Lua engine and rules: sandboxed mlua, config file, risk function per repo and per developer, layers shown by risk, load error falls back to the safest rule | Same change shown as a glance or a full read depending on the rules | Rust unit tests of the sandbox limits; journey with two rule files |
| 9 | Actions: merge according to the rules, ask for a fix, comment a line, fix in the user's own editor (Lua command with `{file}`, `{line}`, `{dir}`, `{base}`; TUI editor through an adapter, GUI editor as an external command; the UI shows the change once saved) | A finished change merged from the browser; a line opened in nvim and in a GUI editor | Journey: merge with delivery evidence (ADR 0011), fix request reaches the agent as a message, the saved edit appears on the card |
| 10 | v1 release: docs rebuilt, `ask_human` over MCP kept, installer, `v2.0.0` after Jeff's explicit go | `curl ... installer \| sh`, then the inbox on a real day of work | The whole journey set on a real machine, then a day of daily use before the tag |

Order rationale: steps 1 to 3 prove the observe-only pipeline end to end before
any decision is taken; 4 and 5 are the two risky checks of section 7 turned into
features; 8 comes before 9 so merges are never unruled.

## 12. Way to restart from scratch on branch `v2` (open, see section 10)

Options:

- **A. One commit that removes the old code, history kept.** `v2` branches
  from `main`, a `legacy/0.x` pointer marks 0.18, keepers stay in place and are
  adapted commit by commit. History, blame and the release pipeline survive.
- **B. Orphan branch.** Clean tree, no history. Clearer break, but loses blame
  for the code we keep (store, git, mcp) and the CI and release setup must be
  rebuilt or copied by hand.
- **C. New repository.** Same loss as B, plus issues, stars and the docs site
  URL.

**Recommendation: A**, in a few reviewable commits: (1) remove the
terminal-owning crates and the TUI (host, term, tui, TUI journeys), with the
superseded ADRs kept and marked, not deleted; (2) cut the daemon down to the
event log, git, merge flow and MCP; (3) the new skeleton (hook server,
`claude agents --json` poller, Lua engine). Before commit 1, a maintainer
creates `legacy/0.x` on the current `main` head so 0.18 stays maintainable.
`main` keeps shipping 0.18 until v2 reaches the inbox, then v2 is merged and
released as `v2.0.0`. This note applies none of it, and nothing is removed or marked before Jeff
agrees, removal by removal.

## 13. Working rules

- Commits and pushes are allowed on `v2` and its work branches.
- Merge into `main` only when Jeff validates, even with green CI.
- No publication of any kind: no tag, release, package, binary, installer or
  docs site update.
- A step is done only after local tests and real trials in the sandbox.
- No old code, ADR or doc is removed or marked without Jeff's explicit
  agreement, asked separately for each removal.
