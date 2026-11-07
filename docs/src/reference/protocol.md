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
| `messages.send` | `from`, `to`, `kind`, `body`, optional `refs`, `in_reply_to`, `thread` | `message`, `thread` |
| `messages.read` | `message`, `by` | none |
| `hooks.ingest` | `agent`, `event`, `payload` | `stdout`: what the hook prints, if anything |
| `hooks.wait` | `agent`, `timeout_ms` | `text`: messages that wake the agent, or `null` |
| `missions.create` | `title`, `kind` (`change` or `analysis`), `goal`, `repos`, optional `requested_by` | `mission`, `thread` |
| `missions.abort` | `mission`, optional `reason` | none |
| `missions.close` | `mission`, optional `report`, `force`, `by` | none |

More methods (`agents.spawn`, `agents.preview`, `agents.diff`,
`agents.attach`, `agents.tidy`) arrive with the harness and git integration.

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

Rust clients get all of this from `volee_proto::replica`: `subscribe` returns
the backlog and a feed of the events that follow, which drops repeats and
picks up again after a lag on request, and `Replica` keeps a `Registry` up
to date from a feed. The MCP server reads a `Replica`; the interface follows
a feed, because it also needs the events themselves.

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
