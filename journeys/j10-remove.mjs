// A person takes volee out: hooks removed from the project, daemon stopped, nothing left running.
import * as L from "./lib.mjs";

export default async function remove(journey) {
  L.restoreSandbox();
  const file = L.path.join(L.SANDBOX, ".claude/settings.local.json");
  const original = L.fileExists(file) ? L.readFile(file) : null;
  const userSettings = L.path.join(L.os.homedir(), ".claude/settings.json");
  const userBefore = L.readFile(userSettings);

  const set = L.doctor(L.SANDBOX);
  journey.check("doctor sets the sandbox repository up", /Wrote volee's hooks/.test(set.out) && /PermissionRequest/.test(L.readFile(file)));
  const removed = L.undoctor(L.SANDBOX);
  journey.check("`volee doctor --remove` says how many entries it removed", /Removed \d+ volee hook entries/.test(removed.out), removed.out);
  journey.check("no hook of volee is left in the project", !/127\.0\.0\.1:\d+\/(hooks|decide)/.test(L.fileExists(file) ? L.readFile(file) : ""));
  journey.check("the project's settings file is back to what it was", (L.fileExists(file) ? L.readFile(file).trim() : null) === (original === null ? null : original.trim()), L.readFile(file));
  journey.check("~/.claude/settings.json was not changed by this journey", L.readFile(userSettings) === userBefore);
  journey.check("no journey worktree or branch is left in the sandbox", !/journey/.test(L.git(L.SANDBOX, "worktree", "list") + L.git(L.SANDBOX, "branch", "--list")));

  const stopped = L.volee(["daemon", "stop"]);
  journey.check("`volee daemon stop` stops the daemon", /Stopped/.test(stopped.out), stopped.out + stopped.err);
  journey.check("the hook port is closed", await L.until(async () => !(await fetch(`http://127.0.0.1:${L.PORT}/healthz`).then((r) => r.ok).catch(() => false)), 15, 500));
  const login = L.volee(["daemon", "uninstall"]);
  journey.check("`volee daemon uninstall` says it never started at login", /does not start at login|Stopped starting/.test(login.out), login.out);
}
