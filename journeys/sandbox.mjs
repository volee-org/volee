// The repository the journeys own: created, reset and checked here, apart from lib.mjs so that
// sandbox.test.mjs can exercise it without an installed volee. See README.md.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const DEFAULT_SANDBOX = path.join(os.homedir(), "Developer/Labs/cockpit-sandbox/volee-journeys");
/** Written in the repository the journeys own: a directory without it is never reset. */
export const MARKER = ".volee-journeys";
export const SEED_FILES = {
  "README.md": "# volee-journeys\n\nThrowaway repository owned by the volee journeys. Reset at the start of every run.\n",
  "src/index.ts": "export const hello = (name: string) => `hello ${name}`;\n",
};

const git = (cwd, ...args) =>
  execFileSync("git", ["-c", "user.name=volee journeys", "-c", "user.email=journeys@volee.invalid", "-c", "commit.gpgsign=false", ...args], {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();

/** Creates the repository when it is missing, or puts it back to its seed commit. Returns the directory. */
export function resetSandbox(dir) {
  if (fs.existsSync(dir)) {
    const entries = fs.readdirSync(dir);
    if (entries.length > 0 && !entries.includes(MARKER)) {
      throw new Error(`${dir} exists and is not a journeys repository (no ${MARKER}): refusing to reset it`);
    }
    // Keep the directory itself (Claude Code's trust is keyed on its path), drop everything in it.
    for (const entry of entries) fs.rmSync(path.join(dir, entry), { recursive: true, force: true });
  }
  fs.mkdirSync(dir, { recursive: true });
  git(dir, "init", "-q", "-b", "main");
  fs.writeFileSync(path.join(dir, MARKER), "owned by volee journeys/run.mjs\n");
  fs.writeFileSync(path.join(dir, ".gitignore"), ".claude/\n");
  for (const [file, text] of Object.entries(SEED_FILES)) {
    fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    fs.writeFileSync(path.join(dir, file), text);
  }
  git(dir, "add", "-A");
  git(dir, "commit", "-q", "-m", "chore: seed the journeys repository");
  return dir;
}

export function claudeJsonPath() {
  return process.env.VOLEE_JOURNEY_CLAUDE_JSON ?? path.join(os.homedir(), ".claude.json");
}

/** True when Claude Code has been told to trust exactly this directory. Reads, never writes. */
export function isTrusted(dir, file = claudeJsonPath()) {
  let projects;
  try {
    projects = JSON.parse(fs.readFileSync(file, "utf8")).projects ?? {};
  } catch {
    return false;
  }
  const candidates = new Set([dir, path.resolve(dir)]);
  try {
    candidates.add(fs.realpathSync(dir));
  } catch {}
  return [...candidates].some((candidate) => projects[candidate]?.hasTrustDialogAccepted === true);
}

export const untrustedMessage = (dir) =>
  `Claude Code does not trust ${dir} yet.\nRun \`claude\` once in that directory, accept the trust question, quit, then run the journeys again:\n  cd ${dir} && claude`;
