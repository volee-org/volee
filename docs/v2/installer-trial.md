# Installer trial (step 10, 2026-11-22)

Goal: a person installs volee v2 and reaches the inbox in a few commands,
tested from a local artifact only (no GitHub release download, no `curl | sh`
against a public URL), isolated from the 0.18 install of this machine.

## Method

- `dist plan` passes; `dist generate --check` reports nothing to regenerate.
  The Release workflow already builds the web interface before cargo
  (`dist-workspace.toml` `github-build-setup`, `.github/build-setup.yml`,
  `VOLEE_REQUIRE_UI=1`).
- `scripts/install-local.sh` (see RELEASING.md): `dist build
  --artifacts=local --target aarch64-apple-darwin` and `--artifacts=global`,
  SHA-256 check of the archive, `python3 -m http.server` on 127.0.0.1, then the
  real dist installer with `VOLEE_CLI_DOWNLOAD_URL`, `VOLEE_CLI_INSTALL_DIR`
  and `VOLEE_CLI_NO_MODIFY_PATH=1`. The dist script was chosen over a custom
  one because it already has these overrides, so the tested file is the
  released file.
- Prefix `~/.local/volee-v2/bin`. Config, state and data in
  `~/.local/volee-v2/{config,state,data}` (`VOLEE_CONFIG_DIR`,
  `VOLEE_STATE_DIR`, `VOLEE_DATA_DIR`, `VOLEE_SOCKET`), `port = 47911` in
  `config.lua`. `~/.local/bin/volee`, `~/.local/state/volee` and
  `~/.config/volee` (0.18) were not touched.

## What ran

1. Installer: `downloading volee-cli 0.18.0 aarch64-apple-darwin`, `installing
   to ~/.local/volee-v2/bin`, `volee --version` prints `volee 0.18.0` (the
   version number moves in the release pull request, not here). The installer
   prints `no checksums to verify`: generated outside CI it has no archive
   manifest, so the script checks the SHA-256 itself (`OK`).
2. `volee config init`, port set, `volee config check`: valid.
3. `volee doctor --project ~/Developer/Labs/cockpit-sandbox/alpha`:

   ```text
   Checks
     ok       claude 2.1.330 (Claude Code)
     ok       git 2.56.0
     ok       gh 2.103.3 (2026-10-26)
     ok       configuration ~/.local/volee-v2/config/config.lua
   Wrote volee's hooks to .../alpha/.claude/settings.local.json
   ...
     daemon   not answering on 127.0.0.1:47911 (Connection refused). Start it with `volee daemon start`...
   ```

4. `volee daemon start`, `volee daemon status` (claude-code 2.1.330 available),
   `volee doctor` again: `hook server answers on 127.0.0.1:47911`.
5. Real sessions in alpha: `claude --model haiku -p` (6 hook events stored, in
   order), then `claude --model haiku --bg` asking an `AskUserQuestion`. The
   inbox shows its card: `installer-1-inbox.png` (Playwright, Chrome channel,
   element `main` only).
6. Uninstall as the guide says: `claude stop`, `volee doctor --remove
   --project alpha` (17 entries removed, file back to `{}` as before),
   `volee daemon stop`, `volee daemon uninstall` (was not installed), port
   closed.

## Left behind

The permission layer refused the `rm` of the binary and of the three directories
under `~/.local/volee-v2`, so they remain: `bin/volee`, `config/config.lua`,
`state/{daemon.lock,hook-token,volee.db,logs/daemon.log}`, empty `data/`.
Remove with `rm -rf ~/.local/volee-v2`. Outside it: Claude Code's own records
of the two trial sessions (not volee's).
