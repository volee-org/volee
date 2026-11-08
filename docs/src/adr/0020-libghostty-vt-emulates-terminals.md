# 0020. libghostty-vt emulates every terminal volee reads

- Status: Proposed
- Date: 2026-11-08

## Context

volee reads screens in three places: the host keeps each agent's screen
([ADR 0019](0019-a-host-process-holds-each-agent-terminal.md)), watchers draw
that screen into the interface, and some sources of agents volee does not host
still arrive as a byte stream (`claude logs`, a pane capture). Until 0.7.0 the
interface used the `vt100` crate for the last case.

What the host needs from an emulator goes beyond drawing cells:

- render its state back as VT sequences, so that a client that connects late
  gets the exact screen (the snapshot);
- answer the program's terminal queries itself while no person is attached;
- understand what agents' interfaces use today: synchronized output (DEC
  2026), the kitty keyboard protocol, colors queried with OSC 10 and 11,
  hyperlinks, wide graphemes.

`vt100` does none of the first two and little of the third. Ghostty's
terminal core is published as a C library, **libghostty-vt**, with no
dependency on Ghostty's interface. It has a formatter that writes a screen as
VT sequences (with modes, cursor, keyboard protocol, palette, scrolling region
and tab stops), callbacks for every query (device attributes, cursor position,
version, colors, size), and a render state with row and cell iterators that
fill a ratatui buffer directly. zmx, cmux and agent-tty already build on it.

Rust bindings, as of 2026-11-08:

| Crate | Source of Ghostty | Zig | License | MSRV | Use |
| --- | --- | --- | --- | --- | --- |
| `libghostty-vt` 0.2.2 (uzaaft/libghostty-rs) | official git, pinned commit | 0.15.2 | MIT OR Apache-2.0 | 1.90 | 200 k downloads, 15 dependents |
| `ghostty-vt` 0.8.0 (gpui-cn) | official source tarball, sha256 | 0.17 | Apache-2.0 | 1.96 | 4 k downloads, 1 dependent |
| `ghostty-vt-rs-sys` 0.3.0 | prebuilt static libraries | none | MIT | 1.90 | prebuilt binaries |

## Decision

- volee uses **`libghostty-vt`**, pinned to an exact version (`=0.2.2`) in the
  workspace. Its build script fetches Ghostty's official repository at one
  pinned commit and builds the library with Zig. volee's own code calls only
  the safe API, so the workspace keeps `unsafe_code = "forbid"`.
- A new crate, **`volee-term`**, wraps it: a `Screen` (one terminal: feed
  bytes, resize, snapshot as VT, answer queries) and a ratatui widget that
  draws a `Screen` into a buffer. No other crate depends on libghostty-vt
  directly, so a change of binding touches one crate.
- The handles of libghostty-vt are neither `Send` nor `Sync`: each `Screen`
  lives on one thread (the host's terminal thread, the interface's thread).
- `vt100` is removed. The previews of sources volee does not host go through
  `volee-term` too.
- **Zig** becomes a build dependency, at the version the pinned commit
  requires (0.15.2 today):
  - contributors install it (`brew install zig@0.15`, the official archive,
    or any version manager) and `CONTRIBUTING.md` says so;
  - CI installs it with a pinned setup action;
  - the release workflow gets it through `github-build-setup` in
    `dist-workspace.toml`, and `release.yml` is regenerated with
    `dist generate`, never edited by hand.
- The MSRV becomes 1.90, the binding's own.
- An update of the binding is a pull request of its own, checked by the
  host's snapshot tests and a run on a real agent.

## Consequences

- A screen is the same everywhere: in the host, in a preview, in the wall of
  screens and after an attach, with the colors and modes Ghostty would show.
- Building volee needs Zig and, on the first build, network access to
  github.com (or `GHOSTTY_SOURCE_DIR` pointing at a checkout of the pinned
  commit). The first build takes about a minute and a half more; later builds
  reuse the library.
- The library is linked statically: no runtime dependency for people who
  install the release binaries.
- libghostty-vt's C API is young (its 1.0 tag is the first stable ABI, and
  names changed before it). Pinning the binding exactly keeps surprises to
  deliberate updates.
- If the binding stalls, the fallback is our own `-sys` crate over the
  official source tarball, behind the same `volee-term` API.

## Alternatives considered

- **`vt100` first, libghostty-vt later.** Cheaper to start, but no snapshot
  formatter and no query answers, which the host needs from day one. The
  product owner chose libghostty-vt right away.
- **`ghostty-vt` (gpui-cn).** Builds from the official tarball checked against
  a sha256, which is better, and uses Zig 0.17. But it serves one application,
  is licensed Apache-2.0 only and needs Rust 1.96. Worth revisiting if
  `libghostty-vt` stops following Ghostty.
- **`ghostty-vt-rs-sys`.** Prebuilt static libraries from a third party: a
  binary in the supply chain that nobody here built.
- **`alacritty_terminal`.** Pure Rust and mature, but no formatter back to VT
  sequences and no query callbacks; the host would have to write both.
- **Our own `-sys` crate now.** More work before the first prototype, for a
  benefit (the tarball and its sha256) that the fallback above keeps open.
