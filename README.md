<div align="center">

# volee

**See, launch and coordinate every coding agent on your machine, from your terminal.**

[![CI](https://github.com/volee-org/volee/actions/workflows/ci.yml/badge.svg)](https://github.com/volee-org/volee/actions/workflows/ci.yml)
[![Docs](https://img.shields.io/badge/docs-volee--org.github.io-8839ef)](https://volee-org.github.io/volee/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)
[![MSRV](https://img.shields.io/badge/MSRV-1.89-orange)](https://github.com/volee-org/volee/blob/main/Cargo.toml)

</div>

> [!WARNING]
> volee is in early development. The first release (0.1) is being built in the
> open: see the [roadmap](https://volee-org.github.io/volee/project/roadmap.html).

## Why

You run five Claude Code sessions in five worktrees. One waits for a
permission, one finished an hour ago, two are talking to each other through a
coordinator, and you are not sure which branch was pushed. volee answers, in
one screen:

- **Who is running?** Every agent on the machine, with its state, repository,
  branch and worktree, whether you started it or another agent did.
- **Who needs me?** Permission prompts, questions and finished work, sorted by
  urgency.
- **What did they do?** Commits, diffs, push state, pull request and checks.
- **Who talks to whom?** Messages between agents and with you, as threads and
  as a live graph.

It also runs **missions**: a goal across one or more repositories, split by a
coordinator agent and delivered by worker agents, with delivery checked by
code, not by trust.

## How it works

- Agents run as Claude Code background sessions, so they survive closed
  windows. Take one over with a key, come back with `Ctrl+Z`.
- A small local daemon keeps the registry, the event log and the messages in
  SQLite. The interface, the command line and the agents all talk to it.
- Agents message each other through an MCP server, which any MCP-capable
  harness can use.
- No multiplexer, editor or git client is required; the ones you use are
  optional integrations.

Read the [architecture overview](https://volee-org.github.io/volee/architecture/overview.html)
and the [design decisions](https://volee-org.github.io/volee/adr/README.html).

## Install

Prebuilt binaries and a Homebrew formula come with the first release. Until
then, build from source with a recent stable Rust toolchain:

```sh
cargo install --git https://github.com/volee-org/volee volee-cli
```

volee supports macOS and Linux and needs [Claude Code](https://code.claude.com)
2.1.300 or later for its agent features.

## Contributing

Contributions are welcome, including ones written with coding agents. Start
with [CONTRIBUTING.md](CONTRIBUTING.md); agents read [AGENTS.md](AGENTS.md).
By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
