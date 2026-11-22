# Rules files (Lua)

volee decides how much care a change needs from a Lua file. Most people only
fill in a table of settings. Functions are there for the special cases. See
[ADR 0037](../adr/0037-lua-config-and-rules.md) for why.

## Where the files live

| File | Who it is for |
| --- | --- |
| `<repo>/.volee/rules.lua` | Everyone who works on that repository. Committed. |
| `<config dir>/rules.lua` | You, on every repository. Not committed. |

Both are optional, and neither is `config.lua`: the machine's settings
(ports, repositories, the review command) live in
[`config.lua`](../guide/configuration.md), which a repository can never
supply. The rules files are reloaded live when they change. The **merge
order** is: built-in defaults, then the
repository file, then your personal file.

- A scalar (`auto_merge = true`) or a function in a later file replaces the
  earlier one.
- A table of settings (`size`, `review`, `layers`) merges key by key, so you
  can change one threshold.
- A list (`sensitive_paths`) replaces the earlier list. An empty list clears
  it. To extend instead, use the table your file receives (below).
- `editors` merges by `name`: same name replaces, a new name is added.

A file is a Lua chunk that returns a table. The personal file receives what
was merged so far as its first argument:

```lua
local base = ...
return {
  sensitive_paths = { base.sensitive_paths[1], "src/payments/**" },
}
```

## Settings

```lua
return {
  sensitive_paths = { "src/auth/**", "infra/**" },  -- high risk
  migration_paths = { "**/migrations/**" },         -- high risk (default shown)
  dependency_paths = { "Cargo.lock", "package.json" }, -- at least medium
  size = {
    medium_lines = 200, high_lines = 800,           -- lines added + removed
    medium_files = 10,  high_files = 30,
    untested_lines = 50,                            -- no test touched: medium
  },
  review = { model = "haiku", reviewers = 1, high_reviewers = 2 },
  layers = {                                        -- what the card shows
    low    = { "summary", "diff" },
    medium = { "summary", "review", "diff" },
    high   = { "summary", "plan", "review", "diagram", "diff" },
  },
  auto_merge = false,            -- true: a LOW risk change may merge by itself
  held_question_timeout = "15m", -- seconds (900) or "30s", "15m", "2h"
  editors = {
    { name = "nvim", kind = "terminal", cmd = "nvim +{line} {file}" },
    { name = "code", kind = "external", cmd = "code -g {file}:{line}" },
  },
  trust_repo_rules = false,      -- personal file only: see "Trusting a repository's file"
  trusted_repos = {},            -- personal file only
}
```

Every key is optional. Unknown keys are reported as warnings (a typo), wrong
types are errors.

**Globs**: `*` stays inside one path segment, `**` crosses segments, `?` is
one character. A pattern without `/` matches the file name at any depth
(`Cargo.lock`). A trailing `/` means everything below.

**Editors**: `terminal` editors open in a terminal pane, `external` ones are
just run. The command is split into words first and the placeholders `{file}`
(absolute path), `{line}`, `{dir}` (worktree) and `{repo}` are filled after,
so paths with spaces or shell characters stay one argument. Use quotes in the
template to group words.

## Functions

All are optional. When one is missing, volee computes the answer in Rust from
the settings.

| Function | Receives | Returns |
| --- | --- | --- |
| `risk(change)` | the change | `"low"\|"medium"\|"high"` and optionally a list of reasons (`return "high", { "touches billing" }`) |
| `layers(change, risk, reasons)` | the change, the level, the reasons | a list of layers |
| `review(change, risk, reasons)` | same | `{ model = "sonnet", reviewers = 2 }` (missing keys take the settings) |
| `auto_merge(change, risk, reasons)` | same | `true\|false` and optionally a reason |
| `finished(facts)` | what Rust knows about an agent | `true\|false` |

`auto_merge` is a setting when it is a boolean and a rule when it is a
function. Whatever it returns, **a high risk change is never merged
automatically**.

The **change** table:

| Field | Meaning |
| --- | --- |
| `repo`, `branch`, `base` | names |
| `files` | list of `{ path, added, removed }` |
| `added`, `removed`, `lines`, `file_count` | totals |
| `commits` | commits the base does not have |
| `tests_touched`, `migrations` | booleans computed by volee |
| `agent`, `developer` | who did the work, who is looking |

The **facts** of `finished`: `stopped_last`, `busy`, `commits`,
`unique_commits`, `merged`. The default rule is: not busy, stopped after its
last activity, not merged, and at least one unique commit.

### Automatic merge, and turning it off for one repository

