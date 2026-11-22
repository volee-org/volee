// The layers of a finished change (crates/volee-daemon/src/work/mod.rs).

export type StepStatus = "pending" | "in-progress" | "done" | "unknown";
export type PlanSource = "tasks" | "plan-mode" | "prompt";
export type FileStatus = "added" | "modified" | "deleted" | "renamed";
export type Category = "source" | "test" | "docs" | "config" | "lock" | "generated";
export type Outcome = "ok" | "failed" | "unknown";

export interface Summary {
  line: string;
  text: string | null;
  source: "transcript" | "git";
}

export interface Step {
  text: string;
  status: StepStatus;
}

export interface Plan {
  source: PlanSource;
  steps: Step[];
  text: string | null;
}

export interface OpenPoint {
  kind: "stated" | "detected";
  text: string;
}

export interface WorkFile {
  path: string;
  old_path: string | null;
  status: FileStatus;
  insertions: number;
  deletions: number;
  binary: boolean;
  category: Category;
  collapsed: boolean;
  patch: string;
  truncated: boolean;
  touched_by: string[];
  why: string | null;
}

export interface Command {
  command: string;
  description: string | null;
  is_test: boolean;
  outcome: Outcome;
  tail: string | null;
}

export interface Work {
  id: string;
  name: string | null;
  transcript: {
    available: boolean;
    reason: string | null;
    lines: number;
    skipped: number;
    prompts: { text: string; at: number | null }[];
  };
  summary: Summary;
  plan: Plan | null;
  open_points: OpenPoint[];
  commits: { sha: string; subject: string }[];
  files: WorkFile[];
  other_touched: string[];
  commands: Command[];
  tests: { ran: number; last: "passed" | "failed" | "unknown" };
  diff: {
    base: string | null;
    branch: string | null;
    error: string | null;
    truncated: boolean;
    files: number;
    insertions: number;
    deletions: number;
  };
  diagram: string;
}

/** The layers, in the order a reader opens them. */
export const LAYERS = ["summary", "plan", "points", "files", "diagram", "diff"] as const;
export type LayerId = (typeof LAYERS)[number];

/** Which layers start open: the ones that answer "what happened" at a glance. */
export function initialOpen(work: Work): Record<LayerId, boolean> {
  return {
    summary: true,
    plan: work.plan !== null,
    points: work.open_points.length > 0,
    files: false,
    diagram: false,
    diff: false,
  };
}

/** The layer after `from` that is still closed, for the "next layer" key. */
export function nextClosed(open: Record<LayerId, boolean>, from: LayerId | null): LayerId | null {
  const start = from === null ? 0 : LAYERS.indexOf(from) + 1;
  for (let i = 0; i < LAYERS.length; i++) {
    const layer = LAYERS[(start + i) % LAYERS.length]!;
    if (!open[layer]) return layer;
  }
  return null;
}

/** The layer `step` places away from `from`, staying inside the list. */
export function moveLayer(from: LayerId | null, step: 1 | -1): LayerId {
  const index = from === null ? (step === 1 ? 0 : LAYERS.length - 1) : LAYERS.indexOf(from) + step;
  return LAYERS[Math.min(Math.max(index, 0), LAYERS.length - 1)]!;
}

/**
 * Which files of the diff start open. A small change is read whole; a big
 * one opens its three most important files only. Locks and generated files
 * never start open.
 */
export function initialOpenFiles(files: WorkFile[]): Set<string> {
  const readable = files.filter((file) => !file.collapsed && !file.binary);
  const lines = readable.reduce((sum, file) => sum + file.insertions + file.deletions, 0);
  const shown = lines <= 400 ? readable : readable.slice(0, 3);
  return new Set(shown.map((file) => file.path));
}

export const STEP_MARK: Record<StepStatus, string> = {
  done: "✓",
  "in-progress": "◐",
  pending: "○",
  unknown: "•",
};

export function stepMark(status: StepStatus): string {
  return STEP_MARK[status];
}

/** Where the plan comes from, in words, so a guess is never taken for a record. */
export function planSourceText(source: PlanSource): string {
  switch (source) {
    case "tasks":
      return "The agent's own task list";
    case "plan-mode":
      return "The plan the agent proposed";
    case "prompt":
      return "Steps from your prompt (what was asked, not a record of what was done)";
  }
}

export function testsText(tests: Work["tests"]): string {
  if (tests.ran === 0) return "No test run recorded";
  const runs = tests.ran === 1 ? "1 test run" : `${tests.ran} test runs`;
  return `${runs}, last one ${tests.last === "passed" ? "passed" : tests.last === "failed" ? "failed" : "unclear"}`;
}

export const CATEGORY_TEXT: Record<Category, string> = {
  source: "source",
  test: "test",
  docs: "docs",
  config: "config",
  lock: "lock file",
  generated: "generated",
};

/** The unified diff of one file, ready for diff2html. */
export function patchOf(file: WorkFile): string {
  return file.patch.endsWith("\n") ? file.patch : `${file.patch}\n`;
}

/** Fetches an agent's work; `null` and a message when the daemon refuses. */
export async function fetchWork(
  token: string,
  id: string,
  fetcher: typeof fetch = fetch,
): Promise<{ work: Work | null; message: string }> {
  try {
    const response = await fetcher(`api/agents/${encodeURIComponent(id)}/work`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!response.ok) {
      const body = (await response.json().catch(() => ({}))) as { message?: string };
      return { work: null, message: body.message ?? `refused (${response.status})` };
    }
    return { work: (await response.json()) as Work, message: "" };
  } catch (error) {
    return { work: null, message: String(error) };
  }
}

/** The agent id in an address like `#work/abc`, or `null`. */
export function workIdFromHash(hash: string): string | null {
  const match = /^#work\/([^/?&]+)/.exec(hash);
  return match ? decodeURIComponent(match[1]!) : null;
}
