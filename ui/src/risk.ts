// How much care a change needs, as the rules say it and the person may
// change it (crates/volee-daemon/src/assess.rs).

export type RiskLevel = "low" | "medium" | "high";

/** The layers a rules file names; the screen has a few more (see `layerMap`). */
export type RuleLayer = "summary" | "plan" | "review" | "diagram" | "diff";

export interface RiskView {
  /** The branch tip this is about. */
  tip: string;
  /** The level in force: the person's, else the rules'. */
  level: RiskLevel;
  reasons: string[];
  /** What the rules computed. */
  computed: RiskLevel;
  /** The person set `level` by hand for this tip. */
  overridden: boolean;
  /** The layers the screen opens with. */
  layers: RuleLayer[];
  auto_merge: { allowed: boolean; reason: string };
  /** What the repository's rules asked for and the person's rules refused. */
  notes: string[];
  /** Rules that failed: the answer is then the safest one. */
  errors: string[];
}

/** A failing rules file, as the daemon tells it (file and line when known). */
export interface RuleErrorView {
  file: string | null;
  line: number | null;
  function: string | null;
  message: string;
  text: string;
}

export interface RulesStatus {
  errors: RuleErrorView[];
  warnings: string[];
}

export const emptyRules: RulesStatus = { errors: [], warnings: [] };

export const LEVELS: RiskLevel[] = ["low", "medium", "high"];

export const LEVEL_TEXT: Record<RiskLevel, string> = {
  low: "Low risk",
  medium: "Medium risk",
  high: "High risk",
};

/** The one line a card shows: the level, and that it was set by hand. */
export function riskText(risk: RiskView): string {
  return risk.overridden ? `${LEVEL_TEXT[risk.level]} (set by you)` : LEVEL_TEXT[risk.level];
}

/** The tooltip of a risk badge: the reasons, one per line. */
export function riskTitle(risk: RiskView): string {
  const lines = [...risk.reasons];
  if (risk.overridden) lines.push(`The rules said ${risk.computed}.`);
  lines.push(risk.auto_merge.allowed ? "May merge by itself." : `Not merged by itself: ${risk.auto_merge.reason}`);
  return lines.join("\n");
}

/** What the banner says: the first failing file with its line, and how many more. */
export function rulesBannerText(status: RulesStatus): string {
  const [first, ...rest] = status.errors;
  if (!first) return "";
  const more = rest.length === 0 ? "" : ` (and ${rest.length} more)`;
  return `The rules do not load, so every change is treated as high risk: ${first.text}${more}`;
}

/** Sets the risk of an agent's branch tip by hand; `null` takes the choice back. */
export async function setRisk(
  token: string,
  id: string,
  level: RiskLevel | null,
  fetcher: typeof fetch = fetch,
): Promise<{ risk: RiskView | null; message: string }> {
  try {
    const response = await fetcher(`api/agents/${encodeURIComponent(id)}/risk`, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify({ level }),
    });
    if (!response.ok) {
      const body = (await response.json().catch(() => ({}))) as { message?: string };
      return { risk: null, message: body.message ?? `refused (${response.status})` };
    }
    return { risk: (await response.json()) as RiskView, message: "" };
  } catch (error) {
    return { risk: null, message: String(error) };
  }
}
