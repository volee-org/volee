// Puts review findings in a diff2html table, under the line they are about.

import type { EditorOption, LineComment } from "./actions";
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

const COMMENT = "volee-comment";

export interface CommentOps {
  /** The comments of this file. */
  comments: LineComment[];
  /** The line whose form is open, if any. */
  composing: number | null;
  /** The editors that can open a file now. */
  editors: EditorOption[];
  onpick: (line: number) => void;
  oncancel: () => void;
  onsave: (line: number, text: string) => void;
  ondelete: (id: number) => void;
  onedit: (editor: string, line: number) => void;
}

function button(label: string, className: string, onclick: () => void): HTMLButtonElement {
  const node = document.createElement("button");
  node.type = "button";
  node.className = className;
  node.textContent = label;
  node.addEventListener("click", onclick);
  return node;
}

/** The new-side line number of a diff row, when it has one. */
function lineOf(row: Element): number | null {
  const cell = row.querySelector(".line-num2");
  const number = cell ? Number(cell.textContent?.trim()) : NaN;
  return Number.isFinite(number) && number > 0 ? number : null;
}

/**
 * Makes the new-side line numbers clickable, shows the person's comments
 * under their lines, and opens the form under the picked line (a comment, and
 * "Open in <editor>"). Text only. Calling it again first removes its rows.
 */
export function mountComments(root: HTMLElement, ops: CommentOps): void {
  root.querySelectorAll(`tr.${COMMENT}`).forEach((row) => row.remove());
  root.onclick = (event) => {
    const cell = (event.target as HTMLElement).closest("td.d2h-code-linenumber");
    const row = cell?.closest("tr");
    const line = row ? lineOf(row) : null;
    if (line !== null) ops.onpick(line);
  };
  for (const row of Array.from(root.querySelectorAll("tr"))) {
    if (row.classList.contains(COMMENT) || row.classList.contains(CLASS)) continue;
    const line = lineOf(row);
    if (line === null) continue;
    const cell = row.querySelector("td.d2h-code-linenumber");
    if (cell instanceof HTMLElement) {
      cell.style.cursor = "pointer";
      cell.title = "Comment on this line, or open it in your editor";
    }
    const here = ops.comments.filter((comment) => comment.line === line);
    const open = ops.composing === line;
    if (here.length === 0 && !open) continue;
    const extra = document.createElement("tr");
    extra.className = COMMENT;
    const td = document.createElement("td");
    td.colSpan = 2;
    const wrap = document.createElement("div");
    wrap.className = "vc-box";
    for (const comment of here) {
      const item = document.createElement("div");
      item.className = "vc-item";
      item.append(
        element("span", "vc-label", "Your comment"),
        element("span", "vc-text", comment.text),
        button("Remove", "vc-remove", () => ops.ondelete(comment.id)),
      );
      wrap.append(item);
    }
    if (open) {
      const form = document.createElement("div");
      form.className = "vc-form";
      const area = document.createElement("textarea");
      area.rows = 2;
      area.placeholder = `Comment on line ${line}: it goes into the fix request`;
      area.setAttribute("aria-label", `Comment on line ${line}`);
      const save = () => {
        if (area.value.trim()) ops.onsave(line, area.value);
      };
      area.addEventListener("keydown", (event) => {
        if (event.key === "Escape") ops.oncancel();
        if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) save();
        event.stopPropagation();
      });
      const actions = document.createElement("div");
      actions.className = "vc-actions";
      actions.append(button("Add comment", "vc-save", save), button("Cancel", "vc-cancel", ops.oncancel));
      for (const editor of ops.editors) {
        actions.append(button(`Open in ${editor.name}`, "vc-edit", () => ops.onedit(editor.name, line)));
      }
      form.append(area, actions);
      wrap.append(form);
      queueMicrotask(() => area.focus());
    }
    td.append(wrap);
    extra.append(td);
    row.after(extra);
  }
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
