# 0014. Agents of other harnesses join through MCP

- Status: Accepted
- Date: 2026-11-07

## Context

volee discovers Claude Code sessions only (ADR 0004). An agent of another
harness, such as Codex CLI or Gemini CLI, can run `volee mcp`, which ADR 0005
made the harness-neutral way in, but the server needs an agent identity, and
only agents volee launched or discovered have one. So an agent of another AI
cannot take part in conversations yet. The product owner wants an agent of
AI A to talk to an agent of AI B, and chose "free inside a mission" for
routing: ADR 0006 already lets members of a mission write to each other, and
it stays as it is.

Such an agent gives volee nothing but its MCP server: no hooks, no session
listing, no process volee owns.

## Decision

- **Identity.** `volee mcp --join <name> [--harness <kind>] [--mission
  <mission>]` calls `agents.join` with the name, the harness, the directory
  the server runs in, and the mission. The daemon looks for an agent that
  joined with the same name, harness and directory, and was not forgotten. It
  finds that agent again, or it registers a new one: `launched_by: outside`,
  harness `Other(<kind>)`, role `independent`, workspace at that directory,
  enriched from git like any workspace. The name is what people and agents
  type, the directory tells two projects apart, and the harness tells two AIs
  in one project apart. An agent of another harness that volee did not launch
  is a joined one (`Agent::joined`), since volee discovers the sessions of no
  other harness yet.
- **State.** A joined agent is alive while a connection that joined for it
  stays open: the connection of its MCP server, which lives as long as its
  harness. It is `idle` when it joins. Each of its tool calls is recorded as
  its activity (`agents.activity`, sent by its server). When the last such
  connection closes, it is `stopped`. When the daemon starts, no connection
  is open yet, so joined agents still recorded as alive are stopped until
  their server joins again. volee never resumes a joined agent: it did not
  launch it.
- **Routing.** ADR 0006 unchanged. Outside a mission, a joined agent writes
  to the person, and the person to it. With `--mission`, it joins an open
  mission as a member: members of the mission and the agent write to each
  other, which is the AI A to AI B case. A new event, `membership-changed`,
  records the mission a joined agent takes part in when it joins again with
  another mission, or with none; only joined agents change missions. The
  supervisor judges workers only, so a member that joined is never judged,
  and the mission's coordinator hears that it joined.
- **Messages.** Nothing hands a joined agent its messages: there are no hooks
  to deliver them at the end of a turn or to wake it. They wait in its
  threads until it calls `read_messages`, which marks them read. Its MCP
  server's instructions tell the model so, and `volee msg send` says it. When
  the agent is gone, its messages wait for its next join.

## Consequences

- Any harness that can start an MCP server over stdio takes part, with one
  line of its configuration and no adapter in volee.
- A joined agent's state is coarse: idle while connected, stopped once its
  server is gone. volee cannot tell when it works, and the interface shows it
  that way.
- A message reaches a joined agent only when its model calls `read_messages`.
  The instructions ask it to do so at the start of a task, after a question,
  and before it finishes; a model that forgets leaves messages unread.
- Two servers of the same identity (the same CLI open twice in one project)
  are one agent, alive until both close.
- If volee later discovers sessions of another harness, `Agent::joined` will
  need an explicit marker to tell those from the agents that joined.

## Alternatives considered

- **A token or a file to keep the identity.** Robust across renames, but one
  more thing to configure and to lose. Name, harness and directory are what
  the person already writes in the harness's configuration.
- **Identity including the mission.** No new event, but joining a new
  mission would create a second agent with the same name, and commands that
  take a name would become ambiguous.
- **Liveness from tool calls with a timeout.** Works without a connection,
  but an idle agent that thinks for a while would flicker to stopped. The
  server's connection is exact and costs nothing.
- **Free routing between any agents.** Simpler for the AI A to AI B case, but
  ADR 0006 keeps conversations inside missions on purpose; joining a mission
  gives the same reach where it is wanted.
