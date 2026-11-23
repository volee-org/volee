// Finished work is read in layers, with the AI pre-review as an opinion.
import * as L from "./lib.mjs";
import { finishedWork, openWork, layerOpen } from "./work.mjs";

export default async function work(journey, ctx) {
  L.restoreSandbox();
  L.prepareBase();
  // High risk on src/auth/**: every layer opens.
  L.setPersonalRules(`return { sensitive_paths = { "src/auth/**" } }\n`);
  journey.later(() => L.clearPersonalRules());
  await ctx.page.goto(L.uiUrl());

  const agent = await finishedWork(journey, "j05");
  const page = ctx.page;
  const card = page.locator(`article[data-id="${agent.card.id}"]`);
  await card.waitFor({ timeout: 30000 });
  journey.check("proof in git: the agent made one commit on its branch", agent.commits() === 1, `commits: ${agent.commits()}`);
  const changed = L.git(agent.dir, "diff", "--name-only", `${L.BASE}..HEAD`).split("\n").sort();
  journey.check("proof in git: it changed src/auth/login.py and docs/notes.md", changed.join() === "docs/notes.md,src/auth/login.py", changed.join());
  journey.check("the card carries a risk badge from the rules (high)", /high/i.test(await card.locator('[data-testid="risk-badge"]').innerText()));
  await L.shot(journey, page, `article[data-id="${agent.card.id}"]`, "finished-card");

  await openWork(page, agent.card);
  const facts = await page.locator("p.facts").innerText();
  journey.check("the work screen says 1 commit and 2 files", /1 commit/.test(facts) && /2 files?/.test(facts), facts);
  const risk = page.locator('[data-testid="risk"]');
  journey.check("the risk is high, from the rules, with its reasons", (await risk.getAttribute("data-level")) === "high" && /auth/.test(await risk.innerText()));
  const open = {};
  for (const layer of ["summary", "review", "plan", "points", "files"]) open[layer] = await layerOpen(page, layer);
  journey.check("high risk opens every layer", Object.values(open).every((value) => value === "true"), JSON.stringify(open));
  journey.check("the summary layer lists the commit subject", /feat: add login check/.test(await page.locator('[data-layer="summary"]').innerText()));
  const plan = await page.locator('[data-layer="plan"]').innerText();
  journey.check("the plan layer is there (from the transcript, or says why not)", plan.length > 20);
  journey.check("the diff lists exactly the files git lists", await L.until(async () => {
    const paths = await page.locator('[data-testid="diff"] article.file').evaluateAll((nodes) => nodes.map((n) => n.getAttribute("data-path")).sort());
    return paths.join() === changed.join();
  }, 20, 500));
  await L.shot(journey, page, '[data-testid="work"] .layers', "layers-high");

  // The pre-review: an opinion, run by Claude Code's /code-review in haiku.
  const state = page.locator('[data-testid="review"] .state');
  await state.waitFor({ timeout: 20000 });
  const run = page.locator('[data-testid="review"] button.run');
  if (await run.count()) {
    journey.check("the review layer offers to run the review", /review/i.test(await run.innerText()), await run.innerText());
    await run.click();
  }
  const done = await L.until(async () => ["done", "failed"].includes((await state.getAttribute("data-status")) ?? ""), 240, 2000);
  journey.check("the review finishes by itself (pushed, no reload)", done);
  journey.check("the review did not fail", (await state.getAttribute("data-status")) === "done", await state.innerText());
  const findings = await page.locator('[data-testid="review"] li.finding').count();
  console.log(`  note  the review found ${findings} finding(s)`);
  journey.check("the review says it is an opinion that never blocks", /never blocks/.test(await page.locator('[data-testid="review"]').innerText()));
  const stored = (await L.api("GET", `/api/agents/${encodeURIComponent(agent.card.id)}/review`)).json;
  journey.check("the daemon stored the review of this branch tip", JSON.stringify(stored).includes('"done"'), JSON.stringify(stored)?.slice(0, 120));
  await L.shot(journey, page, '[data-layer="review"]', "review-layer");
  if (findings > 0) {
    await L.sleep(1500);
    journey.check("the findings show in the guided diff, under their files", (await page.locator(".vf-item").count()) >= 1);
  }
  await page.locator("button.back").click();
  await page.waitForSelector(`article[data-id="${agent.card.id}"]`);
  journey.check("back on the inbox the card carries the review badge", findings === 0 || (await page.locator(`article[data-id="${agent.card.id}"] [data-testid="review-badge"]`).count()) === 1);
}
