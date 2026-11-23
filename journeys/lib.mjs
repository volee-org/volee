// Shared pieces of the journeys: see README.md in this directory.
//
// A journey does what a person does with volee, for real: the INSTALLED binary
// (scripts/install-local.sh), an isolated daemon, the real web interface driven
// by Playwright (Chrome channel), real `claude --model haiku` sessions in a
// sandbox repository. Delivery is proved by git, by the session's transcript and
// by volee's own stored hook events, never by searching words in what an agent
// says. Nothing here is imported by volee, and nothing here runs in CI.

import { spawnSync, execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { DEFAULT_SANDBOX, resetSandbox, isTrusted, untrustedMessage } from "./sandbox.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
export const REPO = path.resolve(here, "..");
export const SHOTS = path.join(REPO, "docs/v2/screenshots");

// Where the isolated install lives. Nothing here is ever ~/.local/bin or the
// 0.18 state: that install is somebody else's.
export const PREFIX = process.env.VOLEE_JOURNEY_HOME ?? path.join(os.homedir(), ".local/volee-v2");
export const BIN = path.join(PREFIX, "bin/volee");
export const PORT = Number(process.env.VOLEE_JOURNEY_PORT ?? 47911);
// The repository the journeys own (sandbox.mjs): never alpha, beta or gamma, which other tools use.
export const SANDBOX = process.env.VOLEE_JOURNEY_SANDBOX ?? DEFAULT_SANDBOX;
export const MODEL = process.env.VOLEE_JOURNEY_MODEL ?? "haiku";
export const BASE = "journey/base";

// What a Claude Code session leaves in the environment of its children: a nested
// session must not inherit it.
const SESSION_VARIABLES = `CLAUDECODE CLAUDE_CODE_AGENT CLAUDE_CODE_BRIDGE_SESSION_ID
CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_ENTRYPOINT CLAUDE_CODE_EXECPATH
CLAUDE_CODE_HOST_SHORT_TEMP_DIRS CLAUDE_CODE_MESSAGING_SOCKET CLAUDE_CODE_MESSAGING_TOKEN
CLAUDE_CODE_SESSION_ATTENDED CLAUDE_CODE_SESSION_ID CLAUDE_EFFORT CLAUDE_JOB_DIR CLAUDE_PID
CLAUDE_ENV_FILE`.split(/\s+/);

export function voleeEnv() {
  const env = { ...process.env };
  for (const name of SESSION_VARIABLES) delete env[name];
  Object.assign(env, {
    VOLEE_CONFIG_DIR: path.join(PREFIX, "config"),
    VOLEE_STATE_DIR: path.join(PREFIX, "state"),
    VOLEE_DATA_DIR: path.join(PREFIX, "data"),
    VOLEE_SOCKET: path.join(PREFIX, "state/volee.sock"),
    PATH: `${path.join(PREFIX, "bin")}:${process.env.PATH}`,
  });
  return env;
}

export const configDir = () => path.join(PREFIX, "config");
export const stateDir = () => path.join(PREFIX, "state");
export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Runs a program, returns {code, out, err}. Never throws. */
export function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    env: options.env ?? voleeEnv(),
    cwd: options.cwd,
    input: options.input,
    timeout: options.timeout ?? 120_000,
  });
  return { code: result.status ?? -1, out: result.stdout ?? "", err: result.stderr ?? "" };
}

export const volee = (args, options = {}) => run(BIN, args, options);
export const git = (cwd, ...args) => {
  const tolerate = args.includes("--tolerate");
  const result = run("git", args.filter((arg) => arg !== "--tolerate"), { cwd });
  if (result.code !== 0 && !tolerate) throw new Error(`git ${args.join(" ")} in ${cwd}: ${result.err}`);
  return result.out.trim();
};

// --- checks ----------------------------------------------------------------

export class Journey {
  constructor(name) {
    this.name = name;
    this.passed = 0;
    this.failed = 0;
    this.shots = 0;
    this.cleanups = [];
    console.log(`\n=== journey ${name}`);
  }
  check(label, ok, detail = "") {
    if (ok) this.passed += 1;
    else this.failed += 1;
    console.log(`  ${ok ? "PASS" : "FAIL"}  ${label}${!ok && detail ? `  [${detail}]` : ""}`);
    return ok;
  }
  /** Runs `fn` last, whatever happens. */
  later(fn) {
    this.cleanups.unshift(fn);
  }
  async finish() {
    for (const fn of this.cleanups) {
      try {
        await fn();
      } catch (error) {
        console.log(`  cleanup problem: ${error.message}`);
      }
    }
    console.log(`  -> ${this.passed}/${this.passed + this.failed} checks`);
    return { name: this.name, passed: this.passed, failed: this.failed };
  }
}

