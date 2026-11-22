// The shapes the daemon sends (crates/volee-daemon/src/web/inbox.rs).

import type { ReviewBrief } from "./review";

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
  /** How the base is known: "upstream", "reflog", "nearest" or "default" (a guess). */
  base_source: string | null;
  cwd: string;
  diff: DiffStat | null;
  ahead: number;
  dirty: boolean;
  /** The AI pre-review of the branch's current tip, in short; absent when nobody asked. */
  review?: ReviewBrief | null;
}

export interface Suggestion {
  index: number;
  /** "Always allow Bash(npm test) in this project (local)". */
  label: string;
}

export interface QuestionOption {
  label: string;
  description?: string;
}

export interface Question {
  question: string;
  header?: string;
  multiSelect?: boolean;
  options: QuestionOption[];
}

export type DecisionKind = "permission" | "question";

/** A request Claude Code is holding open until the person decides. */
export interface Decision {
  id: string;
  kind: DecisionKind;
  session_id: string;
  tool_name: string;
  /** One line that says what is asked (the command, the file). */
  summary: string;
  tool_input: unknown;
  suggestions: Suggestion[];
  /** The questions as the agent wrote them (null for a permission). */
  questions: Question[] | null;
  created_at: number;
  /** When the hold ends and the terminal takes over. */
  expires_at: number;
  name: string | null;
  repo: string | null;
  branch: string | null;
  base: string | null;
  base_source: string | null;
  cwd: string | null;
}

export interface Inbox {
  updated_at: number | null;
  /** Why the agents could not be listed, when they could not. */
  error: string | null;
  /** The terminal adapter that can open an agent's terminal, if one is there. */
  terminal: string | null;
  /** Agents that do not need a decision now. */
  others: number;
  /** Held permission requests and questions, oldest first. */
  decisions: Decision[];
  cards: Card[];
}

export const emptyInbox: Inbox = {
  updated_at: null,
  error: null,
  terminal: null,
  others: 0,
  decisions: [],
  cards: [],
};

/** What a title is made from. */
export interface Titled {
  name: string | null;
  repo: string | null;
  branch: string | null;
  cwd: string | null;
  id: string;
}

/** The card's title: its name, else the repository and branch. */
export function title(card: Titled): string {
  if (card.name) return card.name;
  if (card.repo && card.branch) return `${card.repo} · ${card.branch}`;
  return card.repo ?? card.cwd?.split("/").filter(Boolean).pop() ?? card.id.slice(0, 8);
}

export function split(cards: Card[]): { waiting: Card[]; finished: Card[] } {
  return {
    waiting: cards.filter((c) => c.kind === "waiting"),
    finished: cards.filter((c) => c.kind === "finished"),
  };
}
