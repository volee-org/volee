#!/usr/bin/env node
// Runs the journeys, in order, against the INSTALLED volee. See README.md.
//
//   node journeys/run.mjs                 every journey, installing first
//   node journeys/run.mjs --skip-install  reuse the install already in the prefix
//   node journeys/run.mjs j03 j05         only these (names start with these prefixes)
//
// Exit code 0 only when every check passed.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as L from "./lib.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const skipInstall = argv.includes("--skip-install");
const wanted = argv.filter((arg) => !arg.startsWith("--"));
const files = fs
  .readdirSync(here)
  .filter((file) => /^j\d\d-.*\.mjs$/.test(file))
  .sort()
  .filter((file) => wanted.length === 0 || wanted.some((prefix) => file.startsWith(prefix)));

// The journeys own their repository: created when missing, reset to its seed commit at the start.
try {
  L.resetOwnSandbox();
} catch (error) {
  console.log(error.message);
  process.exit(2);
}
// Claude Code asks about workspace trust once per directory. Never edit ~/.claude.json: ask the person.
if (!L.sandboxTrusted()) {
  console.log(L.sandboxUntrustedMessage());
  process.exit(2);
}

const results = [];
let session = null;
try {
  for (const file of files) {
    if (!session && file !== "j01-install.mjs") {
      // Every journey but the first one needs the isolated daemon running.
      if (!L.daemonUp()) L.startDaemon();
      await L.until(() => L.daemonUp(), 20, 500);
    }
    session ??= await L.browser();
    const module = await import(path.join(here, file));
    const journey = new L.Journey(file.replace(/^j\d\d-/, "").replace(/\.mjs$/, ""));
    try {
      await module.default(journey, { page: session.page, skipInstall });
    } catch (error) {
      journey.check("the journey ran to its end", false, error.stack?.split("\n").slice(0, 3).join(" | "));
    }
    results.push(await journey.finish());
  }
} finally {
  // Always, on success or failure: browser closed, sessions stopped and removed, worktrees and journey
  // branches deleted, the repository back on main.
  try {
    await session?.instance.close();
  } catch {}
  let left = [];
  try {
    left = L.sweepSessions();
  } catch (error) {
    console.log(`cleanup problem (sessions): ${error.message}`);
  }
  try {
    L.restoreSandbox();
  } catch (error) {
    console.log(`cleanup problem (repository): ${error.message}`);
  }
  for (const agent of left) console.log(`left behind, remove by hand: claude rm ${agent.id}  (${agent.name}, ${agent.state})`);
}

const passed = results.reduce((sum, r) => sum + r.passed, 0);
const total = results.reduce((sum, r) => sum + r.passed + r.failed, 0);
console.log("\n=== summary");
for (const r of results) console.log(`  ${r.failed === 0 ? "ok  " : "FAIL"}  ${r.name}: ${r.passed}/${r.passed + r.failed}`);
console.log(`  ${passed}/${total} checks`);
process.exit(passed === total && total > 0 ? 0 : 1);
