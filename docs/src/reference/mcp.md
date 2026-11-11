# MCP tools for agents

Agents talk to volee, and to each other, through an
[MCP](https://modelcontextprotocol.io) server that volee runs for each of them
(ADR 0005). Agents launched by volee get it automatically. Any MCP-capable
harness can use it too:

```json
{
  "mcpServers": {
    "volee": { "type": "stdio", "command": "volee", "args": ["mcp", "--agent", "ag_01jc5…"] }
  }
}
```

The server speaks for one agent, given by `--agent`. It starts the volee
daemon if needed.

An agent of another harness, which volee did not launch, joins instead
([ADR 0014](../adr/0014-agents-of-other-harnesses-join-through-mcp.md)):

```sh
volee mcp --join <name> [--harness <kind>] [--mission <title, short id or id>]
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--join` | required | The agent's name. The same name, harness and directory find the same agent again on the next start. |
| `--harness` | `other` | The harness it runs in, such as `codex` or `gemini-cli`: lowercase letters, digits, `.`, `_` and `-`. `claude-code` is refused: volee finds those sessions itself. |
| `--mission` | none | An open mission to take part in, so that its members and this agent can write to each other. Without it, the agent writes to the person, and the person to it. |

The directory the server runs in is the agent's workspace. The agent is
idle while the server runs and stopped once it exits; each tool call shows
as its activity. No hooks hand it messages, so the server's instructions ask
the model to call `read_messages` at the start of a task, after a question,
and before it finishes. The [guide](../guide/other-harnesses.md) has
configurations for Codex CLI and Gemini CLI.

Agents volee launches may call every tool below without a permission
prompt: their settings allow `mcp__volee`, so that a background agent never
stalls on asking to send a message. Only a mission's coordinator may call
`spawn_worker`, `request_approval` and `close_mission`.

## Tools

### `whoami`

The caller's identity: id, name, role, state, repository, branch, worktree,
base branch, mission and parent.

### `list_agents`

Every agent volee knows, those that need attention first. Each entry has an
id, a name, a role, a state, the repository and branch, the mission, the
parent, the harness, and `can_message`: whether the caller may write to it.

| Argument | Type | Default | Meaning |
| --- | --- | --- | --- |
| `include_finished` | boolean | `false` | Also list stopped, failed and lost agents. |

### `send_message`

| Argument | Type | Default | Meaning |
| --- | --- | --- | --- |
| `to` | list of strings | required | Agent ids (`ag_…`), agent names from `list_agents`, or `human`. |
| `body` | string | required | Markdown. |
| `kind` | `question`, `answer`, `contract`, `report`, `alert`, `note` | `note` | What the message is for. |
| `in_reply_to` | string | none | The id of the answered message, which keeps the thread. |
| `refs` | list | none | Each item sets one of `file` (with an optional `line`), `commit` or `url`. |

Routing follows ADR 0006: inside a mission every member writes to every
member; otherwise an agent writes to its parent, its children, its mission's
coordinator, the agent that requested its mission, and the human. A refusal
comes back as a tool error that explains why, so the model can correct
itself.

A message to an agent whose session ended resumes it, unless the human
stopped that agent or its mission is over: then the message waits. When the
human stops a worker of a running mission, volee tells the coordinator in the
mission's thread, so that it plans without that worker.

### `read_messages`

Messages addressed to the caller, oldest first, with sender, kind, id and
references. Returned messages are marked as read.

| Argument | Type | Default | Meaning |
| --- | --- | --- | --- |
| `unread_only` | boolean | `true` | Only messages not read yet. |

Messages also reach the agents volee launched without this tool: at the end
of a turn through the `Stop` hook, or by waking an idle agent. An agent that
joined with `--join` has no hooks: this tool is the only way its messages
reach it.

### `ask_human`

Asks the human a question that only they can decide, with the answers they
choose from. The question stays on top of the human's messages, in the
interface and in `volee msg questions`, until they answer it.

| Argument | Type | Meaning |
| --- | --- | --- |
| `question` | string | The question, short and complete: the human may read it hours later. |
| `options` | list of strings | The answers to choose from: two to six, each a few words on one line (40 characters at most), all different. |

The answer reaches the agent as an `answer` message in reply to the
question, like any other message. Its text is one of the options, in the
option's own spelling, when the human picked one; anything else is the
human's own words and is never a yes. The tool returns at once: the agent
ends its turn and volee wakes it with the answer.

`send_message` with the kind `question` still asks a question without
options. Options on anything but a question to the human are refused
([ADR 0031](../adr/0031-a-question-may-offer-its-answers.md)).

### `report`

Tells the agent's parent, its mission's coordinator and the human that the
task is done, blocked or failed. A `done` report is a `report` message;
`blocked` and `failed` are `alert` messages.

| Argument | Type | Meaning |
| --- | --- | --- |
| `outcome` | `done`, `blocked`, `failed` | How the task ended. |
| `summary` | string | What was done, what is left, what to check. |
| `refs` | list | Commits, pull requests or files that prove the work. |

### `spawn_worker`

For a mission's coordinator: launches a worker on a new branch, in its own
worktree of one of the mission's repositories. volee tells the coordinator
when the worker stops, with the evidence it checked. A mission launches at
most `[missions] max_workers` agents (8 by default); past it, the tool
returns an error that tells the coordinator to ask the human.

| Argument | Type | Meaning |
| --- | --- | --- |
| `repo` | string | A repository, as the mission lists it. |
| `branch` | string | A new branch, such as `feat/invoices`. |
| `task` | string | The whole task: the worker knows only what it says. |
| `name` | string, optional | A short display name. |
| `base` | string, optional | The branch to start from; the repository's default branch otherwise. |

### `request_approval`

For a mission's coordinator, once volee told it that every worker delivered:
presents its report to the human and asks for their approval
([ADR 0032](../adr/0032-a-mission-is-merged-only-after-the-person-approved.md)).

| Argument | Type | Meaning |
| --- | --- | --- |
| `done` | string | What is done, in one line. |
| `proof` | string | The proof, in one line: pull requests, branches, test results. |
| `left` | string, optional | What is left, in one line; "nothing" when left out. |

volee posts the report as a question from the coordinator to the human, in
the mission's thread, with the answers `ok merge` and `no` (`ok close` and
`no` for an analysis mission); the lead that asked for the mission reads it
too. The mission waits: nothing is merged or closed before the human
answers. The tool returns at once, and the coordinator ends its turn. volee
then tells it what the answer allows: on the approving answer, to merge each
worker's work and call `close_mission`; on any other, that nothing is merged
and the mission stays open.

Refused, with the reason, when the caller is not the coordinator, while a
worker has not delivered (a report presented for approval says the work is
ready), while the human has not answered the report already presented, and
once they approved.

### `close_mission`

For a mission's coordinator: closes the mission once the human approved the
report presented with `request_approval` and, for a change mission, the
work of every worker is merged. It takes no argument. volee reads the
workers' evidence again from git and the forge before it answers, and
refuses, with the reason, when the caller is not the coordinator, when the
human has not approved, or while volee sees no merge of a worker's branch.
An agent can never force a mission closed: only the human can.

Closing then tidies the mission: the worktree of every worker whose work
landed is removed, its branch kept, and the coordinator is stopped once its
turn ended.
