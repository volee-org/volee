# 0040. A finished change is read in layers, from git first and the transcript second

- Status: Accepted
- Date: 2026-11-22

## Context

Step 6 of the v2 plan opens a finished-work card into a screen that lets the
person decide how much to read: what was done, in what order, why, and the
diff. Two sources exist. Git knows exactly what changed. The Claude Code
transcript (`~/.claude/projects/<encoded cwd>/<session>.jsonl`) knows why and
how, but it is Claude Code's own format and it changes between versions. The
framing note therefore asks for it to be read through one tolerant module that
degrades to hooks-only facts.

## Decision

- **Git is the truth, the transcript is commentary.** The files, the counts,
  the diff and the diagram come from `git diff base...branch`. The transcript
  only adds the prompts, the plan, the reason each file was touched, the
  commands run and the agent's closing message.
- **One tolerant reader.** `crates/volee-daemon/src/transcript.rs` is the only
  code that knows the format. A line that does not parse, a line of an unknown
  type, a missing field: skipped and counted. An unreadable or missing file
  gives "transcript unavailable" and the screen shows the git and hook facts
  only. It is tested against a real recorded session (filtered to the
  conversation lines, home directory rewritten) and synthetic odd lines.
- **The window** starts at the last prompt typed before the branch's first
  commit (the last prompt when that is unknown).
- **The plan says where it comes from**: the agent's task list (`TodoWrite`,
  `TaskCreate` and `TaskUpdate`), a plan-mode plan (`ExitPlanMode`), or, as a
  last resort, the numbered steps of a prompt. The last one is labelled as what
  was asked, not as a record of what was done. A real haiku session had no task
  tool, so this fallback is the common case for small models.
- **Open points** are the lines under a "Not done" style heading of the
  closing message, else lines with a cue ("did not", "n'ai pas"), plus what
  volee notices: unfinished plan steps, a failed last test run, source changed
  with no test run recorded, uncommitted changes. Each says who noticed.
- **The diagram is deterministic**: files grouped by directory, an arrow where
  a changed file gained a relative import (JavaScript, TypeScript) or a `mod`
  declaration (Rust) of another changed file, directories only above 36 files.
  No model is asked.
- **The diff is ordered for reading**: source, tests, docs, config, then lock
  and generated files (folded). Patches are cut per file (150 kB) and in total
  (3 MB) and say so.
- **The page loads the heavy parts on demand.** Mermaid and diff2html are
  dynamic imports, fetched when their layer opens (about 0.9 MB in 28 files with
  the dagre layout, against 0.1 MB for the page itself).

## Consequences

- The release binary embeds the whole `ui/dist` (about 5 MB, most of it lazy
  Mermaid chunks that are only fetched when a diagram is drawn).
- The reason of a file is the last thing the assistant said before touching it
  and is empty when it said nothing: a hint, not an explanation.
- Uncommitted changes are not in the diff, which is `base...branch`; the screen
  lists them as an open point and names the files the agent touched that the
  diff does not hold.
