# 0036. The web interface is a Svelte app the daemon embeds and feeds by server-sent events

- Status: Accepted
- Date: 2026-11-22

## Context

v2 puts the decision inbox in a browser tab
([framing note](https://github.com/volee-org/volee/blob/v2/docs/v2/framing.md),
sections 5 and 6). A browser runs only JavaScript, so the interface is
TypeScript by force of the platform. What is left to choose is how it is
built, how it ships, and how the daemon keeps it live. The later steps add a
guided diff (diff2html), a diagram of the change (Mermaid) and, maybe, an
agent graph (React Flow). No editor is embedded: the person's own editor is
launched by a command.

Constraints that shape the choice:

- The interface ships **inside the binary**: `curl ... | sh` installs one
  file, and the daemon serves the page on its own loopback port. There is no
  separate web server and no network dependency at run time.
- Rust tests and a plain `cargo build` must not need Node.
- The page is a small, mostly read-only list that changes live. It is not a
  large application, and the fewer dependencies it has, the fewer there are to
  audit and update.

## Decision

- **Svelte 5 with TypeScript, built by Vite.** Components are compiled away:
  the first bundle is 55 kB (21 kB compressed), with no framework runtime to
  speak of. Reactive state (`$state`, `$derived`) fits "a list that a stream
  rewrites" without a store library. diff2html and Mermaid are plain DOM
  libraries and work the same in any framework. All the logic that matters
  (formatting, the token, the event stream, the terminal request) lives in
  plain `.ts` modules, tested with Vitest without a DOM, so a later change of
  framework would touch the components only.
- **npm**, with a committed `package-lock.json` and exact versions. It is
  installed on every CI runner and on the release machines; pnpm or Bun would
  add a tool to install for no gain at this size.
- **Lint, types, tests, build** are `eslint` (typescript-eslint and
  eslint-plugin-svelte), `svelte-check`, `vitest` and `vite build`, run as
  `npm run check` in `ui/` and as a CI job.
- **The bundle is embedded by a build script**, not a crate. `build.rs` of
  `volee-daemon` lists `ui/dist` and generates `include_bytes!` entries. When
  `ui/dist/index.html` does not exist it embeds a small stub page that says
  the interface was not built, so Rust tests and `cargo build` work without
  Node. A release build sets `VOLEE_REQUIRE_UI=1`, which turns the missing
  bundle into a build error: a release can never ship the stub. The release
  workflow builds the UI first (a build-setup step in `dist-workspace.toml`).
- **The same listener serves everything.** The hook server's loopback port
  also serves the page, a JSON API and the event stream. The page's files are
  public (the code is not a secret and the page must load before it can read
  its token); `/api/*` needs the per-install token; a `Host` that is not a
  loopback name is refused (DNS rebinding); responses carry a restrictive
  Content-Security-Policy.
- **The token reaches the page through the link's fragment.** `volee ui`
  prints and opens `http://127.0.0.1:<port>/#token=...`. A fragment is never
  sent to a server nor logged; the page keeps the token for its tab and
  removes it from the address bar.
- **Live updates use server-sent events (SSE), not WebSocket.** The traffic is
  one way (the daemon tells the page what changed); the page's few actions
  (open a terminal now, decide later) are ordinary `POST` requests. SSE is
  plain HTTP, reconnects by itself in the browser, needs no extra Rust
  dependency (WebSocket would add tungstenite and a handshake), and the daemon
  sends the whole inbox first on every connection, so a reconnection loses
  nothing. The inbox is small, so every event carries a full snapshot, not a
  diff. A browser's `EventSource` cannot set a header, so this one route also
  accepts the token as `?token=`.
- **"Open terminal" goes through an adapter trait.** An adapter only locates
  and focuses; it never reads a screen nor types. Rex is the first: it finds
  the pane by the agent's process id (the program Rex reports for the pane),
  falling back to a directory that exactly one pane has, then focuses it. When
  the adapter is not available (no Rex, no server socket) the interface hides
  the button. tmux and `claude attach` can implement the same trait later.
- **A change of agents reaches the page within a second.** The agents view
  counts the readings that changed what a person sees; the stream wakes on
  that counter. A stored hook event nudges the poller, which reads after 300
  ms (a burst of events around a stop is read once), instead of waiting for
  the next poll.

## Consequences

- One more toolchain to build a release (Node), contained in `ui/` and one CI
  job. A contributor who only touches Rust never needs it.
- The binary grows by about 60 kB today, more when the diff and diagram
  libraries arrive.
- The interface can be developed against a running daemon with `npm run dev`
  (Vite proxies `/api`).
- Every change of the inbox sends a full snapshot. If it grows large (many
  agents, long summaries), switch to deltas then; the stream's format is the
  only thing that would change.
- Browsers limit a host to six HTTP/1.1 connections; a person with many tabs
  open on the interface could exhaust them. One tab is the expected use.

## Alternatives considered

- **React.** The widest ecosystem, and the only framework with React Flow if
  the agent graph comes back. It ships a runtime of roughly 45 kB compressed
  before any code, and needs more ceremony for a live list. Svelte Flow exists
  if the graph returns, so React Flow alone did not justify the cost.
- **Solid.** Close to Svelte in size and reactivity, with a much smaller
  ecosystem and community; no gain that Svelte lacks here.
- **Plain TypeScript with no framework (or htmx and server-rendered HTML).**
  Fine for a list, but the card screen of step 6 (layers, guided diff, inline
  review notes) is exactly where hand-written DOM code turns into an ad hoc
  framework.
- **Bun or pnpm instead of npm.** Faster installs, but one more tool in CI and
  in the release job for a project with a dozen dev dependencies.
- **`rust-embed` or `include_dir` instead of a build script.** Both embed a
  folder, but both fail to compile when the folder is missing, which the
  Node-free Rust build forbids; the stub fallback and the release guard would
  need a build script anyway.
- **WebSocket.** Two-way, but nothing here needs the browser to talk back over
  the same channel, and it costs a dependency and a custom reconnection.
- **Polling `GET /api/inbox`.** Simple, but a stale card for up to the polling
  interval is exactly what the inbox must not show. The endpoint stays, for
  scripts and tests.
- **A desktop shell (Tauri) now.** Decided against for v1 in the framing note:
  a plain browser tab first, and the interface must not depend on a shell.
