// A person installs volee, sets a repository up and opens the inbox.
import * as L from "./lib.mjs";

export default async function install(journey, ctx) {
  if (!ctx.skipInstall) {
    const built = L.run("sh", [L.path.join(L.REPO, "scripts/install-local.sh"), L.path.join(L.PREFIX, "bin")], {
      timeout: 900_000,
      cwd: L.REPO,
      env: { ...process.env },
    });
    journey.check("the dist installer, run against a local artifact, succeeds", built.code === 0, built.err.slice(-300));
    journey.check("it served the archive from 127.0.0.1 (no download from a release)", /127\.0\.0\.1|downloading volee-cli/.test(built.out + built.err));
  }
  journey.check("the installed binary exists in the isolated prefix, not in ~/.local/bin", L.fileExists(L.BIN) && !L.BIN.includes(".local/bin/"));
  const version = L.volee(["--version"]);
  journey.check("`volee --version` answers", version.code === 0 && /^volee \d+\.\d+\.\d+/.test(version.out), version.out + version.err);

  L.ensureConfig();
  const check = L.volee(["config", "check"]);
  journey.check("`volee config check` accepts the configuration", check.code === 0, check.out + check.err);

  // A person sets a repository up: hooks at project level only.
  L.restoreSandbox();
  L.prepareBase();
  const dir = L.makeWorktree("j01");
  journey.later(() => {
    L.undoctor(dir);
    L.removeWorktree("j01");
  });
  const doctor = L.doctor(dir);
  journey.check("`volee doctor` prints its checks", /Checks/.test(doctor.out) && /claude/.test(doctor.out));
  journey.check("claude, git and the configuration are reported ok", (doctor.out.match(/^\s+ok\s/gm) ?? []).length >= 3, doctor.out);
  journey.check("hooks were written in the project's settings.local.json", L.fileExists(L.path.join(dir, ".claude/settings.local.json")));
  const settings = JSON.parse(L.readFile(L.path.join(dir, ".claude/settings.local.json")));
  journey.check("PermissionRequest and PreToolUse hooks are there", !!settings.hooks?.PermissionRequest && !!settings.hooks?.PreToolUse);
  journey.check("the daemon is not running yet, doctor said so", /not answering/.test(doctor.out) || L.daemonUp());

  L.startDaemon();
  journey.check("`volee daemon start` starts the daemon", await L.until(() => L.daemonUp(), 20, 500));
  journey.check("the hook server answers on the isolated port", (await fetch(`http://127.0.0.1:${L.PORT}/healthz`).then((r) => r.text()).catch(() => "")) === "ok");
  const again = L.doctor(dir);
  journey.check("doctor now sees the daemon", /hook server answers/.test(again.out));

  // A person opens the inbox.
  const link = L.uiUrl();
  journey.check("`volee ui` prints a link with the token in its fragment", /^http:\/\/127\.0\.0\.1:\d+\/#token=/.test(link), link);
  await ctx.page.goto(link);
  await ctx.page.waitForSelector("main", { timeout: 15000 });
  const live = await L.until(async () => /live/.test(await ctx.page.locator("header, main").first().innerText()), 15, 500);
  journey.check("the page loads from the binary (embedded interface, not the stub) and says live", live);
  const stub = await ctx.page.content();
  journey.check("the bundled interface is served, not the 'not built' page", !/not built/i.test(stub));
  await L.shot(journey, ctx.page, "main", "inbox-after-install");

  // Claude Code must not have been touched by volee.
  journey.check("a user-wide ~/.claude/settings.json hook entry was not added", !/127\.0\.0\.1:\d+\/hooks/.test(L.readFile(L.path.join(L.os.homedir(), ".claude/settings.json"))));
}