When `auto_merge` allows a change (low risk, trust rules respected), the
daemon merges it **by itself once the agent finished**, once per branch tip,
and the journal on the inbox says it was a rule and gives the reason you
returned. There is no switch in the interface: turn it off where you set it.
A function can look at the repository:

```lua
return {
  auto_merge = function(change, risk)
    if change.repo == "billing" then return false, "billing is always read" end
    if risk == "low" and volee.touches(change, "docs/**") then
      return true, "docs only, low risk"
    end
    return false
  end,
}
```

The merge itself is the one-click merge of the work screen: a pull request
through `gh` when the repository's `origin` is on GitHub (method from the
repository's `merge` in `config.lua`), else a merge commit into the base in
the main checkout, which must be clean. See
[ADR 0041](../adr/0041-actions-on-a-finished-change-and-how-a-fix-reaches-the-agent.md).

### Editors

`editors` feed the "Open in ..." buttons on a file header and on a diff line
(click a line number). A `terminal` editor opens in a floating pane of the
agent's Rex session (and is hidden when Rex is not there); an `external` one
is just run, with the worktree as its working directory.

## Helpers

| Helper | |
| --- | --- |
| `volee.api` | the API version, `1` |
| `volee.glob(pattern, path)` | boolean |
| `volee.touches(change, glob_or_list)` | the first matching path, or `nil` |
| `volee.settings()` | the merged settings as a table |
| `volee.default.risk(change)` | level, reasons computed from the settings |
| `volee.default.layers(risk)`, `.review(risk)`, `.auto_merge(risk)` | the computed defaults |

`os.time`, `os.clock`, `os.date`, and the `table`, `string`, `utf8` and `math`
libraries are available. Nothing else is: no `io`, `require`, `load`,
`debug`, `coroutine` or other `os` functions. A call that runs more than 10
million instructions, 500 ms, or allocates more than 16 MiB is stopped.

## Trusting a repository's file

A repository's `.volee/rules.lua` is code from that repository. By default it
is **not trusted**, and the rules are evaluated twice: with its file and
without it (your own rules alone). Then:

- the risk is the **higher** of the two answers: a repository can raise a risk,
  never lower what your own rules say;
- `auto_merge` is your own rules' answer: a repository can never turn
  automatic merging on, whatever its file says (the card says so);
- everything else it sets (layers, review, editors, the held question delay)
  applies.

You opt in from your own file, which a repository cannot write:

```lua
return {
  trust_repo_rules = true,                    -- every repository
  trusted_repos = { "/Users/me/code/shop" },  -- or only these roots
}
```

A repository's file that tries to set either key is ignored.

## When something is wrong

A file that fails to load (cannot be read, syntax error, does not return a
table, invalid settings, runs out of budget) makes **every answer the safest
one**: high risk, no automatic merge, every layer, at least two reviewers.
The error is shown with its file and line, for example
`/repo/.volee/rules.lua:12: attempt to index a nil value`. A function that
fails when called makes that one answer the safest and is reported with its
name. A `finished` function that fails falls back to the built-in rule.

## A realistic file

```lua
-- .volee/rules.lua of a web shop
return {
  sensitive_paths = { "src/payments/**", "src/auth/**", "infra/**", ".github/workflows/**" },
  size = { medium_lines = 150, high_lines = 600 },
  review = { model = "haiku", reviewers = 1, high_reviewers = 2 },
  held_question_timeout = "20m",
  editors = {
    { name = "nvim", kind = "terminal", cmd = "nvim +{line} {file}" },
    { name = "code", kind = "external", cmd = "code -g {file}:{line}" },
  },

  risk = function(change)
    local level, reasons = volee.default.risk(change)
    local only_docs = true
    for _, f in ipairs(change.files) do
      if not (volee.glob("docs/**", f.path) or volee.glob("*.md", f.path)) then
        only_docs = false
      end
    end
    if only_docs then return "low", { "documentation only" } end
    if change.migrations and not change.tests_touched then
      return "high", { "migration without a test" }
    end
    return level, reasons
  end,

  auto_merge = function(change, risk)
    if risk ~= "low" then return false, "needs a person at " .. risk end
    if change.agent ~= "claude" then return false, "unknown agent" end
    return true, "low risk change by claude"
  end,
}
```

A personal file that is stricter for you only:

```lua
local base = ...
return {
  risk = function(change)
    -- base.risk is nil when no earlier file defines one
    local level, reasons = (base.risk or volee.default.risk)(change)
    if level == "low" and change.lines > 100 then
      return "medium", { "more than 100 lines, I want to read it" }
    end
    return level, reasons
  end,
  auto_merge = false,
}
```
