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
| `daemon.status` | `protocol` (the client's protocol version, or 0) | version, protocol, pid, start time, last event, agent count, harnesses, forge |
| `daemon.shutdown` | none | none; the daemon stops after answering |
| `events.since` | `after` (sequence number, optional) | `events` (sequence number and event), `last_seq` |
| `events.subscribe` | `after` (optional), `interactive` (optional, `false` by default) | the same batch, then `events.appended` notifications |
| `discovery.refresh` | `timeout_ms` (optional: 3,000 by default, 10,000 at most) | `listed`: whether a listing that started after the request was applied in time |
| `agents.forget` | `agent` | none |
| `agents.join` | `name`, `harness`, `cwd`, optional `mission` | `agent`, `rejoined` |
| `agents.activity` | `agent`, `activity` | none |
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
A failed start marks the agent `failed` and keeps the worktree; the same
request sent again reuses that worktree while nothing was done in it, and
forgets the failed agent
([ADR 0016](../adr/0016-what-a-failed-launch-left-untouched.md)). It refuses
coordinators (missions launch them) and, for a mission, repositories the
mission does not list. `missions.create` refuses (`-32003`) when the daemon
has no usable Claude Code (missing or too old, as `daemon.status` reports),
before it records anything. Otherwise it records the mission and launches the
coordinator; when that launch fails, it tells the human in the mission's
thread and `coordinator` is null.

### Joining

`agents.join` brings an agent of another harness into volee
([ADR 0014](../adr/0014-agents-of-other-harnesses-join-through-mcp.md)); `volee
mcp --join` sends it. The daemon finds the agent that joined with the same
`name`, `harness` and `cwd` (an absolute path) and was not forgotten, or
registers one (`launched_by: outside`, harness `other` with that name, role
`independent`), records it idle, and answers whether it knew it
(`rejoined`). With `mission`, which must be open, the agent is a member of
that mission, and its coordinator is told; joining again with another
mission, or without one, records a `membership-changed` event. The agent
stays alive while the connection that joined stays open: when the last such
connection closes, or when the daemon starts, it is stopped. Invalid names or
harnesses answer `-32602`, a mission that is over `-32002`, and `claude-code`
is refused (`-32002`): those sessions are found without joining.

`agents.activity` records a joined agent's last action in one line (its MCP
server sends one per tool call). It refuses agents that did not join.

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

An agent whose session runs in a multiplexer pane the daemon found
([ADR 0015](../adr/0015-tuios-location-adapter.md)) is not refused:
`agents.preview` returns what the pane shows, each row placed with a cursor
move, and `agents.attach` returns an empty `program` and a `focus` object
(`multiplexer`, `session`, `steps`). The client runs each step, a program and
its arguments, in order, in its own environment and without giving up its
terminal, so that the multiplexer acts on the person's own client.
`daemon.status` lists the multiplexers the daemon uses in `multiplexers`,
with whether each answers now.

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
than 4,096 events behind receives `events.lagged` (with the number of events
it `skipped`) and is dropped; it can resubscribe with `after` set to the last
sequence number it applied.

A client a person watches, such as the interface, subscribes with
`interactive` set to `true`. While at least one interactive subscription is
open, the daemon lists the harness's sessions every two to ten seconds
(two after a listing that changed something, longer while nothing changes)
instead of every thirty, and a new one triggers a listing at once, so the
client opens on fresh data
([discovery](../architecture/domain.md#reconciliation)). The
subscription stops counting when it ends or its connection closes. Programs,
such as an agent's MCP server, leave it `false`: agents volee launched
report through their hooks in real time anyway.

A one-off command that wants the sessions as they are now, such as
`volee ls`, sends `discovery.refresh` before `events.since`. The daemon
lists at once and answers when a listing that started after the request is
applied, or when `timeout_ms` runs out; the listing then still completes in
the background. Requests made while a listing runs share the next one.
`listed` is `false` without a harness, after a failed listing, or after the
timeout: the events then show the last listing, as before.

Rust clients get all of this from `volee_proto::replica`: `subscribe` returns
the backlog and a feed of the events that follow, which drops repeats and
picks up again after a lag on request (`subscribe_interactive` does the
same for a client a person watches), and `Replica` keeps a `Registry` up to
date from a feed. The MCP server reads a `Replica`; the interface follows an
interactive feed, because it also needs the events themselves.

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
