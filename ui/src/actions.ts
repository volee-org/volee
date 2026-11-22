// What the person does on a finished change: merge, ask for a fix, comment a
// line, open a file in their editor (crates/volee-daemon/src/actions/).

import type { Finding } from "./review";

export interface LineComment {
  id: number;
  file: string;
  /** The line on the new side of the diff. */
  line: number;
  text: string;
  created_at: number;
}

export interface EditorOption {
  name: string;
  kind: "terminal" | "external";
  /** A terminal editor needs a terminal program the daemon can open a pane in. */
  usable: boolean;
}

export interface JournalEntry {
  id: number;
  at: number;
  repo: string;
  branch: string;
  tip: string;
  kind: "merge" | "fix" | "comment" | "editor";
  ok: boolean;
  /** volee did it by itself, by a rule. */
  automatic: boolean;
  message: string;
}

export interface ActionResult {
  ok: boolean;
  message: string;
  /** For a fix request: whether the agent was shown to have it. */
  delivered: boolean | null;
}

async function call(
  token: string,
  path: string,
  method: string,
  body: unknown,
  fetcher: typeof fetch,
): Promise<{ status: number; json: Record<string, unknown> }> {
  const response = await fetcher(path, {
    method,
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const json = (await response.json().catch(() => ({}))) as Record<string, unknown>;
  return { status: response.status, json };
}

async function act(
  token: string,
  path: string,
  body: unknown,
  fetcher: typeof fetch,
): Promise<ActionResult> {
  try {
    const { status, json } = await call(token, path, "POST", body, fetcher);
    const message = typeof json.message === "string" ? json.message : status < 300 ? "" : `refused (${status})`;
    return {
      ok: status < 300 && json.ok !== false,
      message,
      delivered: typeof json.delivered === "boolean" ? json.delivered : null,
    };
  } catch (error) {
    return { ok: false, message: String(error), delivered: null };
  }
}

const agentPath = (id: string, tail: string) => `api/agents/${encodeURIComponent(id)}/${tail}`;

/** Merges the finished work by the repository's method; the daemon does it. */
export function mergeWork(token: string, id: string, fetcher: typeof fetch = fetch): Promise<ActionResult> {
  return act(token, agentPath(id, "merge"), undefined, fetcher);
}

/** Sends a fix request to the agent. `delivered: false` means offer the fallback. */
export function askFix(
  token: string,
  id: string,
  text: string,
  fetcher: typeof fetch = fetch,
): Promise<ActionResult> {
  return act(token, agentPath(id, "fix"), { text }, fetcher);
}

/** Opens a file of the agent's worktree in one of the person's editors. */
export function openEditor(
  token: string,
  id: string,
  target: { editor: string; file: string; line: number },
  fetcher: typeof fetch = fetch,
): Promise<ActionResult> {
  return act(token, agentPath(id, "editor"), target, fetcher);
}

export async function addComment(
  token: string,
  id: string,
  comment: { file: string; line: number; text: string },
  fetcher: typeof fetch = fetch,
): Promise<{ comment: LineComment | null; message: string }> {
  try {
    const { status, json } = await call(token, agentPath(id, "comments"), "POST", comment, fetcher);
    if (status >= 300) return { comment: null, message: String(json.message ?? `refused (${status})`) };
    return { comment: json as unknown as LineComment, message: "" };
  } catch (error) {
    return { comment: null, message: String(error) };
  }
}

export async function deleteComment(
  token: string,
  id: string,
  commentId: number,
  fetcher: typeof fetch = fetch,
): Promise<boolean> {
  try {
    const { status } = await call(token, agentPath(id, `comments/${commentId}`), "DELETE", undefined, fetcher);
    return status < 300;
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// The fix request

/** One point the person can send to the agent: a review finding or a comment of theirs. */
export interface FixItem {
  key: string;
  kind: "finding" | "comment";
  file: string;
  line: number | null;
  text: string;
  checked: boolean;
}

/**
 * The points a fix request can carry: the review's findings (the important
 * ones start selected, the rest do not) and every line comment (selected).
 */
export function fixItems(findings: Finding[], comments: LineComment[]): FixItem[] {
  const fromReview = findings.map((finding, i): FixItem => {
    const scenario = finding.failure_scenario ? ` Failure scenario: ${finding.failure_scenario}` : "";
    return {
      key: `finding:${i}`,
      kind: "finding",
      file: finding.file,
      line: finding.line ?? null,
      text: `(review, ${finding.severity}) ${finding.summary}${scenario}`,
      checked: finding.severity === "important",
    };
  });
  const own = comments.map(
    (comment): FixItem => ({
      key: `comment:${comment.id}`,
      kind: "comment",
      file: comment.file,
      line: comment.line,
      text: comment.text,
      checked: true,
    }),
  );
  return [...own, ...fromReview];
}

/** The request text for the selected items, numbered, one line each. */
export function composeFix(items: FixItem[]): string {
  const chosen = items.filter((item) => item.checked);
  if (chosen.length === 0) return "";
  const lines = chosen.map((item, i) => {
    const where = item.line === null ? item.file : `${item.file}:${item.line}`;
    return `${i + 1}. ${where}: ${item.text.replace(/\s+/g, " ").trim()}`;
  });
  return ["Please fix these points in your change, then commit:", "", ...lines].join("\n");
}

// ---------------------------------------------------------------------------
// The journal

export const KIND_TEXT: Record<JournalEntry["kind"], string> = {
  merge: "Merge",
  fix: "Fix request",
  comment: "Comment",
  editor: "Editor",
};

/** One line for an entry: what, who did it (a rule, or you), and what came of it. */
export function journalLine(entry: JournalEntry): string {
  const who = entry.automatic ? "by a rule" : "by you";
  return `${KIND_TEXT[entry.kind] ?? entry.kind} ${who}: ${entry.message}`;
}

/** The entries of one branch, newest first (the daemon already sorts them). */
export function journalOf(entries: JournalEntry[], branch: string | null): JournalEntry[] {
  return branch === null ? entries : entries.filter((entry) => entry.branch === branch);
}

/** Whether the automatic merge line should say "off": the rules, or the risk, refuse it. */
export function autoMergeText(auto: { allowed: boolean; reason: string } | undefined): string {
  if (!auto) return "";
  return auto.allowed
    ? `Merges by itself when the agent finishes: ${auto.reason}`
    : `Not merged by itself: ${auto.reason}`;
}
