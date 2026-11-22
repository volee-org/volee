-- volee configuration (Lua). Every setting is optional.
-- The file returns a table and runs in the same sandbox as the rules.
-- Docs: https://volee-org.github.io/volee/guide/configuration.html
-- Decisions about a change (risk, layers, merge, how long a question is
-- held) are not here: they live in rules.lua next to this file.

return {
  agents = {
    -- Model for the agents volee launches, unless a repository sets one.
    -- model = "opus",

    -- Permission mode passed to Claude Code: "acceptEdits", "auto", "manual",
    -- "dontAsk" or "plan". Unset: "manual".
    -- permission_mode = "auto",

    -- Seconds an idle agent stays reachable by messages.
    wake_timeout = 3600,

    -- How often the daemon lists Claude Code's sessions
    -- (`claude agents --json`), in seconds.
    -- poll_interval = 5,

    -- resume_after_reboot = true,
  },

  -- One entry per repository. The key is the name used on the command line.
  -- repos = {
  --   billing = {
  --     path = "~/code/billing",
  --     base = "develop",      -- the repository's default branch when unset
  --     model = "sonnet",
  --     setup = { "bun install" },
  --     merge = "squash",      -- "merge", "squash" or "rebase"
  --   },
  -- },

  messages = {
    -- summaries = true,
    -- summary_model = "haiku",
  },

  missions = {
    -- max_workers = 8,
  },

  git = {
    -- Which repositories the daemon fetches in the background:
    -- "launched" (default), "all" or "never".
    -- fetch = "launched",
  },

  review = {
    -- The AI pre-review of finished work (Claude Code's /code-review).
    -- The model and the number of reviewers come from rules.lua.
    -- enabled = true,
    -- auto = true,
    -- timeout = 300,
    -- concurrency = 1,
    -- command = "claude",
  },

  hooks = {
    -- The port on 127.0.0.1 where the daemon receives Claude Code's hooks.
    -- `volee doctor` writes it into the hook settings: run it again after a
    -- change.
    -- port = 47835,

    -- Seconds a permission request waits for your decision in the browser.
    -- permission_timeout = 3600,
  },
}
