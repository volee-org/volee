// A permission is asked: the person allows one in the browser and denies another.
import * as L from "./lib.mjs";

const prompt = (file) =>
  `Use the Write tool to create the file ${file} containing the single word ok. If the tool is refused, do not try another way: stop.`;

export default async function permission(journey, ctx) {
  L.restoreSandbox();
  L.prepareBase();
  L.clearPersonalRules();
  await ctx.page.goto(L.uiUrl());

  // --- allowed from the browser ---
  const allowed = await L.withAgent(journey, "j02a", prompt("docs/allowed.txt"));
  const decision = await L.decisionOf("j02a");
  journey.check("a permission card appears for the Write the agent attempts", !!decision && decision.kind === "permission" && decision.tool_name === "Write", JSON.stringify(decision)?.slice(0, 200));
  const card = ctx.page.locator(`article[data-id="${decision?.id}"]`);
  await card.waitFor({ timeout: 20000 });
  journey.check("the card names the file", (await card.innerText()).includes("allowed.txt"));
  await L.shot(journey, ctx.page, `article[data-id="${decision.id}"]`, "permission-card");
  const before = L.fileExists(L.path.join(allowed.dir, "docs/allowed.txt"));
  await card.locator('button[data-action="allow"]').click();
  journey.check("the card disappears after Allow", await L.until(async () => !(await L.inbox()).decisions.some((d) => d.id === decision.id), 20));
  const done = await L.until(() => L.fileExists(L.path.join(allowed.dir, "docs/allowed.txt")), 60);
  journey.check("proof on disk: the file did not exist before and exists after Allow", !before && done);
  const wrote = L.toolUses(allowed.transcript(), "Write");
  const result = L.toolResults(allowed.transcript()).find((block) => block.tool_use_id === wrote[0]?.id);
  journey.check("proof in the transcript: the Write ran and its result is not an error", wrote.length === 1 && result && !result.is_error);
  journey.check("the hook events show PermissionRequest for this session", L.events().some((line) => /PermissionRequest/.test(line) && line.includes(allowed.sessionId.slice(0, 8))));

  // --- denied from the browser, with a reason ---
  const denied = await L.withAgent(journey, "j02b", prompt("docs/denied.txt"));
  const second = await L.decisionOf("j02b");
  journey.check("a second permission card appears", !!second && second.kind === "permission");
  const card2 = ctx.page.locator(`article[data-id="${second?.id}"]`);
  await card2.waitFor({ timeout: 20000 });
  await card2.locator('button[data-action="deny"]').click();
  await card2.locator('input[aria-label="Reason for denying"]').fill("journey: not this one");
  await L.shot(journey, ctx.page, `article[data-id="${second.id}"]`, "deny-with-reason");
  await card2.locator('button[data-action="deny-send"]').click();
  await L.sleep(15000);
  journey.check("proof on disk: the denied file was never written", !L.fileExists(L.path.join(denied.dir, "docs/denied.txt")));
  const attempt = L.toolUses(denied.transcript(), "Write")[0];
  const refusal = L.toolResults(denied.transcript()).find((block) => block.tool_use_id === attempt?.id);
  const text = JSON.stringify(refusal?.content ?? "");
  journey.check("proof in the transcript: the Write came back refused with the person's reason", !!refusal && refusal.is_error === true && text.includes("not this one"), text.slice(0, 200));
  journey.check("the card of the denied request is gone", await L.until(async () => !(await L.inbox()).decisions.some((d) => d.id === second.id), 20));
}
