// Run with: node --test journeys/sandbox.test.mjs  (no Claude, no volee needed)
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { resetSandbox, isTrusted, MARKER, SEED_FILES } from "./sandbox.mjs";

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), "volee-journeys-test-"));
const g = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();

test("creates the repository when missing", () => {
  const dir = path.join(tmp(), "repo");
  resetSandbox(dir);
  assert.equal(g(dir, "branch", "--show-current"), "main");
  assert.equal(g(dir, "rev-list", "--count", "HEAD"), "1");
  for (const file of Object.keys(SEED_FILES)) assert.ok(fs.existsSync(path.join(dir, file)));
  assert.equal(g(dir, "status", "--porcelain"), "");
});

test("reset drops branches, commits, worktrees and stray files", () => {
  const dir = path.join(tmp(), "repo");
  resetSandbox(dir);
  g(dir, "branch", "journey/base");
  g(dir, "worktree", "add", "-q", path.join(dir, ".claude/worktrees/j"), "-b", "journey/j");
  fs.writeFileSync(path.join(dir, "stray.txt"), "x");
  fs.writeFileSync(path.join(dir, "README.md"), "changed");
  resetSandbox(dir);
  assert.equal(g(dir, "branch", "--format=%(refname:short)"), "main");
  assert.equal(g(dir, "worktree", "list").split("\n").length, 1);
  assert.ok(!fs.existsSync(path.join(dir, "stray.txt")));
  assert.equal(fs.readFileSync(path.join(dir, "README.md"), "utf8"), SEED_FILES["README.md"]);
  assert.equal(g(dir, "log", "--format=%s"), "chore: seed the journeys repository");
});

test("refuses to reset a directory it does not own", () => {
  const dir = tmp();
  fs.writeFileSync(path.join(dir, "precious.txt"), "keep");
  assert.throws(() => resetSandbox(dir), /refusing/);
  assert.ok(fs.existsSync(path.join(dir, "precious.txt")));
});

test("resets an empty existing directory", () => {
  const dir = tmp();
  resetSandbox(dir);
  assert.ok(fs.existsSync(path.join(dir, MARKER)));
});

test("trust is read from the Claude Code state file, exact path only", () => {
  const home = tmp();
  const file = path.join(home, "claude.json");
  const dir = path.join(home, "repo");
  fs.mkdirSync(dir);
  assert.equal(isTrusted(dir, file), false); // no file
  fs.writeFileSync(file, JSON.stringify({ projects: { [fs.realpathSync(dir)]: { hasTrustDialogAccepted: false } } }));
  assert.equal(isTrusted(dir, file), false);
  fs.writeFileSync(file, JSON.stringify({ projects: { [fs.realpathSync(home)]: { hasTrustDialogAccepted: true } } }));
  assert.equal(isTrusted(dir, file), false); // a parent does not count
  fs.writeFileSync(file, JSON.stringify({ projects: { [fs.realpathSync(dir)]: { hasTrustDialogAccepted: true } } }));
  assert.equal(isTrusted(dir, file), true);
  fs.writeFileSync(file, "not json");
  assert.equal(isTrusted(dir, file), false);
});
