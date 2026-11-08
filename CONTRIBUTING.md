# Contributing to volee

Thanks for your interest in volee. It is a terminal tool (Rust, ratatui TUI, a
local daemon and an MCP server) to see every coding agent running on your
machine and to run multi-repo missions. It is in early development, so there is
plenty to do and your help is welcome.

By taking part, you agree to follow our [Code of Conduct](CODE_OF_CONDUCT.md).
How decisions are made is described in [GOVERNANCE.md](GOVERNANCE.md).

## Ways to contribute

- Report a bug or suggest a feature with an [issue](https://github.com/volee-org/volee/issues/new/choose).
- Ask a question or share an idea in [Discussions](https://github.com/volee-org/volee/discussions).
- Improve the documentation (the site is at https://volee-org.github.io/volee/).
- Fix a bug or implement a feature. For anything big, open an issue first so we
  can agree on the approach before you spend time on it.
- Review pull requests and try early builds.

Security problems must not be reported in public issues: see [SECURITY.md](SECURITY.md).

## Development setup

volee supports macOS and Linux.

1. Install Rust with [rustup](https://rustup.rs). The stable toolchain is pinned
   by `rust-toolchain.toml`, so rustup picks the right version automatically.
   The minimum supported Rust version (MSRV) is declared in `Cargo.toml`.
2. Install [Zig](https://ziglang.org) **0.15.2** exactly. The terminal
   emulator, libghostty-vt, is built from a pinned Ghostty commit that needs
   this version ([ADR 0020](docs/src/adr/0020-libghostty-vt-emulates-terminals.md)).
   On macOS, use Homebrew: `brew install zig@0.15` installs it outside your
   `PATH`, so put `$(brew --prefix zig@0.15)/bin` first in the `PATH` of the
   shell you build from. The archive from ziglang.org fails to link against
   the macOS 26 SDK (seen in CI); Homebrew's build works on macOS 27. On
   Linux, take the 0.15.2 archive from ziglang.org or use a version manager. The first build also needs network access to github.com to fetch
   Ghostty, and takes about a minute and a half longer.
3. Clone the repository and build:

   ```sh
   git clone https://github.com/volee-org/volee
   cd volee
   cargo build --workspace
   ```

4. Optional tools (install the ones you need):

   | Tool | Used for |
   | --- | --- |
   | [just](https://github.com/casey/just) | Shortcut recipes. Optional: every recipe is a plain cargo command listed in the `justfile`. |
   | [cargo-nextest](https://nexte.st) | Faster test runner |
   | [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) | License and advisory checks |
   | [typos](https://github.com/crate-ci/typos) | Spell checking |
   | [cargo-insta](https://insta.rs) | Reviewing snapshot changes |
   | [mdBook](https://rust-lang.github.io/mdBook/) and mdbook-mermaid | Building the docs |
   | [vhs](https://github.com/charmbracelet/vhs) | Recording demos of the TUI |

## Repository map

| Path | Content |
| --- | --- |
| `crates/` | The Rust crates of the workspace |
| `docs/` | The mdBook documentation site |
| `docs/src/adr/` | Architecture decision records |
| `AGENTS.md`, `CLAUDE.md` | Instructions for coding agents |
| `RELEASING.md` | How maintainers cut a release |

## Checks

Run this before you push:

```sh
just check
```

It runs formatting, clippy and the tests. Without `just`, run the commands
directly:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace
```

Other useful commands:

```sh
cargo deny check      # licenses, advisories, duplicate dependencies
typos                 # spelling
mdbook serve docs     # preview the documentation locally
cargo insta review    # review changed snapshots
```

## Testing guidelines

- Put unit tests next to the code they test.
- Test TUI rendering with [insta](https://insta.rs) snapshots. Review changes
  with `cargo insta review` and commit the updated `.snap` files.
- Test git code against real temporary git repositories, not mocks.
- Tests must not use the network.
- A bug fix should come with a test that fails without the fix.

## Commits and pull requests

We use [Conventional Commits](https://www.conventionalcommits.org):
`<type>(<scope>): <description>`.

- Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`,
  `ci`, `chore`, `revert`.
- Scopes are crate names without the `volee-` prefix, for example
  `feat(tui): add a diff pane`.

Pull requests are squash-merged and the PR title becomes the commit message, so
the PR title must follow Conventional Commits. The commits inside your branch
can be as messy as you like. The title is also the entry of your change in the
changelog, so describe the change for users: `fix(daemon): keep messages to a
stopped agent`, not `fix: address review`.

Keep pull requests focused: one change per PR is easier to review. Fill in the
PR template, and attach a screenshot or a vhs recording for UI changes.

Maintainers cut releases with a release pull request and a tag, as described
in [RELEASING.md](RELEASING.md): git-cliff writes `CHANGELOG.md` from the PR
titles, and dist builds the binaries. Do not edit `CHANGELOG.md` or version
numbers in your pull request.

## Proposing a design change

Significant decisions are recorded as ADRs (architecture decision records) in
`docs/src/adr`, in MADR style. To propose one, copy `docs/src/adr/template.md`,
fill it in and open a pull request. Discussion happens in that PR. If your code
change reverses or replaces an earlier decision, add or update the ADR in the
same PR.

## Documentation changes

Docs live in `docs/src` and are built with mdBook. Run `mdbook serve docs` to
preview your changes, and run `typos` before you push. Small fixes are welcome
as direct pull requests.

## Working with coding agents

AI-assisted contributions are welcome. You stay responsible for what you
submit: read the code, run the checks and be able to explain every change.
`AGENTS.md` is the source of truth for coding agents (`CLAUDE.md` points to
it). Review agent output like any other code, and do not submit changes you do
not understand.

## License of contributions

volee is licensed under MIT OR Apache-2.0. Contributions are accepted under the
same terms (inbound = outbound).

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