/** Polls `fn` until it returns something truthy, or `seconds` pass. */
export async function until(fn, seconds = 60, every = 1000) {
  const end = Date.now() + seconds * 1000;
  for (;;) {
    let value;
    try {
      value = await fn();
    } catch {
      value = false;
    }
    if (value) return value;
    if (Date.now() > end) return false;
    await sleep(every);
  }
}

// --- the isolated install ----------------------------------------------------

export function ensureConfig() {
  fs.mkdirSync(configDir(), { recursive: true });
  const file = path.join(configDir(), "config.lua");
  if (!fs.existsSync(file)) volee(["config", "init"]);
  else if (volee(["config", "check"]).code !== 0) {
    // A file written by an older build can name keys this build no longer reads: start from the
    // example of the installed binary. The prefix is the journeys' own.
    const fresh = fs.mkdtempSync(path.join(PREFIX, "fresh-"));
    volee(["config", "init"], { env: { ...voleeEnv(), VOLEE_CONFIG_DIR: fresh } });
    fs.copyFileSync(path.join(fresh, "config.lua"), file);
    fs.rmSync(fresh, { recursive: true, force: true });
    console.log("  note  the config.lua left by an older build was rejected by this one: replaced by the new example");
  }
  let text = fs.readFileSync(file, "utf8");
  if (!new RegExp(`^\\s*port = ${PORT},`, "m").test(text)) {
    text = text.replace(/^(\s*)-- port = \d+,/m, `$1port = ${PORT},`).replace(/^(\s*)port = \d+,/m, `$1port = ${PORT},`);
    fs.writeFileSync(file, text);
  }
}

export function daemonUp() {
  return /\(pid \d+\)/.test(volee(["daemon", "status"]).out);
}
export function startDaemon() {
  ensureConfig();
  volee(["daemon", "start"]);
}
export function stopDaemon() {
  volee(["daemon", "stop"]);
}
export async function restartDaemon() {
  stopDaemon();
  await until(() => !daemonUp(), 15, 500);
  startDaemon();
  return until(() => daemonUp(), 20, 500);
}

