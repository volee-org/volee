# 0037. Lua config and rules

- Status: Accepted
- Date: 2026-11-22

## Context

v2 gives each change a risk level and lets that level pick what the person
reads and whether the merge can be automatic
([framing note](https://github.com/volee-org/volee/blob/v2/docs/v2/framing.md),
sections 5 and 6). Jeff decided that this is configured in Lua, like Rex:
readable settings by default, Lua functions for special cases, one rules file
per repository and one personal file per developer. The framing note left the
design questions: sandbox, limits, merge of the two files, versioning, and
what happens when a file is broken. `volee_core::finished` was written to be
replaced by the same mechanism.

## Decision

- **A new crate, `volee-rules`**, standalone: it depends on no other volee
  crate and is not yet wired into the daemon. Rust owns the facts (typed
  `Change`, `FinishedFacts`) and the effects; Lua only decides.
- **Lua 5.4, vendored, through `mlua`** (features `lua54`, `vendored`,
  `serialize`, `send`). Luau was the other serious candidate (see below).
- **A file returns a table.** Keys are settings (`sensitive_paths`,
  `migration_paths`, `dependency_paths`, `size`, `review`, `layers`,
  `auto_merge`, `held_question_timeout`, `editors`) and optional functions
  (`risk`, `layers`, `review`, `auto_merge`, `finished`). `auto_merge` is a
  setting when it is a boolean and a rule when it is a function. Whatever is
  missing is computed in Rust from the settings, so most people write no Lua.
  Unknown keys are a warning (a typo should not turn the rules off), wrong
  types are an error.
- **Two files**: `<repo>/.volee/rules.lua` and `<config dir>/rules.lua`.
  Order: defaults, repo file, personal file. Scalars and functions replace;
  tables of settings merge key by key; lists replace (an empty list clears),
  except `editors`, which merges by `name`. The personal file receives a copy
  of what was merged so far as its first argument, to extend a list or wrap a
  function (`local base = ...`).
- **Sandbox.** Loaded: `table`, `string`, `utf8`, `math`, and an `os` reduced
  to `time`, `clock`, `date`. Absent: `io`, `package`, `require`, `debug`,
  `coroutine`, `load`, `dofile`, `loadfile`, `string.dump`, `collectgarbage`,
  `xpcall`. Chunks load in text mode only (bytecode is refused). `print` does
  nothing. `pcall` is replaced by one that cannot swallow an exhausted
  budget. Every call (the file's top level counts as one) has an instruction
  limit (10 million), a wall-clock limit (500 ms) and a memory ceiling
  (16 MiB above the sandbox itself), enforced by a VM hook every 1000
  instructions. The limits belong to the embedding Rust code, not to the file.
- **Failure is safe and loud.** A load error (read, syntax, wrong return,
  invalid settings, budget) degrades the whole rule set: every answer is the
  safest outcome (high risk, no automatic merge, every layer, at least two
  reviewers) and the settings are the built-in defaults, so a broken file
  cannot half-apply. A runtime error in one function makes that one answer the
  safest; a failing `risk` gives high risk, and the other answers follow from
  it. `finished` is the exception: it falls back to the built-in rule,
  because a stray card costs less than a missed one. Each error carries the
  file and line (`RuleError`) and the function name.
- **High risk is never merged automatically**, whatever `auto_merge` returns.
  That is a Rust invariant, not a convention of the file.
- **A small versioned API**: `volee.api` is `1`; helpers are `volee.glob`,
  `volee.touches`, `volee.settings()` and `volee.default.risk|layers|review|auto_merge`,
  so a function can start from the computed default and adjust.
- **Editor commands** are validated and expanded in Rust: the template is
  split into words first, placeholders (`{file}`, `{line}`, `{dir}`, `{repo}`)
  are filled after, so a path with spaces or shell characters is one argument
  and never interpreted.
- The old TOML `volee-config` crate is untouched. Replacing it with this one
  is a later step.

## Consequences

- A person can adjust risk with a readable table and reach for a function
  only for a special case. The same change can need different care for two
  people, because the personal file wins.
- Lua 5.4 is compiled from C by `lua-src`: it needs a C compiler, which the
  release machines already have (SQLite is bundled the same way).
- The VM hook cannot interrupt a single C function. A pathological pattern in
  `string.find` or `string.match` (exponential backtracking) can run past the
  time budget, since the hook only fires between VM instructions. The daemon
  should run evaluation off its main loop (or in a worker thread) and treat a
  late answer as the safest outcome. `string.rep` and table growth are
  bounded by the memory ceiling.
- A repository file is code that runs when volee loads the repo's rules. It
  cannot touch the machine (sandbox), but it can say `auto_merge = true` for a
  low risk change, or lower a risk. Someone who opens an untrusted repository
  in volee should know that; the personal file is the place to override it
  (`auto_merge = false`). A later option is to ignore the repo file's merge
  settings unless the personal file opts in.

## Alternatives considered

- **Luau** (Roblox). It has a built-in sandbox mode with read-only globals and
  interrupt callbacks for time limits, which is attractive. It lost on
  familiarity and tooling: Rex, the model for this decision, uses plain Lua;
  people know Lua 5.4 and their editors highlight it; Luau's extras (types,
  `continue`) are not needed for twenty-line rule functions; and Luau builds
  from C++. Its interrupt has the same blind spot as the 5.4 hook (a long C
  function), so the sandbox advantage is smaller than it looks. The mlua API
  is nearly identical, so a move later would be small.
- **LuaJIT or Lua 5.1.** No sandbox facilities that Lua 5.4 lacks, a JIT that
  is harder to bound, and older language semantics.
- **Starlark, Rhai, or a JSON/TOML rule language.** Safer by construction,
  but Jeff chose Lua, and a declarative-only format cannot express "docs
  only is low risk unless a migration is present". The settings table already
  gives the declarative path.
- **Letting the settings set the limits.** A file could then raise its own
  budget. The limits stay in Rust.
- **Degrading only the broken file.** Ignoring a broken personal file and
  keeping the repo's rules would silently drop the developer's stricter
  rules. All-or-safest is easier to reason about and impossible to miss.
