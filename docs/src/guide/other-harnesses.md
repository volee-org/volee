# Agents of other harnesses

volee finds Claude Code sessions by itself. An agent of another harness,
such as Codex CLI or Gemini CLI, joins volee through its MCP server instead:
it then shows up in `volee ls` and in the interface, can write to you and
read your answers, and, inside a mission, can talk with the mission's other
agents, whatever AI runs them. The decision behind it is
[ADR 0014](../adr/0014-agents-of-other-harnesses-join-through-mcp.md).

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
