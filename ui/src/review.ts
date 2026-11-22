// The AI pre-review of a branch tip (crates/volee-daemon/src/review.rs).
// An opinion: nothing here gates anything.

export type ReviewStatus = "none" | "pending" | "running" | "done" | "failed" | "disabled";
export type Severity = "important" | "nit" | "pre_existing";

export interface Finding {
  file: string;
  line: number | null;
  severity: Severity;
  summary: string;
  failure_scenario: string | null;
}

export interface Review {
  status: ReviewStatus;
  tip?: string;
  base?: string;
  model?: string;
  reviewers?: number;
  requested_at?: number;
  started_at?: number | null;
  finished_at?: number | null;
  error?: string | null;
  cost_usd?: number | null;
  raw?: string | null;
  findings: Finding[];
  message?: string;
}

/** What a finished card carries about its review: enough for a badge. */
export interface ReviewBrief {
  status: string;
  findings: number;
  tip: string;
}

const SEVERITY_RANK: Record<Severity, number> = { important: 0, nit: 1, pre_existing: 2 };

export const SEVERITY_TEXT: Record<Severity, string> = {
  important: "important",
  nit: "nit",
  pre_existing: "already there",
};

/** Important first, then nits, then what was there before; then by file and line. */
export function sortFindings(findings: Finding[]): Finding[] {
  return [...findings].sort(
    (a, b) =>
      (SEVERITY_RANK[a.severity] ?? 9) - (SEVERITY_RANK[b.severity] ?? 9) ||
      a.file.localeCompare(b.file) ||
      (a.line ?? 0) - (b.line ?? 0),
  );
}

export interface Badge {
  text: string;
  tone: "running" | "findings" | "clean" | "failed";
}

/** The small badge of a finished card, or `null` when there is no review. */
export function badge(brief: ReviewBrief | null | undefined): Badge | null {
  if (!brief) return null;
  switch (brief.status) {
    case "pending":
    case "running":
      return { text: "Review running", tone: "running" };
    case "failed":
      return { text: "Review failed", tone: "failed" };
    case "done":
      return brief.findings === 0
        ? { text: "Review: no findings", tone: "clean" }
        : {
            text: `Review: ${brief.findings} ${brief.findings === 1 ? "finding" : "findings"}`,
            tone: "findings",
          };
    default:
      return null;
  }
}

/** A key that changes when the review changed: the screen reads it again then. */
export function briefKey(brief: ReviewBrief | null | undefined): string {
  return brief ? `${brief.status}:${brief.tip}:${brief.findings}` : "none";
}

/** The label of the button that asks for a review, or `null` when none is allowed. */
export function runLabel(review: Review | null): string | null {
  if (!review) return null;
  if (review.status === "none") return "Run review";
  if (review.status === "failed") return "Retry";
  return null;
}

export function busy(review: Review | null): boolean {
  return review?.status === "pending" || review?.status === "running";
}

/** The line shown while the layer is closed. */
export function reviewHint(review: Review | null): string {
  if (!review) return "reading…";
  switch (review.status) {
    case "none":
      return "not run yet";
    case "disabled":
      return "switched off";
    case "pending":
    case "running":
      return "running…";
    case "failed":
      return "failed";
    case "done":
      return review.findings.length === 0
        ? "no findings"
        : `${review.findings.length} ${review.findings.length === 1 ? "finding" : "findings"}`;
  }
}

/** Who reviewed, how long it took and what it cost, in words. */
export function reviewFacts(review: Review): string {
  const parts: string[] = [];
  const reviewers = review.reviewers ?? 1;
  parts.push(`${reviewers} ${reviewers === 1 ? "reviewer" : "reviewers"}`);
  if (review.model) parts.push(review.model);
  if (review.started_at && review.finished_at) {
    parts.push(`${Math.max(1, Math.round((review.finished_at - review.started_at) / 1000))} s`);
  }
  if (typeof review.cost_usd === "number") parts.push(`$${review.cost_usd.toFixed(3)}`);
  return parts.join(" · ");
}

/** The review of an agent's branch tip; `null` and a message when the daemon refuses. */
export async function fetchReview(
  token: string,
  id: string,
  fetcher: typeof fetch = fetch,
): Promise<{ review: Review | null; message: string }> {
  try {
    const response = await fetcher(`api/agents/${encodeURIComponent(id)}/review`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!response.ok) {
      const body = (await response.json().catch(() => ({}))) as { message?: string };
      return { review: null, message: body.message ?? `refused (${response.status})` };
    }
    return { review: (await response.json()) as Review, message: "" };
  } catch (error) {
    return { review: null, message: String(error) };
  }
}

/** Asks for a review now. The answer says what happened (queued, exists, off). */
export async function requestReview(
  token: string,
  id: string,
  retry: boolean,
  fetcher: typeof fetch = fetch,
): Promise<{ ok: boolean; message: string }> {
  try {
    const response = await fetcher(
      `api/agents/${encodeURIComponent(id)}/review${retry ? "?retry=1" : ""}`,
      { method: "POST", headers: { Authorization: `Bearer ${token}` } },
    );
    const body = (await response.json().catch(() => ({}))) as { message?: string };
    return { ok: response.ok, message: body.message ?? (response.ok ? "" : `refused (${response.status})`) };
  } catch (error) {
    return { ok: false, message: String(error) };
  }
}

// ---------------------------------------------------------------------------
// Findings in the diff

function clean(path: string): string {
  return path.replace(/^\.\//, "").replace(/^\/+/, "");
}

/**
 * Whether a finding's file is this diff file. Tolerant: the reviewer may say
 * an absolute path, or leave a folder out, so one path ending the other is a
 * match.
 */
export function sameFile(findingFile: string, path: string): boolean {
  const a = clean(findingFile);
  const b = clean(path);
  return a === b || a.endsWith(`/${b}`) || b.endsWith(`/${a}`);
}

export function findingsOf(findings: Finding[], path: string): Finding[] {
  return sortFindings(findings.filter((finding) => sameFile(finding.file, path)));
}

/** The new-side line numbers a unified diff shows (its added and unchanged lines). */
export function visibleLines(patch: string): Set<number> {
  const lines = new Set<number>();
  let next: number | null = null;
  for (const line of patch.split("\n")) {
    const hunk = /^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) {
      next = Number(hunk[1]);
      continue;
    }
    if (next === null) continue;
    if (line.startsWith("+") || line.startsWith(" ")) {
      lines.add(next);
      next += 1;
    }
  }
  return lines;
}

export interface Placed {
  /** Findings whose line is in the diff, by new-side line number. */
  inline: Map<number, Finding[]>;
  /** The others: no line, or a line the diff does not show. They go on the file header. */
  header: Finding[];
}

/** Where each finding of a file goes: at its line, or on the file header. */
export function placeFindings(patch: string, findings: Finding[]): Placed {
  const shown = visibleLines(patch);
  const placed: Placed = { inline: new Map(), header: [] };
  for (const finding of sortFindings(findings)) {
    if (finding.line !== null && shown.has(finding.line)) {
      const at = placed.inline.get(finding.line) ?? [];
      at.push(finding);
      placed.inline.set(finding.line, at);
    } else {
      placed.header.push(finding);
    }
  }
  return placed;
}

/** `file:line`, or just the file when the reviewer gave no line. */
export function whereText(finding: Finding): string {
  return finding.line === null ? finding.file : `${finding.file}:${finding.line}`;
}
