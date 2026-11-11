# Agents of other harnesses

volee finds Claude Code sessions by itself, and launches, finds and resumes
[OpenCode](#opencode) agents too. For any other AI, there are two ways in,
and they add up:

- **[`volee run`](#any-program-volee-run)** starts it under volee from your
  terminal: it is in the list, you see its screen and you open it from the
  interface, like any agent. This works for any program, today.
- **[Joining](#join)** gives it volee's MCP tools: it can write to you, read
  your answers and, inside a mission, talk with the mission's other agents,
  whatever AI runs them
  ([ADR 0014](../adr/0014-agents-of-other-harnesses-join-through-mcp.md)).

## Any program: `volee run`

```sh
cd ~/code/billing
volee run -- codex                      # any command line, after --
volee run --name invoices -- gemini     # with a name of your choice
```

`volee run` starts the program in a volee host, in the directory you are
in and with your terminal's environment, exactly as if you had typed the
command alone. Your terminal is on it at once: you use the program as
usual. `ctrl-q` gives the terminal back and the program keeps running; it
is now an agent of the list, named after its directory unless you named it
([ADR 0029](../adr/0029-volee-run.md)).

From there it is an agent in a host, like the ones volee launches: its
preview, its tile on the wall and the full-screen view show its exact
screen, `enter` opens it inside volee where you type into it, `volee attach
<name>` opens it in a terminal, and `volee stop <name>` ends it. Run from a
script, with no terminal, the command starts the program and prints how to
open it.

What volee knows of such a program is its screen, and nothing else:

- **State.** A screen that changes is working, one unchanged for a few
  seconds is idle, a program that ended is stopped. volee cannot tell when
  it waits for you, and it shows as working while you type into it.
- **Messages.** Nothing hands it a message: you talk to it by typing into
  it. The composer and `volee msg send` say so. To give it volee's tools,
  [join](#join) it as well.
- **No resume.** When the program ends, by itself, by `volee stop` or with
  a reboot, nothing starts it again. Its row shows the end for a minute,
  then leaves the list; run the command again to have it back.
- **One row.** A program that volee also finds by itself (Claude Code in
  its own list, OpenCode in the process table) is listed once, as the
  program you ran, even when the command went through a wrapper such as
  `npx` or a shell.

For Claude Code, prefer what volee finds by itself when you can: a session
in a tuios or tmux pane, or a background session, is listed with its exact
state, its name and its summary, and opens where it runs. `volee run --
claude` is for a plain terminal, which has no door otherwise: you get the
door, with the screen-read state above.

## OpenCode

volee has a profile for [OpenCode](https://opencode.ai)
([ADR 0022](../adr/0022-harness-profiles.md)), measured on OpenCode 2.2.5:

```sh
volee spawn billing feat/export "Export invoices as CSV." --harness opencode
```

The launch dialog (`n`) has the same choice in its `harness` field. volee
creates the branch and the worktree as for any agent, runs the
repository's setup commands, then starts
`opencode --session <id> --prompt <task>` in a volee host, in the worktree,
with a session id it chooses (and `--auto` when
`[agents] permission_mode = "auto"`). Take it over with `a` or
`volee attach`, watch it in the preview, full screen or on the wall, and
stop it with `volee stop`, like any agent in a host.

- **State.** OpenCode takes no hooks on its command line, so volee reads
  the agent's state from its screen: a screen that changes is working, one
  unchanged for a few seconds is idle, a program that ended is stopped.
  volee cannot tell when it waits for you.
- **Messages.** Nothing hands it a message while it runs. A message waits
  until the agent stops, then volee resumes it in a new host with
  `opencode --session <id> --prompt <messages>`.
- **After a reboot.** volee resumes it like a Claude Code agent, with
  `opencode --session <id>` and a prompt that says the machine restarted.
- **Started elsewhere.** An OpenCode agent you started yourself is found in
  the process table (not the background service,
  `opencode serve --service`), with its directory, and so its repository
  and branch. It is shown until its process ends, then leaves the list a
  minute later.
  - **In a tuios or tmux pane**, it is an agent in a pane like any other
    ([panes](interface.md#agents-in-tuios-panes)): the list says which
    pane, its preview and its tile on the wall are the pane's screen, and
    in a tuios pane `enter` opens it inside volee, where you type into it;
    `ctrl-q` comes back and it goes on as before. Nothing is stopped or
    restarted for it.
  - **In a plain terminal**, volee has no door on it and says so: switch to
    that terminal. To drive it from volee, quit it, then adopt it (`A`, or
    `volee adopt`): volee resumes it in a host, which only works when it
    was started with `--session`, since volee needs the session id. Or
    start it with [`volee run -- opencode`](#any-program-volee-run) next
    time: it then runs in a host from the start.

## Join

Add volee's MCP server to the harness's configuration, with the name the
agent should have:

```sh
volee mcp --join infra --harness codex
```

- `--join` names the agent. Starting the harness again in the same directory,
  with the same name and harness, finds the same agent again, with its
  messages.
- `--harness` says which AI it is (`codex`, `gemini-cli`...), so the
  interface can tell agents apart; `other` when you leave it out.
- `--mission` (optional) makes the agent a member of an open mission, by its
  title, short id or id. Members of a mission write to each other, so this
  is how an agent of one AI talks with agents of another. Without it, the
  agent writes to you, and you to it.

The directory the server runs in is the agent's workspace: volee reads its
repository and branch there. Harnesses usually start their MCP servers in
your project; if yours does not, set the server's working directory in its
configuration.

## Codex CLI

Codex CLI reads its MCP servers from `~/.codex/config.toml` (or a project's
`.codex/config.toml`, for projects you trust):

```toml
[mcp_servers.volee]
command = "volee"
args = ["mcp", "--join", "infra", "--harness", "codex"]
```

Or with its own command:

```sh
codex mcp add volee -- volee mcp --join infra --harness codex
```

Check Codex CLI's documentation on MCP for the options of your version, such
as `cwd` or `startup_timeout_sec`.

## Gemini CLI

Gemini CLI reads its MCP servers from `~/.gemini/settings.json`, or from a
project's `.gemini/settings.json`:

```json
{
  "mcpServers": {
    "volee": {
      "command": "volee",
      "args": ["mcp", "--join", "docs", "--harness", "gemini-cli"]
    }
  }
}
```

Gemini CLI asks before each tool call of a server it does not trust; set
`"trust": true` on the server if you want volee's tools to run without
asking. It also has a `gemini mcp add` command. Check Gemini CLI's
documentation on MCP servers for that command and for the other options of
your version, such as `cwd` or `timeout`.

## What to expect

- **State.** The agent is idle while its harness runs, and stopped once the
  harness exits (its MCP server closes). volee cannot see when it works: the
  interface shows its last tool call as its activity, such as "wrote to
  human".
- **Messages.** Nothing hands it messages on its own: there are no hooks in
  other harnesses to do it. They wait until the agent calls `read_messages`,
  and the server's instructions ask the model to call it when it starts a
  task, after it asks a question, and before it finishes. You can also ask
  it to "check your volee messages". `volee msg send` says so when you write
  to it, and a message sent while it is stopped waits for its next start.
- **Missions.** A member that joined is never judged by the mission's
  supervisor, which judges workers only, and the coordinator hears that it
  joined. Joining again with another mission, or without one, changes its
  membership.
- **Not resumed.** volee never starts or resumes it: you start its harness.
- **Its screen.** An agent that only joined has no screen in volee: its
  harness runs in a terminal of yours. Start the harness with
  [`volee run`](#any-program-volee-run) to have its screen and a door on it
  too; the program you ran and the agent that joined are then two rows, one
  for the screen and one for the messages.
