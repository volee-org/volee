// Puts review findings in a diff2html table, under the line they are about.

import { type Finding, SEVERITY_TEXT, whereText } from "./review";

const CLASS = "volee-finding";

function element(tag: string, className: string, text: string): HTMLElement {
  const node = document.createElement(tag);
  node.className = className;
  node.textContent = text;
  return node;
}

function box(findings: Finding[]): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "vf-box";
  for (const finding of findings) {
    const item = document.createElement("div");
    item.className = `vf-item ${finding.severity}`;
    const head = document.createElement("div");
    head.className = "vf-head";
    head.append(
      element("span", "vf-label", "AI opinion"),
      element("span", "vf-sev", SEVERITY_TEXT[finding.severity] ?? finding.severity),
      element("span", "vf-where", whereText(finding)),
    );
    item.append(head, element("div", "vf-summary", finding.summary));
    if (finding.failure_scenario) {
      item.append(element("div", "vf-scenario", `Failure scenario: ${finding.failure_scenario}`));
    }
    wrap.append(item);
  }
  return wrap;
}

/**
 * Inserts one row after each row whose new-side line number has findings.
 * Text only (`textContent`): a finding is model output and is never HTML.
 * Calling it again first removes what it inserted before.
 */
export function annotate(root: HTMLElement, byLine: Map<number, Finding[]>): void {
  root.querySelectorAll(`tr.${CLASS}`).forEach((row) => row.remove());
  if (byLine.size === 0) return;
  for (const row of Array.from(root.querySelectorAll("tr"))) {
    const cell = row.querySelector(".line-num2");
    const number = cell ? Number(cell.textContent?.trim()) : NaN;
    const findings = Number.isFinite(number) ? byLine.get(number) : undefined;
    if (!findings) continue;
    const extra = document.createElement("tr");
    extra.className = CLASS;
    const td = document.createElement("td");
    td.colSpan = 2;
    td.append(box(findings));
    extra.append(td);
    row.after(extra);
  }
}
