// A question is asked: single choice, multiple choice and free text, answered in the browser.
import * as L from "./lib.mjs";

const PROMPT = `Call the AskUserQuestion tool exactly once, with these three questions in ONE call:
1. header "Color", question "Which color should the badge be?", single choice, options "Red" and "Blue".
2. header "Toppings", question "Which toppings do you want?", multiSelect true, options "Cheese", "Ham" and "Olives".
3. header "Name", question "What should the file be called?", single choice, options "alpha" and "beta".
Do not ask anything else and do not use any other tool. Once you have the answers, stop.`;

export default async function question(journey, ctx) {
  L.restoreSandbox();
  L.prepareBase();
  L.clearPersonalRules();
  await ctx.page.goto(L.uiUrl());

  const agent = await L.withAgent(journey, "j03", PROMPT);
  const decision = await L.decisionOf("j03");
  journey.check("a question card appears with its three questions", decision?.kind === "question" && decision.questions?.length === 3, JSON.stringify(decision)?.slice(0, 200));
  const card = ctx.page.locator(`article[data-id="${decision?.id}"]`);
  await card.waitFor({ timeout: 20000 });
  journey.check("the answer cannot be sent while a question is unanswered", await card.locator('button[data-action="answer"]').isDisabled());

  // single choice by a click, multiple choice by two clicks, free text typed.
  await card.locator('fieldset[data-question="0"] button[data-option="Blue"]').click();
  await card.locator('fieldset[data-question="1"] button[data-option="Cheese"]').click();
  await card.locator('fieldset[data-question="1"] button[data-option="Olives"]').click();
  await card.locator('fieldset[data-question="2"] input.other').fill("journey-free-text-7");
  journey.check("the answer can be sent once every question has an answer", await L.until(async () => !(await card.locator('button[data-action="answer"]').isDisabled()), 5, 200));
  await L.shot(journey, ctx.page, `article[data-id="${decision.id}"]`, "question-answered");
  await card.locator('button[data-action="answer"]').click();
  journey.check("the card disappears after Send answer", await L.until(async () => !(await L.inbox()).decisions.some((d) => d.id === decision.id), 20));

  // Proof by the transcript: the answers that reached the model, in Claude Code's own tool result.
  const reached = await L.until(() => L.toolResults(agent.transcript()).find((block) => JSON.stringify(block.content).includes("journey-free-text-7")), 60);
  const text = JSON.stringify(reached?.content ?? "");
  journey.check("the tool result holds the single answer (Blue)", text.includes("Blue"), text.slice(0, 300));
  journey.check("the tool result holds the multiple answer (Cheese and Olives, not Ham)", text.includes("Cheese") && text.includes("Olives") && !/"[^"]*Which toppings[^"]*"\s*=\s*"[^"]*Ham/.test(text), text.slice(0, 300));
  journey.check("the tool result holds the free text", text.includes("journey-free-text-7"));
  journey.check("the question was answered by the hook, so no terminal dialog was needed", !agent.transcript().some((entry) => /user rejected|doesn't want to proceed/i.test(JSON.stringify(entry.message?.content ?? ""))));
  journey.check("the hook events show the AskUserQuestion of this session", L.events().some((line) => /AskUserQuestion/.test(line) && line.includes(agent.sessionId.slice(0, 8))));
}
