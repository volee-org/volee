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
| `agents.preview` | `agent` | `screen`: terminal output that draws the agent's screen, with its escape sequences; `exact`: whether it is the screen itself, row for row, or the replay of a session's output; `size` (`cols`, `rows`): the size of the terminal it was drawn in, when the daemon knows it |
| `agents.diff` | `agent`, optional `full` | `text`: a summary by file, or the full patch; `truncated` |
| `agents.attach` | `agent` | `program`, `args`, `cwd`, `env_remove`, `hint`: the command that opens the agent in the client's terminal |
| `agents.open` | `agent`, `cols`, `rows` | `socket`: a volee host to connect to, for a client that shows the agent inside its own screen; `view`: whether it is a view on a session that runs elsewhere |
| `agents.stop` | `agent` | none |
| `agents.adopt` | `agent` | none; the agent's conversation continues in a volee host |
| `agents.tidy` | `agent`, optional `dry_run` | `outcome`: `removed` or `would-remove` (`worktree`, `branch`), or `kept` (`reasons`) |
| `messages.send` | `from`, `to`, `kind`, `body`, optional `refs`, `in_reply_to`, `thread`, `options` (the answers a question to the human offers: two to six short lines) | `message`, `thread` |
| `messages.read` | `message`, `by` | none |
| `hooks.ingest` | `agent`, `event`, `payload` | `stdout`: what the hook prints, if anything |
| `hooks.wait` | `agent`, `timeout_ms` | `text`: messages that wake the agent, or `null` |
| `agents.spawn` | `repo`, `branch`, `task`, `role`, optional `base`, `parent`, `mission`, `model`, `name`, `harness` (`claude-code` by default, or `opencode`) | `agent`, `worktree` |
| `agents.run` | `host`: the id of a volee host the client started, in the daemon's hosts directory; optional `name` | `agent`, `name`; the program of the host is now an agent |
| `threads.summarize` | `thread` | `summary`, `messages`, `model`; refused unless `[messages] summaries` is on |
| `missions.create` | `title`, `kind` (`change` or `analysis`), `goal`, `repos`, optional `requested_by` | `mission`, `thread`, `coordinator` |
| `missions.abort` | `mission`, optional `reason` | none |
| `missions.request_approval` | `mission`, `by` (its coordinator), `done`, `proof`, `left` | `question`: the report, posted as a question to the human that offers the approving answer and `no` |
| `missions.close` | `mission`, optional `report`, `force` | none; the daemon then tidies the mission's workers and stops its coordinator. The human's own gesture: a request with `by` (an agent) is refused |

### Running a program

`agents.run` makes the program of a volee host an agent
([ADR 0029](../adr/0029-volee-run.md)). The client starts the host itself
(`volee host run`, detached, under the daemon's hosts directory), so that
the program gets the client's environment and directory, then gives the
daemon the host's id. The daemon reads the host's record and registers an
agent in that host, launched by the human, with the role `independent`, the
name asked for (else the host's own name, else the name of the program's
directory) and, as its harness, the name of the program. It refuses an id
that names no file of its hosts directory (`-32602`), a host it does not
find (`-32001`), and a program that already ended or a host that already
is an agent (`-32002`); the client then ends the program it started.

The agent has no session, so nothing that needs one applies to it: its
states come from its screen, no message is handed to it and nothing resumes
it. `agents.preview`, `agents.open`, `agents.attach` and `agents.stop` act
on its host, as for any agent in a host; once its program ended,
`agents.open` and `agents.attach` are refused with words that say nothing
resumes it, and the agent leaves the registry a minute later. A session or
a process that a harness lists inside the host, the program itself or a
process below it, is not registered a second time.

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
the agent, then starts the session in a new volee host
([ADR 0019](../adr/0019-a-host-process-holds-each-agent-terminal.md)), from
the repository's main checkout with `--worktree`
([ADR 0013](../adr/0013-worktrees-live-in-their-repository.md)). With
`harness: "opencode"`, it starts `opencode --session <id> --prompt <task>`
in the worktree instead, for a lead or a worker; any other role there, and
any other harness, is refused (`-32602`).
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

For an agent in a volee host
([ADR 0019](../adr/0019-a-host-process-holds-each-agent-terminal.md)),
`agents.preview` returns a snapshot of the host's screen (`exact`, with the
`size` of the host's terminal: a snapshot redraws its screen in a terminal
of that size only), `agents.attach`
returns the `volee host attach` command for the client to run (Ctrl+Q comes
back), and `agents.stop` asks the host to end the program. For a Claude
Code background session that volee does not host, `agents.attach` returns
the harness's own command (`claude attach <id>`): nothing is stopped or
resumed ([ADR 0027](../adr/0027-open-without-owning.md)). `agents.adopt` is
another thing: it first stops the session, then resumes its conversation in
a new host. It refuses an agent that already runs in a host and one that
runs in a terminal of its own, which must be quit there first
([ADR 0023](../adr/0023-every-agent-on-the-machine.md)).

