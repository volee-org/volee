// The risk comes from rules.lua, follows it live, and the person can change it on the card.
import * as L from "./lib.mjs";
import { finishedWork, openWork, layerOpen } from "./work.mjs";

const HIGH = `return { sensitive_paths = { "src/auth/**" }, layers = { low = { "summary" } } }\n`;
const LOW = `return { sensitive_paths = { "src/billing/**" }, layers = { low = { "summary" } } }\n`;

export default async function risk(journey, ctx) {
  L.restoreSandbox();
  L.prepareBase();
  L.setPersonalRules(HIGH);
  journey.later(() => L.clearPersonalRules());
  const page = ctx.page;
  await page.goto(L.uiUrl());

  const agent = await finishedWork(journey, "j06");
  const level = () => page.locator('[data-testid="risk"]').getAttribute("data-level");
  const badge = page.locator(`article[data-id="${agent.card.id}"] [data-testid="risk-badge"]`);
  await badge.waitFor({ timeout: 30000 });
  journey.check("the rules file marks the change high risk (src/auth/**)", /high/i.test(await badge.innerText()));
  await openWork(page, agent.card);
  journey.check("the work screen agrees: high, every layer open", (await level()) === "high" && (await layerOpen(page, "files")) === "true");
  await L.shot(journey, page, '[data-testid="risk"]', "risk-high-from-rules");

  // The file is edited while the daemon runs: no restart.
  L.setPersonalRules(LOW);
  const workLevel = async () => (await L.api("GET", `/api/agents/${encodeURIComponent(agent.card.id)}/work`)).json?.risk?.level;
  journey.check("editing rules.lua makes the daemon answer low for the same change, without restarting anything", await L.until(async () => (await workLevel()) === "low", 30, 1000));
  // The open screen reads the work again when it is opened or reloaded.
  await page.reload();
  await page.waitForSelector('[data-testid="actions"]', { timeout: 30000 });
  journey.check("the screen shows low risk", await L.until(async () => (await level()) === "low", 15, 500));
  journey.check("low risk opens the summary only, as the rules' layers say", (await layerOpen(page, "summary")) === "true" && (await layerOpen(page, "files")) !== "true");
  await L.shot(journey, page, '[data-testid="risk"]', "risk-low-after-edit");

  // The person raises it by hand, then gives it back to the rules.
  await page.locator('[data-testid="risk"] button[data-level="high"]').click();
  const raised = await L.until(async () => (await page.locator('[data-testid="risk"]').getAttribute("data-overridden")) === "true", 15, 500);
  journey.check("a click on `high` sets the risk by hand, and the card says so", raised && (await level()) === "high");
  const stored = (await L.api("GET", `/api/agents/${encodeURIComponent(agent.card.id)}/work`)).json;
  journey.check("the daemon holds the override for this tip", stored?.risk?.overridden === true && stored?.risk?.level === "high", JSON.stringify(stored?.risk)?.slice(0, 160));
  await L.shot(journey, page, '[data-testid="risk"]', "risk-set-by-hand");
  await page.locator('[data-testid="risk-reset"]').click();
  journey.check("`Back to the rules` returns to the computed level (low)", await L.until(async () => (await level()) === "low", 15, 500));

  // A broken rules file never goes unnoticed and never lowers anything.
  await page.locator("button.back").click();
  L.setPersonalRules(`return { sensitive_paths = ,\n`);
  const banner = page.locator('[data-testid="rules-banner"]');
  journey.check("a broken rules file shows a banner with its file and line", await L.until(async () => (await banner.count()) > 0 && /rules\.lua/.test(await banner.innerText()), 20, 500));
  const doctor = L.volee(["doctor", "--project", agent.dir]);
  journey.check("`volee doctor` prints the same rules error", /rules error/.test(doctor.out), doctor.out.slice(0, 200));
  const degraded = await L.until(async () => /high/i.test(await badge.innerText()), 15, 500);
  journey.check("until it is fixed, the card falls back to high risk", degraded);
  await L.shot(journey, page, "main", "rules-broken-banner");
  L.setPersonalRules(LOW);
  journey.check("saving a good file clears the banner", await L.until(async () => (await banner.count()) === 0, 20, 500));
}
