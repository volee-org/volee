import { describe, expect, it } from "vitest";

import { escapeHtml, inline, renderMarkdown } from "../src/markdown";

describe("markdown", () => {
  it("never lets raw HTML through", () => {
    const html = renderMarkdown('<script>alert(1)</script> <img src=x onerror="x()"> **ok**');
    expect(html).not.toContain("<script");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;script&gt;");
    expect(html).toContain("<strong>ok</strong>");
    expect(escapeHtml(`a&b<"'`)).toBe("a&amp;b&lt;&quot;&#39;");
  });

  it("renders the inline marks", () => {
    expect(inline("a `code **not bold**` b")).toBe("a <code>code **not bold**</code> b");
    expect(inline("**bold** and *it* and _it2_")).toBe("<strong>bold</strong> and <em>it</em> and <em>it2</em>");
    expect(inline("snake_case_name stays")).toBe("snake_case_name stays");
    expect(inline("2 * 3 * 4")).toBe("2 * 3 * 4");
  });

  it("links only http and https, with a safe rel", () => {
    expect(inline("[x](https://a.b/c?d=1&e=2)")).toBe(
      '<a href="https://a.b/c?d=1&amp;e=2" target="_blank" rel="noopener noreferrer">x</a>',
    );
    expect(inline("[x](javascript:alert(1))")).not.toContain("<a");
    expect(inline('[x](https://a.b/"onmouseover="y)')).not.toContain('"onmouseover');
    expect(inline("[x](data:text/html,hi)")).not.toContain("<a");
  });

  it("renders headings, lists, paragraphs and fenced code", () => {
    const html = renderMarkdown(
      "**Commits**\n- `a1b2` feat: x\n- second\n  wrapped\n\nText one\nline two\n\n1. first\n2. second\n\n```\n<b>raw</b>\n```\n\n## Title",
    );
    expect(html).toContain("<ul><li><code>a1b2</code> feat: x</li><li>second wrapped</li></ul>");
    expect(html).toContain("<p><strong>Commits</strong></p>");
    expect(html).toContain("<p>Text one<br>line two</p>");
    expect(html).toContain("<ol><li>first</li><li>second</li></ol>");
    expect(html).toContain("<pre><code>&lt;b&gt;raw&lt;/b&gt;</code></pre>");
    expect(html).toContain('<p class="md-heading"><strong>Title</strong></p>');
  });

  it("copes with odd input", () => {
    expect(renderMarkdown("")).toBe("");
    expect(renderMarkdown("```\nunclosed")).toBe("<pre><code>unclosed</code></pre>");
    expect(renderMarkdown("\u0000 weird \u00001\u0000")).toContain("weird");
  });
});
