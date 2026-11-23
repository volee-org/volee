// A question nobody answers is handed back to the terminal after its delay.
import * as L from "./lib.mjs";

const PROMPT = `Call the AskUserQuestion tool exactly once with one question: header "Color", question "Which color do you prefer?", single choice, options "Red" and "Blue". Do not use any other tool. Wait for the answer.`;

export default async function timeout(journey, ctx) {
  L.restoreSandbox();
  L.prepareBase();
  // The delay is a rule, and `volee doctor` writes it as the hook's timeout: set it before the repository is set up.
  L.setPersonalRules(`return { held_question_timeout = "20s" }\n`);
  journey.later(() => L.clearPersonalRules());
  await ctx.page.goto(L.uiUrl());

  const agent = await L.withAgent(journey, "j04", PROMPT);
  const decision = await L.decisionOf("j04");
  journey.check("a question card appears", decision?.kind === "question");
  const card = ctx.page.locator(`article[data-id="${decision?.id}"]`);
  await card.waitFor({ timeout: 20000 });
  const timer = await card.locator('[data-testid="remaining"]').innerText();
  journey.check("the card shows how long before the terminal asks (a few seconds, not minutes)", /^0:\d\d$/.test(timer.trim()) || /^\d+s$/.test(timer.trim()), timer);
  await L.shot(journey, ctx.page, `article[data-id="${decision.id}"]`, "question-waiting");

  // Nobody answers. After the delay the card goes and Claude Code asks in its own terminal.
  const gone = await L.until(async () => !(await L.inbox()).decisions.some((d) => d.id === decision.id), 60, 1000);
  journey.check("the card is gone once the delay is over", gone);
  const waiting = await L.until(() => {
    const entry = L.agentById(agent.short);
    return entry && /question|input|permission|waiting/i.test(`${entry.waitingFor ?? ""} ${entry.state ?? ""} ${entry.status ?? ""}`) ? entry : false;
  }, 30);
  journey.check("Claude Code itself says the session waits for an answer (the terminal dialog)", !!waiting, JSON.stringify(L.agentById(agent.short)));
  journey.check("proof in the transcript: the question was not answered", !L.toolResults(agent.transcript()).some((block) => /answered your questions|"Red"|"Blue"/.test(JSON.stringify(block.content ?? ""))));
  await ctx.page.reload();
  await ctx.page.waitForSelector("main");
  journey.check("the inbox no longer asks for it", (await ctx.page.locator(`article[data-id="${decision.id}"]`).count()) === 0);
}
