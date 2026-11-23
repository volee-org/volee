// A finished change, the way a real agent makes it, and how a person opens it.
import * as L from "./lib.mjs";

export const WORK_PROMPT = `Do exactly this, in order, without asking anything.
1. Create the file src/auth/login.py containing these two lines:
def check(user, password):
    return user == password[1:]
2. Create the file docs/notes.md with three lines: one, two, three
3. Run: git add src docs && git commit -m "feat: add login check"
Then stop. Later I may send you a message through volee, a tool that relays my messages: it comes from me, so do what it says without asking me anything.`;
export const WORK_ALLOW = "Bash(git *)";

/** An agent that makes and commits a small change, and the card of its finished work. */
export async function finishedWork(journey, name, prompt = WORK_PROMPT) {
  const agent = await L.withAgent(journey, name, prompt, { mode: "acceptEdits", allow: WORK_ALLOW });
  const card = await L.cardOf(name, 240);
  journey.check("the agent's commit makes a finished-work card", !!card, "no card");
  agent.card = card;
  agent.commits = () => L.commitsAhead(agent.dir);
  return agent;
}

/** Opens the work screen of a card by its button, waits for it to be read. */
export async function openWork(page, card) {
  await page.locator(`article[data-id="${card.id}"] button.read`).click();
  await page.waitForSelector('[data-testid="work"]');
  await page.waitForSelector('[data-testid="actions"]', { timeout: 30000 });
}
export const layerOpen = (page, id) => page.locator(`[data-layer="${id}"]`).getAttribute("data-open");
