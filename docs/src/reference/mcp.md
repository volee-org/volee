# MCP tools for agents

volee runs an [MCP](https://modelcontextprotocol.io) server, over stdio, that a
coding agent can use to ask you a question and to exchange messages
([ADR 0005](../adr/0005-harness-neutral-agent-protocol.md)). It is **optional**:
the inbox works from Claude Code's hooks alone and needs no MCP server.

> [!NOTE]
> The MCP server is carried over from 0.x. v2 does not launch agents, so no
> agent gets the server by default, and the messaging tools belong to the 0.x
> registry (see [Data and events](../architecture/domain.md#the-0x-registry)).
> The tool that matters for v2 is `ask_human`. The others may be removed or
> reworked before v2.0.0, and the tools that launched agents and missions
> (`spawn_worker`, `spawn_agent`, `start_mission`) are already gone.

## Using it

An agent of any harness joins volee by starting the server with a name
([ADR 0014](../adr/0014-agents-of-other-harnesses-join-through-mcp.md)):

```json
{
  "mcpServers": {
    "volee": { "type": "stdio", "command": "volee", "args": ["mcp", "--join", "reviewer"] }
  }
}
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--join` | required | The agent's name. The same name, harness and directory find the same agent again on the next start. |
| `--harness` | `other` | The harness it runs in, such as `codex` or `gemini-cli`: lowercase letters, digits, `.`, `_` and `-`. `claude-code` is refused: Claude Code sessions are seen through hooks and `claude agents --json`. |
| `--mission` | none | An open mission to take part in. Without it, the agent writes to you, and you to it. |

The directory the server runs in is the agent's workspace. The server starts the
daemon when it is not running. `volee mcp --agent <id>` is the form 0.x used for
agents it launched.

## Tools

| Tool | Does |
| --- | --- |
| `whoami` | The caller's identity: id, name, role, state, repository, branch, worktree, mission, parent. |
| `list_agents` | The agents of the registry, those that need attention first, with `can_message`. |
| `send_message` | A message to agents or to `human`, with a kind (`question`, `answer`, `contract`, `report`, `alert`, `note`), a reply target and references. Routing follows [ADR 0006](../adr/0006-messaging.md). |
| `read_messages` | Messages addressed to the caller, oldest first; they are marked read. An agent that joined with `--join` has no hooks: this tool is the only way its messages reach it. |
| `ask_human` | Asks you a question, with answers to choose from. |
| `report` | Tells the parent, the mission's coordinator and you that the task is done, blocked or failed. |
| `request_approval` | For a mission's coordinator: presents its report and asks for your approval ([ADR 0032](../adr/0032-a-mission-is-merged-only-after-the-person-approved.md)). |

### `ask_human`

| Argument | Type | Meaning |
| --- | --- | --- |
| `question` | string | The question, short and complete: you may read it hours later. |
| `options` | list of strings | The answers to choose from: two to six, each a few words on one line (40 characters at most), all different. |

The tool returns at once; the agent is told to end its turn. The question stays
unanswered until you answer it **from the command line**:

```sh
volee msg questions                  # the questions waiting for you, with their offered answers
volee msg answer <question|agent> 2  # the number of an offered answer, its words, or your own words
```

The web inbox does not list these questions: it shows the questions Claude Code
itself asks through `AskUserQuestion`, which is a different mechanism (a hook,
see [the interface](../guide/interface.md#a-question)). Your answer reaches the
agent as an `answer` message in reply to its question, which it reads with
`read_messages`: one of the options, in its own spelling, when you picked one;
anything else is your own words and is never a yes. Options on anything but a
question to you are refused
([ADR 0031](../adr/0031-a-question-may-offer-its-answers.md)).

### Routing and refusals

Inside a mission every member writes to every member; otherwise an agent writes
to its parent, its children, its mission's coordinator and you. A refusal comes
back as a tool error that explains why, so the model can correct itself. Only a
mission's coordinator may call `request_approval`, and it is refused while a
worker has not delivered or while you have not answered the report already
presented. No tool merges or closes a mission: volee merges, after your approval.
