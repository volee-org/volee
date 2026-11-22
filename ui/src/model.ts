// The shapes the daemon sends (crates/volee-daemon/src/web/inbox.rs).

export type CardKind = "waiting" | "finished";

export interface DiffStat {
  files: number;
  insertions: number;
  deletions: number;
}

export interface Card {
  /** The Claude Code session id. */
  id: string;
  kind: CardKind;
  name: string | null;
  /** What Claude Code reports: "needs-input", "idle", "working"... */
  state: string;
  /** What a waiting agent waits for, in words. */
  waiting_for: string | null;
  /** Since when it waits (or has finished), in ms since the Unix epoch. */
  since: number | null;
  repo: string | null;
  branch: string | null;
  base: string | null;
  cwd: string;
  diff: DiffStat | null;
  ahead: number;
  dirty: boolean;
}

export interface Inbox {
  updated_at: number | null;
  /** Why the agents could not be listed, when they could not. */
  error: string | null;
  /** The terminal adapter that can open an agent's terminal, if one is there. */
  terminal: string | null;
  /** Agents that do not need a decision now. */
  others: number;
  cards: Card[];
}

export const emptyInbox: Inbox = {
  updated_at: null,
  error: null,
  terminal: null,
  others: 0,
  cards: [],
};

/** The card's title: its name, else the repository and branch. */
export function title(card: Card): string {
  if (card.name) return card.name;
  if (card.repo && card.branch) return `${card.repo} · ${card.branch}`;
  return card.repo ?? card.cwd.split("/").filter(Boolean).pop() ?? card.id.slice(0, 8);
}

export function split(cards: Card[]): { waiting: Card[]; finished: Card[] } {
  return {
    waiting: cards.filter((c) => c.kind === "waiting"),
    finished: cards.filter((c) => c.kind === "finished"),
  };
}
