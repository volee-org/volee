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
coordinator and the human. A refusal comes back as a tool error that explains
why, so the model can correct itself.

### `read_messages`

Messages addressed to the caller, oldest first, with sender, kind, id and
references. Returned messages are marked as read.

| Argument | Type | Default | Meaning |
| --- | --- | --- | --- |
| `unread_only` | boolean | `true` | Only messages not read yet. |

Messages also reach agents without this tool: at the end of a turn through
the `Stop` hook, or by waking an idle agent.

### `report`

Tells the agent's parent, its mission's coordinator and the human that the
task is done, blocked or failed. A `done` report is a `report` message;
`blocked` and `failed` are `alert` messages.

| Argument | Type | Meaning |
| --- | --- | --- |
| `outcome` | `done`, `blocked`, `failed` | How the task ended. |
| `summary` | string | What was done, what is left, what to check. |
| `refs` | list | Commits, pull requests or files that prove the work. |
