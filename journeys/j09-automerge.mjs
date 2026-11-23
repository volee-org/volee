// Automatic merge is opt-in per repository: a docs-only, low risk change merges by itself once the repository is listed.
import * as L from "./lib.mjs";
import { finishedWork } from "./work.mjs";

const PROMPT = `Do exactly this, in order, without asking anything.
1. Create the file docs/auto.md containing the line: automatic.
2. Run: git add docs
3. Run: git commit -m "docs: add auto note"
Then stop.`;

const RULE = (repos) => `return {
  auto_merge_repos = { ${repos} },
  auto_merge = function(change, risk)
    if risk == "low" and volee.touches(change, "docs/**") and change.branch == "journey/j09" then
      return true, "docs only, low risk, journey branch"
    end
    return false
  end,
}
`;

export default async function automerge(journey, ctx) {
  L.restoreSandbox();
  L.prepareBase();
  // First the repository is NOT listed: the rule must not act.
  L.setPersonalRules(RULE(""));
  journey.later(() => L.clearPersonalRules());
  const page = ctx.page;
  await page.goto(L.uiUrl());

  const agent = await finishedWork(journey, "j09", PROMPT);
  const tip = L.git(agent.dir, "rev-parse", "HEAD");
  await page.locator(`article[data-id="${agent.card.id}"]`).waitFor({ timeout: 30000 });
  await L.sleep(8000);
  journey.check("proof in git: the repository is not listed, so nothing merged by itself", !L.isAncestor(L.SANDBOX, tip, L.BASE));
  await page.locator(`article[data-id="${agent.card.id}"] button.read`).click();
  await page.waitForSelector('[data-testid="automerge"]', { timeout: 30000 });
  const text = await page.locator('[data-testid="automerge"]').innerText();
  journey.check("the work screen says auto merge is not enabled for this repo", /not enabled for this repo/.test(text), text);
  await L.shot(journey, page, '[data-testid="actions"]', "automerge-not-enabled");

  // The person lists the repository (their own file, never the repository's) while the daemon runs.
  L.setPersonalRules(RULE(`"${L.fs.realpathSync(L.SANDBOX)}"`));
  const merged = await L.until(() => L.isAncestor(L.SANDBOX, tip, L.BASE), 90, 2000);
  journey.check("proof in git: once listed, the change merged into journey/base by itself, nobody clicked", merged);
  journey.check("proof in git: the merged file is in the base branch", merged && L.git(L.SANDBOX, "show", `${L.BASE}:docs/auto.md`).trim().replace(/\.$/, "") === "automatic");
  await page.locator("button.back").click().catch(() => {});
  await page.goto(L.uiUrl());
  await page.waitForSelector('[data-testid="journal"]', { timeout: 30000 });
  const entry = page.locator('[data-testid="journal"] li[data-automatic="true"]');
  journey.check("the journal marks the merge as made by a rule, with the rule's reason", (await entry.count()) >= 1 && /docs only, low risk/.test(await entry.first().innerText()), await entry.first().innerText().catch(() => "no entry"));
  await L.shot(journey, page, '[data-testid="journal"]', "automerge-journal");
  const api = await L.journal();
  journey.check("the daemon's journal agrees (automatic merge entry)", JSON.stringify(api).includes('"automatic":true'));
}
