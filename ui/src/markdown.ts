// A small, safe markdown renderer for what an agent writes: paragraphs,
// headings, bold, italic, inline code, fenced code, lists and http(s) links.
//
// Safety comes from the order: every piece of text is HTML-escaped BEFORE any
// tag is added, so raw HTML in the input can only ever be shown as text. The
// only attribute built from the input is a link's href, and only for
// http(s) addresses.

const ESCAPES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (char) => ESCAPES[char]!);
}

/** Inline markup of one already-trimmed line. */
export function inline(raw: string): string {
  const codes: string[] = [];
  // Code spans first, so nothing inside them is interpreted.
  let text = escapeHtml(raw.replaceAll("\u0000", "")).replace(/`([^`]+)`/g, (_, code: string) => {
    codes.push(`<code>${code}</code>`);
    return `\u0000${codes.length - 1}\u0000`;
  });
  text = text
    .replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, (_, label: string, url: string) => {
      return `<a href="${url}" target="_blank" rel="noopener noreferrer">${label}</a>`;
    })
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^*\w])\*([^*\s][^*]*)\*(?!\*)/g, "$1<em>$2</em>")
    .replace(/(^|[^\w])_([^_\s][^_]*)_(?!\w)/g, "$1<em>$2</em>");
  // eslint-disable-next-line no-control-regex
  return text.replace(/\u0000(\d+)\u0000/g, (_, index: string) => codes[Number(index)] ?? "");
}

type Block =
  | { kind: "p"; lines: string[] }
  | { kind: "ul" | "ol"; items: string[] }
  | { kind: "code"; text: string }
  | { kind: "h"; text: string };

function blocks(source: string): Block[] {
  const out: Block[] = [];
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;
    if (/^\s*```/.test(line)) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !/^\s*```/.test(lines[i]!)) body.push(lines[i++]!);
      i++; // the closing fence, when there is one
      out.push({ kind: "code", text: body.join("\n") });
      continue;
    }
    const heading = /^\s{0,3}#{1,6}\s+(.*)$/.exec(line);
    if (heading) {
      out.push({ kind: "h", text: heading[1]!.trim() });
      i++;
      continue;
    }
    const bullet = /^\s*[-*+]\s+(.*)$/.exec(line);
    const numbered = /^\s*\d+[.)]\s+(.*)$/.exec(line);
    if (bullet || numbered) {
      const kind = bullet ? "ul" : "ol";
      const pattern = bullet ? /^\s*[-*+]\s+(.*)$/ : /^\s*\d+[.)]\s+(.*)$/;
      const items: string[] = [];
      while (i < lines.length) {
        const match = pattern.exec(lines[i]!);
        if (match) {
          items.push(match[1]!);
        } else if (/^\s{2,}\S/.test(lines[i]!) && items.length > 0) {
          items[items.length - 1] += ` ${lines[i]!.trim()}`; // a wrapped item
        } else break;
        i++;
      }
      out.push({ kind, items });
      continue;
    }
    if (line.trim() === "") {
      i++;
      continue;
    }
    const paragraph: string[] = [];
    while (
      i < lines.length &&
      lines[i]!.trim() !== "" &&
      !/^\s*(```|#{1,6}\s|[-*+]\s|\d+[.)]\s)/.test(lines[i]!)
    ) {
      paragraph.push(lines[i++]!.trim());
    }
    out.push({ kind: "p", lines: paragraph });
  }
  return out;
}

/** Markdown to safe HTML. */
export function renderMarkdown(source: string): string {
  return blocks(source)
    .map((block) => {
      switch (block.kind) {
        case "code":
          return `<pre><code>${escapeHtml(block.text)}</code></pre>`;
        case "h":
          return `<p class="md-heading"><strong>${inline(block.text)}</strong></p>`;
        case "ul":
        case "ol":
          return `<${block.kind}>${block.items.map((item) => `<li>${inline(item)}</li>`).join("")}</${block.kind}>`;
        case "p":
          return `<p>${block.lines.map(inline).join("<br>")}</p>`;
      }
    })
    .join("");
}
