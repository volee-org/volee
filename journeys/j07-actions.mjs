// What a person does with a finished change: open a file in their editor, comment a line,
// ask for a fix (the live agent takes it), merge in one click.
import * as L from "./lib.mjs";
import { finishedWork, openWork } from "./work.mjs";

export default async function actions(journey, ctx) {
  L.restoreSandbox();
  L.prepareBase();
  // A harmless stand-in editor: it only records how it was called.
  const standin = L.path.join(L.PREFIX, "standin.sh");
  const log = L.path.join(L.PREFIX, "standin.log");
  L.fs.rmSync(log, { force: true });
  L.writeFile(standin, `#!/bin/sh\necho "$@" >> '${log}'\n`);
  L.fs.chmodSync(standin, 0o755);
  L.setPersonalRules(`return {\n  sensitive_paths = { "src/billing/**" },\n  editors = { { name = "standin", kind = "external", cmd = "${standin} --line {line} {file}" } },\n}\n`);
  journey.later(() => {
    L.clearPersonalRules();
    L.fs.rmSync(standin, { force: true });
    L.fs.rmSync(log, { force: true });
  });
  const page = ctx.page;
  await page.goto(L.uiUrl());

  const agent = await finishedWork(journey, "j07");
  await openWork(page, agent.card);
  journey.check("proof in git: one commit so far", agent.commits() === 1);
  const diffLayer = page.locator('[data-layer="diff"] .head, [data-layer="files"] .head');
  for (const id of ["diff", "files"]) {
    const layer = page.locator(`[data-layer="${id}"]`);
    if ((await layer.count()) && (await layer.getAttribute("data-open")) !== "true") await layer.locator(".head").click();
  }
  void diffLayer;

  // --- open in an external editor ---
  const header = page.locator('article.file[data-path="docs/notes.md"]');
  await header.waitFor({ timeout: 20000 });
  await header.locator('[data-testid="file-editors"] button', { hasText: "Open in standin" }).click();
  const called = await L.until(() => L.fileExists(log) && L.readFile(log).trim(), 20, 500);
  const expectedFile = L.path.join(L.fs.realpathSync(agent.dir), "docs/notes.md");
  journey.check("the stand-in editor was run with the line and the absolute path of the file in the worktree", !!called && called.split("\n").some((l) => l.startsWith("--line 1 ") && l.endsWith("docs/notes.md") && L.fs.realpathSync(l.split(" ").slice(2).join(" ")) === expectedFile), String(called));
  await L.shot(journey, page, '[data-testid="actions"]', "after-open-in-editor");

  // --- comment a line ---
  await header.locator("button.head").click().catch(() => {});
  if ((await header.getAttribute("data-open")) !== "true") await header.locator("button.head").click();
  // After an editor was opened the screen reads the work again every few seconds and may redraw
  // the diff under the form: the person simply tries again.
  let shown = false;
  for (let attempt = 1; attempt <= 3 && !shown; attempt += 1) {
    const lineCell = header.locator('tr:has(.line-num2:text-is("3")) td.d2h-code-linenumber').first();
    await lineCell.waitFor({ timeout: 20000 });
    await lineCell.click();
    const box = page.locator('textarea[aria-label="Comment on line 3"]');
    await box.waitFor({ timeout: 10000 });
    await box.fill("journey-comment: add a fourth line after this one");
    if (attempt === 1) await L.shot(journey, page, 'article.file[data-path="docs/notes.md"]', "comment-form");
    await page.locator(".vc-save").click({ timeout: 5000 }).catch(() => {});
    shown = await L.until(async () => (await page.locator(".vc-text").count()) === 1, 6, 300);
  }
  journey.check("the comment shows under its line", shown);
  const saved = (await L.api("GET", `/api/agents/${encodeURIComponent(agent.card.id)}/comments`)).json;
  journey.check("the daemon stored it for this branch tip, line 3 of docs/notes.md", JSON.stringify(saved).includes("docs/notes.md") && JSON.stringify(saved).includes("journey-comment"), JSON.stringify(saved)?.slice(0, 160));

  // --- ask for a fix: the agent is alive, so the message goes over its socket ---
  await page.locator('[data-testid="ask-fix"]').click();
  const area = page.locator('textarea[aria-label="The request, as the agent will read it"]');
  await area.waitFor({ timeout: 10000 });
  const prefilled = await area.inputValue();
  journey.check("the request is prefilled from the comment", prefilled.includes("journey-comment"), prefilled.slice(0, 120));
  await area.fill(`${prefilled}\n\nDo it now and do not ask me anything: append a fourth line containing four to docs/notes.md, then run git add docs and git commit -m "fix: add line four".`);
  await page.locator("button.send").click();
  const result = page.locator('[data-testid="fix-result"]');
  await result.waitFor({ timeout: 30000 });
  const said = await result.innerText();
  journey.check("volee says the fix request was delivered over the live socket", /delivered/i.test(said) && /socket/i.test(said), said);
  await L.shot(journey, page, '[data-testid="actions"]', "fix-delivered");
  const committed = await L.until(async () => {
    await L.allowStray("j07");
    return agent.commits() === 2;
  }, 150, 2000);
  if (!committed) {
    console.log("  debug", JSON.stringify(L.agentById(agent.short)), JSON.stringify((await L.inbox()).decisions.map((d) => [d.kind, d.branch, d.summary])));
    console.log(L.events().slice(-8).join("\n"));
  }
  journey.check("proof in git: the agent made a second commit", committed, `commits: ${agent.commits()}`);
  const notes = L.git(agent.dir, "show", "HEAD:docs/notes.md").split("\n");
  journey.check("proof in git: docs/notes.md now ends with the fourth line", notes.length === 4 && notes[3] === "four", notes.join("|"));
  journey.check("proof in the hook events: the message reached the agent as a prompt, signed by volee", L.events().some((l) => /UserPromptSubmit/.test(l) && /Message from the person/.test(l) && l.includes(agent.sessionId.slice(0, 8))));

  // --- merge in one click (the change is low risk) ---
  const tip = L.git(agent.dir, "rev-parse", "HEAD");
  await page.reload();
  await page.waitForSelector('[data-testid="actions"]', { timeout: 30000 });
  const merge = page.locator('[data-testid="merge"]');
  // The work screen reads the agent's state when it opens: after a fix, a reload shows it finished.
  const enabled = await L.until(async () => {
    await L.allowStray("j07");
    if (!(await merge.count()) || (await merge.isDisabled())) {
      await page.reload();
      await page.waitForSelector('[data-testid="actions"]', { timeout: 30000 }).catch(() => {});
    }
    return (await merge.count()) && !(await merge.isDisabled());
  }, 150, 4000);
  journey.check("the merge button is enabled once the agent has finished", !!enabled);
  journey.check("the merge button names the base", /Merge into journey\/base/.test(await merge.innerText()));
  await merge.click({ timeout: 10000 });
  const message = page.locator('[data-testid="action-message"]');
  await message.waitFor({ timeout: 60000 });
  const merged = await message.innerText();
  journey.check("volee reports the merge with its evidence", /merged/i.test(merged) && /evidence/i.test(merged), merged);
  journey.check("proof in git: the agent's tip is now part of journey/base", L.isAncestor(L.SANDBOX, tip, L.BASE));
  journey.check("proof in git: the base branch has the merged docs/notes.md with its fourth line", L.git(L.SANDBOX, "show", `${L.BASE}:docs/notes.md`).split("\n")[3] === "four");
  await L.shot(journey, page, '[data-testid="actions"]', "merged");
  await page.locator("button.back").click();
  await page.waitForSelector('[data-testid="journal"]', { timeout: 20000 });
  journey.check("the journal records the merge", /merged/i.test(await page.locator('[data-testid="journal"]').innerText()));
  journey.check("the card is gone", await L.until(async () => !(await L.inbox()).cards.some((c) => c.id === agent.card.id), 20));
  await L.shot(journey, page, '[data-testid="journal"]', "journal-after-merge");
}
