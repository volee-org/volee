// A fix is asked of an agent whose session was stopped: volee resumes that same session.
import * as L from "./lib.mjs";
import { finishedWork } from "./work.mjs";

export default async function resume(journey, ctx) {
  L.restoreSandbox();
  L.prepareBase();
  L.setPersonalRules(`return { sensitive_paths = { "src/billing/**" } }\n`);
  journey.later(() => L.clearPersonalRules());
  const page = ctx.page;
  await page.goto(L.uiUrl());

  const agent = await finishedWork(journey, "j08");
  const before = L.agentById(agent.short);
  journey.check("the session is alive before it is stopped", !!before?.pid);
  L.claudeStop(agent.short);
  const stopped = await L.until(() => !L.claudeAgents().some((a) => a.sessionId === agent.sessionId && a.pid === before.pid), 30, 1000);
  journey.check("`claude stop` stopped it: its process is gone", stopped);

  // volee learns it at its next look at `claude agents`: asked sooner, it would still take the session for a live one.
  const knows = await L.until(async () => (await L.api("GET", `/api/agents/${encodeURIComponent(agent.card.id)}/work`)).json?.agent?.alive === false, 90, 2000);
  journey.check("the daemon knows the session is no longer alive", knows);

  // A stopped session is no longer an inbox card, but its work is still there to read by its id.
  await page.evaluate((id) => {
    location.hash = `work/${id}`;
  }, agent.card.id);
  await page.reload();
  await page.waitForSelector('[data-testid="actions"]', { timeout: 30000 });
  journey.check("the work screen of the stopped session still opens", /feat: add login check/.test(await page.locator('[data-layer="summary"]').innerText()));
  await page.locator('[data-testid="ask-fix"]').click();
  const area = page.locator('textarea[aria-label="The request, as the agent will read it"]');
  await area.fill('Do it now and do not ask me anything: append a line containing resumed to docs/notes.md, then run git add docs and git commit -m "fix: add resumed line".');
  await page.locator("button.send").click();
  const result = page.locator('[data-testid="fix-result"]');
  await result.waitFor({ timeout: 90000 });
  const said = await result.innerText();
  journey.check("volee says the stopped session was resumed with the message", /resume/i.test(said) && !/not (reach|deliver)/i.test(said), said);
  await L.shot(journey, page, '[data-testid="actions"]', "fix-resume-delivered");

  const committed = await L.until(() => agent.commits() === 2, 150, 2000);
  journey.check("proof in git: the resumed agent made a second commit", committed, `commits: ${agent.commits()}`);
  const notes = L.git(agent.dir, "show", "HEAD:docs/notes.md").split("\n").filter(Boolean);
  journey.check("proof in git: docs/notes.md ends with the requested line", notes[notes.length - 1] === "resumed", notes.join("|"));
  const same = L.claudeAgents().filter((a) => a.sessionId === agent.sessionId);
  journey.check("proof by Claude Code: the same session id came back once, with a new process, no copy was started", same.length === 1 && same[0].pid !== before.pid, JSON.stringify(same.map((a) => [a.sessionId.slice(0, 8), a.pid])));
  journey.check("proof in the transcript: the message is in the original session's own transcript", JSON.stringify(agent.transcript()).includes("volee message"));
}