`agents.open` is for a client that draws the agent inside its own screen,
as the interface does. It answers with the socket of a volee host, which
the client connects to with the host protocol: the host the agent runs in
(`view` false), or a view (`view` true), a host the daemon starts on the
session's own door, at `cols` by `rows`: `claude attach` for a Claude Code
background session, running or finished, and the multiplexer's own command
for a pane it can show alone (`tuios pane-view`). A view ends its program
once no client has held its input for 15 seconds, and ends with the daemon;
the session it shows is not touched. Without a `socket`, the agent runs in
a pane only its multiplexer can show: `agents.attach` says how to bring it
forward. It refuses (`-32002`) an agent in a terminal the daemon cannot
reach, and says what makes it reachable.

For other agents, `agents.preview`, `agents.attach` and `agents.stop` act on
the agent's current harness session, as the last listing showed it; an agent
volee just launched is reachable before any listing shows it. They refuse (`-32002`) an
agent whose session runs in a terminal of its own, which only that terminal
can show or control, and answer `-32001` when the harness no longer knows the
session. `agents.stop` also refuses agents volee did not launch
([ADR 0011](../adr/0011-safety-and-delivery-evidence.md)). What
`agents.preview` returns for them is the replay of the session's output,
or the end of the setup log of an agent that never had a session: `exact`
is false, and a client may repair it as a replay
([ADR 0028](../adr/0028-one-emulator-and-the-wall-inside-the-interface.md)).

`agents.attach` returns a command for the client to run in its own terminal,
with the environment variables in `env_remove` removed; the harness keeps the
agent running when the person comes back.

An agent whose session runs in a multiplexer pane the daemon found
([ADR 0015](../adr/0015-tuios-location-adapter.md)) is not refused:
`agents.preview` returns what the pane shows, each row placed with a cursor
move (`exact`, without a `size`: it draws the same cells in any terminal
large enough), and `agents.attach` returns an empty `program` and a `focus` object
(`multiplexer`, `session`, `steps`, `attach`). The client runs each step, a
program and its arguments, in order, in its own environment and without
giving up its terminal, so that the multiplexer acts on the person's own
client. The first step focuses the pane in its session; when a later one
fails, `attach` is the command that shows that session in a terminal (such
as `tuios attach work`), for the client to suggest.
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

`missions.request_approval` is refused (`-32002`) unless `by` is the
mission's coordinator and the mission is `reporting`: every worker
delivered, and no report waits for an answer. It posts the report as a
question to the human and moves the mission to `awaiting-approval`. The
human's answer is an ordinary message in reply to that question
(`messages.send`); the supervisor reads it and moves the mission to
`approved`, or back to `reporting`.

No request merges: the daemon does, when a mission becomes `approved`
([ADR 0032](../adr/0032-a-mission-is-merged-only-after-the-person-approved.md)).
It reads the workers' evidence from git and the forge, merges the work that
has not landed (a pull request on the forge, a branch into its base branch
in the main checkout of a repository without a remote), reads the evidence
again, and closes the mission once every merge shows in it. Otherwise it
moves the mission back to `reporting` and posts an alert in the mission's
thread, to the human, the coordinator and the lead, that names each worker
whose work was not merged, with the reason. A mission that is `approved`
when the daemon starts is carried out then.

`missions.close` is the human closing a mission themselves, without a
merge. It refuses (`-32002`) while a worker has not delivered, unless
`force` is set, and it refuses a request made in an agent's name (`by`): no
agent closes a mission. Without `report`, the report the coordinator
presented is kept, else its latest report in the mission's thread. After a
close, by the human or by the daemon, the daemon tidies the workers whose
work landed, stops the coordinator, and posts one message in the mission's
thread that says what it tidied and what it kept.

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