/** The link `volee ui` prints, token in its fragment. */
export function uiUrl() {
  return volee(["ui", "--no-open"]).out.trim();
}
export function token() {
  return new URLSearchParams((uiUrl().split("#")[1] ?? "").replace(/^#/, "")).get("token") ?? "";
}

export async function api(method, route, body) {
  const response = await fetch(`http://127.0.0.1:${PORT}${route}`, {
    method,
    headers: { Authorization: `Bearer ${token()}`, "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let json = null;
  try {
    json = JSON.parse(text);
  } catch {
    // not JSON
  }
  return { status: response.status, json, text };
}

/** The personal rules file of the isolated install (reloaded live by the daemon). */
export function setPersonalRules(lua) {
  fs.mkdirSync(configDir(), { recursive: true });
  fs.writeFileSync(path.join(configDir(), "rules.lua"), lua);
}
export function clearPersonalRules() {
  fs.rmSync(path.join(configDir(), "rules.lua"), { force: true });
}

// --- the sandbox repository ---------------------------------------------------

export function sandboxReady() {
  return fs.existsSync(path.join(SANDBOX, ".git"));
}
/** Creates the journeys' repository, or puts it back to its seed commit. */
export const resetOwnSandbox = () => resetSandbox(SANDBOX);
export const sandboxTrusted = () => isTrusted(SANDBOX);
export const sandboxUntrustedMessage = () => untrustedMessage(SANDBOX);

/** A throwaway base branch checked out in the sandbox: a local merge needs the base checked out and clean. */
export function prepareBase() {
  git(SANDBOX, "checkout", "-q", "-B", BASE, "main");
}
export function worktreePath(name) {
  return path.join(SANDBOX, ".claude/worktrees", name);
}
/** A worktree on journey/<name>, branched from the base. */
export function makeWorktree(name) {
  const dir = worktreePath(name);
  git(SANDBOX, "worktree", "add", "-q", "-B", `journey/${name}`, dir, BASE);
  return dir;
}
export function removeWorktree(name) {
  git(SANDBOX, "worktree", "remove", "--force", worktreePath(name), "--tolerate");
  git(SANDBOX, "branch", "-q", "-D", `journey/${name}`, "--tolerate");
}
/** Puts the sandbox back as it was: main checked out, no journey branch, no journey worktree. */
export function restoreSandbox() {
  const listing = git(SANDBOX, "worktree", "list", "--porcelain");
  for (const line of listing.split("\n")) {
    const match = line.match(/^worktree (.*\/\.claude\/worktrees\/(?:j-|journey-).*)$/);
    if (match) git(SANDBOX, "worktree", "remove", "--force", match[1], "--tolerate");
  }
  git(SANDBOX, "checkout", "-q", "-f", "main", "--tolerate");
  const branches = git(SANDBOX, "branch", "--list", "journey/*", "--format=%(refname:short)");
  for (const branch of branches.split("\n").filter(Boolean)) git(SANDBOX, "branch", "-q", "-D", branch, "--tolerate");
  git(SANDBOX, "worktree", "prune");
}
export const commitsAhead = (dir, base = BASE) => Number(git(dir, "rev-list", "--count", `${base}..HEAD`));
export const isAncestor = (dir, commit, branch) => run("git", ["merge-base", "--is-ancestor", commit, branch], { cwd: dir }).code === 0;

/** Writes volee's hooks in the project, the way a person sets a repository up. */
export function doctor(dir) {
  return volee(["doctor", "--project", dir]);
}
export function undoctor(dir) {
  return volee(["doctor", "--remove", "--project", dir]);
}

// --- real Claude Code sessions -------------------------------------------------

/** Starts a background session in `dir`; returns its short id. */
export function claudeBg(dir, name, prompt, { mode = "default", allow = null } = {}) {
  const extra = allow ? [`--allowedTools=${allow}`] : [];
  const args = ["--bg", "--model", MODEL, "--permission-mode", mode, "--name", name, ...extra, prompt];
  const result = run("claude", args, { cwd: dir, timeout: 60_000 });
  const match = (result.out + result.err).match(/claude (?:attach|stop|logs) ([0-9a-f]{8})/);
  return match ? match[1] : null;
}
export function claudeAgents() {
  const result = run("claude", ["agents", "--json", "--all"], { timeout: 30_000 });
  try {
    const parsed = JSON.parse(result.out);
    return Array.isArray(parsed) ? parsed : (parsed.agents ?? []);
  } catch {
    return [];
  }
}
export const agentById = (short) => claudeAgents().find((agent) => (agent.sessionId ?? agent.id ?? "").startsWith(short) || (agent.id ?? "").startsWith(short));
export function claudeStop(short) {
  return run("claude", ["stop", short], { timeout: 30_000 });
}
export function claudeRm(short) {
  return run("claude", ["rm", short], { timeout: 30_000 });
}

/** The sessions a run left: named journey-* and started inside the journeys' repository. Pure. */
export function leftoverSessions(agents, sandbox = SANDBOX) {
  const roots = [sandbox];
  try {
    roots.push(fs.realpathSync(sandbox));
  } catch {}
  return agents.filter(
    (agent) =>
      String(agent.name ?? "").startsWith("journey-") &&
      roots.some((root) => agent.cwd === root || String(agent.cwd ?? "").startsWith(`${root}/`)),
  );
}
/** Stops and removes every leftover session, whatever happened. Returns the ones that could not be removed. */
export function sweepSessions() {
  for (const agent of leftoverSessions(claudeAgents())) {
    const short = agent.id ?? String(agent.sessionId).slice(0, 8);
    claudeStop(short);
    claudeRm(short);
  }
  return leftoverSessions(claudeAgents());
}

/** The session's transcript, found by its full id (never by what the agent said). */
export function transcriptOf(dir, sessionId) {
  const encoded = fs.realpathSync(dir).replace(/[/.]/g, "-");
  const file = path.join(os.homedir(), ".claude/projects", encoded, `${sessionId}.jsonl`);
  if (!fs.existsSync(file)) return [];
  return fs
    .readFileSync(file, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      try {
        return JSON.parse(line);
      } catch {
        return null;
      }
    })
    .filter(Boolean);
}
/** Every content block of a transcript, flattened. */
export function blocks(transcript) {
  const out = [];
  for (const entry of transcript) {
    const content = entry.message?.content;
    if (Array.isArray(content)) for (const block of content) out.push({ role: entry.message.role, ...block });
    else if (typeof content === "string") out.push({ role: entry.message.role, type: "text", text: content });
  }
  return out;
}
export const toolUses = (transcript, name) => blocks(transcript).filter((block) => block.type === "tool_use" && block.name === name);
export const toolResults = (transcript) => blocks(transcript).filter((block) => block.type === "tool_result");

/** volee's stored hook events (what the daemon heard), as JSON lines. */
export function events() {
  const result = volee(["events"]);
  return result.out.split("\n").filter(Boolean);
}

// --- the browser -----------------------------------------------------------------

const require = createRequire(path.join(REPO, "ui/package.json"));

export async function browser() {
  const { chromium } = require("playwright");
  const instance = await chromium.launch({ channel: "chrome" });
  const page = await instance.newPage({ viewport: { width: 1100, height: 900 } });
  return { instance, page };
}

/** Screenshot of one element only, named journey-<journey>-<n>-<label>.png. */
export async function shot(journey, page, selector, label) {
  journey.shots += 1;
  fs.mkdirSync(SHOTS, { recursive: true });
  const file = path.join(SHOTS, `journey-${journey.name}-${journey.shots}-${label}.png`);
  // The screenshots are published: the home directory never shows, only `~`.
  await page.evaluate((home) => {
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.nodeValue.includes(home)) node.nodeValue = node.nodeValue.split(home).join("~");
    }
    for (const input of document.querySelectorAll("input, textarea")) {
      if (input.value.includes(home)) input.value = input.value.split(home).join("~");
    }
  }, fs.realpathSync(os.homedir()));
  await page.locator(selector).first().screenshot({ path: file });
  console.log(`  shot  ${path.relative(REPO, file)}`);
  return file;
}

/** The card of a decision or of finished work, by its branch or its text. */
export const cardWith = (page, text) => page.locator("article.card", { hasText: text });

export function fileExists(file) {
  return fs.existsSync(file);
}
export function readFile(file) {
  return fs.readFileSync(file, "utf8");
}
export function writeFile(file, text) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
}
export { fs, path, os, execFileSync };

// --- what the daemon says --------------------------------------------------------

export const inbox = async () => (await api("GET", "/api/inbox")).json ?? { decisions: [], cards: [] };
/** The held decision of the agent on `journey/<name>`, once it exists. */
export const decisionOf = (name, seconds = 90) =>
  until(async () => (await inbox()).decisions.find((decision) => decision.branch === `journey/${name}`), seconds);
/** The finished-work card of the agent on `journey/<name>`, once it exists. */
export const cardOf = (name, seconds = 150) =>
  until(async () => (await inbox()).cards.find((card) => card.kind === "finished" && card.branch === `journey/${name}`), seconds);
/** A small model sometimes checks its work with a command nobody allowed: the person allows it. Returns how many. */
export async function allowStray(name) {
  const stray = (await inbox()).decisions.filter((decision) => decision.kind === "permission" && decision.branch === `journey/${name}`);
  for (const decision of stray) {
    await api("POST", `/api/decisions/${encodeURIComponent(decision.id)}`, { action: "allow" });
    console.log(`  note  allowed a stray permission of the agent: ${decision.tool_name} ${String(decision.summary).slice(0, 60)}`);
  }
  return stray.length;
}
export const sessionIdOf =(short) => agentById(short)?.sessionId ?? null;
export const journal = async () => (await api("GET", "/api/journal")).json ?? [];

/** Whether the `cwd` of the agent shows a card of the branch in `page`. */
export const cardGone = async (name, seconds = 30) =>
  until(async () => {
    const data = await inbox();
    return !data.decisions.some((d) => d.branch === `journey/${name}`) && !data.cards.some((c) => c.branch === `journey/${name}`);
  }, seconds);

/** Everything a journey with a real agent needs and must give back. */
export async function withAgent(journey, name, prompt, options = {}) {
  const dir = makeWorktree(name);
  doctor(dir);
  const short = claudeBg(dir, `journey-${name}`, prompt, options);
  journey.check(`a real ${MODEL} session started in the worktree of journey/${name}`, !!short);
  const agent = { name, dir, short, branch: `journey/${name}` };
  journey.later(() => {
    if (short) {
      claudeStop(short);
      claudeRm(short);
    }
    undoctor(dir);
    removeWorktree(name);
  });
  agent.sessionId = await until(() => sessionIdOf(short), 20, 500);
  agent.transcript = () => (agent.sessionId ? transcriptOf(dir, agent.sessionId) : []);
  return agent;
}
