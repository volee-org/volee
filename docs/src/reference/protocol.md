# Daemon protocol

The daemon listens on a Unix socket (see [Command line](../guide/cli.md#isolated-runs)
for its path) readable only by the current user. Clients speak
[JSON-RPC 2.0](https://www.jsonrpc.org/specification), one JSON object per
line, in both directions. The Rust types live in the `volee-proto` crate.

Try it by hand:

```sh
echo '{"jsonrpc":"2.0","id":1,"method":"daemon.status","params":{"protocol":1}}' \
  | nc -U ~/.local/state/volee/volee.sock
```

## Methods

| Method | Parameters | Result |
| --- | --- | --- |
| `daemon.status` | `protocol` (the client's protocol version, or 0) | version, protocol, pid, start time, last event, agent count, harnesses |
| `daemon.shutdown` | none | none; the daemon stops after answering |
| `events.since` | `after` (sequence number, optional) | `events` (sequence number and event), `last_seq` |
| `events.subscribe` | `after` (optional) | the same batch, then `events.appended` notifications |
| `agents.forget` | `agent` | none |
| `agents.preview` | `agent` | `screen`: the agent's recent terminal output, with its escape sequences |
| `agents.diff` | `agent`, optional `full` | `text`: a summary by file, or the full patch; `truncated` |
| `agents.attach` | `agent` | `program`, `args`, `cwd`, `env_remove`, `hint`: the command that takes the agent over |
| `agents.stop` | `agent` | none |
| `agents.tidy` | `agent`, optional `dry_run` | `outcome`: `removed` or `would-remove` (`worktree`, `branch`), or `kept` (`reasons`) |
| `messages.send` | `from`, `to`, `kind`, `body`, optional `refs`, `in_reply_to`, `thread` | `message`, `thread` |
| `messages.read` | `message`, `by` | none |
| `hooks.ingest` | `agent`, `event`, `payload` | `stdout`: what the hook prints, if anything |
| `hooks.wait` | `agent`, `timeout_ms` | `text`: messages that wake the agent, or `null` |
| `agents.spawn` | `repo`, `branch`, `task`, `role`, optional `base`, `parent`, `mission`, `model`, `name` | `agent`, `worktree` |
| `threads.summarize` | `thread` | `summary`, `messages`, `model`; refused unless `[messages] summaries` is on |
| `missions.create` | `title`, `kind` (`change` or `analysis`), `goal`, `repos`, optional `requested_by` | `mission`, `thread`, `coordinator` |
| `missions.abort` | `mission`, optional `reason` | none |
| `missions.close` | `mission`, optional `report`, `force`, `by` | none |

### Tidying

`agents.tidy` follows [ADR 0011](../adr/0011-safety-and-delivery-evidence.md):
it removes nothing without delivery evidence and never deletes a branch.
`delivery::tidy_check` decides from the registry: the agent was launched by
volee, works in a worktree under `<repository>/.claude/worktrees/`, is not
running, and its delivery evidence, checked since its last change, shows a
clean tree and landed work (merged, merged pull request, or report).
Otherwise the result is `kept` with every reason, each with a `reason` tag
(`busy`, `uncommitted-changes`, `not-delivered`, `not-launched-by-volee`,
`not-a-volee-worktree`, `evidence-not-checked`, `evidence-stale`,
`kept-by-harness`, `refused-by-git`) and a sentence for people when
displayed.

When the evidence allows it, the daemon asks the harness to delete the
session (with the evidence), checks the verdict again, unlocks the worktree
if the harness left it locked, and lets git remove it; git still refuses
changes and commits that no branch keeps, and never forces. It then records
the workspace without its worktree and forgets the agent; its events stay in
the log. With `dry_run`, the answer is `would-remove` or `kept`, and nothing
is touched.

### Launching

`agents.spawn` creates the branch and its worktree with git, writes the
agent's hook settings and MCP configuration (readable only by you), records
the agent, then starts the session from the repository's main checkout with
`--worktree` ([ADR 0013](../adr/0013-worktrees-live-in-their-repository.md)).
A failed start marks the agent `failed` and keeps the worktree. It refuses
coordinators (missions launch them) and, for a mission, repositories the
mission does not list. `missions.create` launches the coordinator when Claude
Code is available; when it cannot, it tells the human in the mission's thread
and `coordinator` is null.

### Acting on agents

`agents.preview`, `agents.attach` and `agents.stop` act on the agent's
current harness session, as the last listing showed it; an agent volee just
launched is reachable before any listing shows it. They refuse (`-32002`) an
agent whose session runs in a terminal of its own, which only that terminal
can show or control, and answer `-32001` when the harness no longer knows the
session. `agents.stop` also refuses agents volee did not launch
([ADR 0011](../adr/0011-safety-and-delivery-evidence.md)).

`agents.attach` returns a command for the client to run in its own terminal,
with the environment variables in `env_remove` removed; the harness keeps the
agent running when the person comes back.

`agents.diff` compares the agent's worktree (committed and uncommitted
changes, and the files it created) with the merge base of the branch it
started from. The text is at most 512 KiB, cut at a line break when longer,
with `truncated` set.

### Missions

`missions.create` opens the mission's thread and records the mission in the
`planning` status. From then on the daemon runs the
[supervisor](../architecture/domain.md#the-mission-supervisor) after every
batch of events that concerns the mission, and once when it starts. Its
decisions are ordinary events (`worker-assessed`, `mission-updated`) and
messages from volee in the mission's thread, so every client sees them.

`missions.close` refuses (`-32002`) while a worker has not delivered, unless
`force` is set. Without `report`, the coordinator's latest report in the
mission's thread is recorded. When `by` names an agent, it must be the
mission's coordinator, it must have sent a report, and it cannot force.

## Events and replicas

Clients do not ask the daemon for "the list of agents". They read events
(`events.since`) and apply them to their own `Registry` from `volee-core`,
exactly as the daemon does. A subscription (`events.subscribe`) answers with
the backlog and then pushes every new event as a notification:

```json
{"jsonrpc":"2.0","method":"events.appended","params":{"seq":42,"event":{"type":"state-changed","at":1793993747000,"agent":"ag_01jc5…","state":{"state":"working"},"detail":null}}}
```

Notifications arrive in log order without gaps. A subscriber that falls more
than 4,096 events behind receives `events.lagged` and is dropped; it can
resubscribe with `after` set to the last sequence number it applied.

## Errors

Standard JSON-RPC codes, plus volee's own:

| Code | Meaning |
| --- | --- |
| `-32700` | the line is not JSON |
| `-32600` | not a valid request |
| `-32601` | unknown method |
| `-32602` | invalid parameters |
| `-32603` | internal error |
| `-32001` | not found: agent, message, thread or mission |
| `-32002` | refused by a rule (routing, cleanup, a mission that cannot close) |
| `-32003` | a required program (harness, git) is unavailable |
| `-32004` | protocol versions differ |

## Versioning

The protocol version is `1`. Adding a method or an optional field does not
change it; changing or removing one does, and `daemon.status` tells a client
which version the daemon speaks.
