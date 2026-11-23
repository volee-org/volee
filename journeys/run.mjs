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

if (!L.sandboxReady()) {
  console.log(`sandbox ${L.SANDBOX} is not a git repository`);
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
  await session?.instance.close();
  L.restoreSandbox();
}

const passed = results.reduce((sum, r) => sum + r.passed, 0);
const total = results.reduce((sum, r) => sum + r.passed + r.failed, 0);
console.log("\n=== summary");
for (const r of results) console.log(`  ${r.failed === 0 ? "ok  " : "FAIL"}  ${r.name}: ${r.passed}/${r.passed + r.failed}`);
console.log(`  ${passed}/${total} checks`);
process.exit(passed === total && total > 0 ? 0 : 1);
