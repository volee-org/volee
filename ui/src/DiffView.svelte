<script lang="ts">
  import { onMount } from "svelte";

  import type { EditorOption, LineComment } from "./actions";
  import { annotate, mountComments } from "./inline";
  import { inline } from "./markdown";
  import { type Finding, findingsOf, placeFindings, SEVERITY_TEXT, whereText } from "./review";
  import { CATEGORY_TEXT, initialOpenFiles, patchOf, type WorkFile } from "./work";

  interface Props {
    files: WorkFile[];
    truncated: boolean;
    /** The AI review's findings: shown at their line, or on the file's header. */
    findings?: Finding[];
    /** The person's comments on lines of this change. */
    comments?: LineComment[];
    /** The editors the person set (rules `editors`). */
    editors?: EditorOption[];
    oncomment?: (file: string, line: number, text: string) => Promise<void> | void;
    ondeletecomment?: (id: number) => Promise<void> | void;
    onedit?: (editor: string, file: string, line: number) => Promise<void> | void;
  }
  let {
    files,
    truncated,
    findings = [],
    comments = [],
    editors = [],
    oncomment,
    ondeletecomment,
    onedit,
  }: Props = $props();

  const usable = $derived(editors.filter((editor) => editor.usable));
  /** The line whose comment form is open. */
  let composing = $state<{ path: string; line: number } | null>(null);

  /** What the diff of one file needs to draw the person's comments and their form. */
  function opsOf(path: string) {
    return {
      comments: comments.filter((comment) => comment.file === path),
      composing: composing?.path === path ? composing.line : null,
      editors: onedit ? usable : [],
      onpick: (line: number) => (composing = { path, line }),
      oncancel: () => (composing = null),
      onsave: async (line: number, text: string) => {
        await oncomment?.(path, line, text);
        composing = null;
      },
      ondelete: (id: number) => void ondeletecomment?.(id),
      onedit: (editor: string, line: number) => void onedit?.(editor, path, line),
    };
  }

  /** Where the findings of each file go, by file path. */
  const placed = $derived(
    Object.fromEntries(
      files.map((file) => [file.path, placeFindings(file.patch, findingsOf(findings, file.path))]),
    ),
  );

  // A file the review has something to say about opens, unless it was closed by hand.
  $effect(() => {
    for (const file of files) {
      const here = placed[file.path];
      if (here && (here.header.length > 0 || here.inline.size > 0) && open[file.path] === undefined) {
        open[file.path] = true;
      }
    }
  });

  /** Puts the inline findings into the rendered diff; again when they change. */
  type Placing = { byLine: Map<number, Finding[]>; html: string; ops: ReturnType<typeof opsOf> | null };
  function place(node: HTMLElement, params: Placing) {
    const draw = (given: Placing) => {
      annotate(node, given.byLine);
      if (given.ops) mountComments(node, given.ops);
    };
    draw(params);
    return { update: draw };
  }

  function countOf(path: string): number {
    const here = placed[path];
    if (!here) return 0;
    return here.header.length + [...here.inline.values()].reduce((sum, list) => sum + list.length, 0);
  }

  type Render = (patch: string) => string;

  let render = $state<Render | null>(null);
  let error = $state("");
  // svelte-ignore state_referenced_locally
  let open = $state<Record<string, boolean>>(
    Object.fromEntries([...initialOpenFiles(files)].map((path) => [path, true])),
  );

  function isDark(): boolean {
    const forced = document.documentElement.getAttribute("data-theme");
    if (forced) return forced === "dark";
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  }

  onMount(async () => {
    try {
      // diff2html and its styles are fetched only when this layer opens.
      const [{ html }] = await Promise.all([import("diff2html"), import("./diff2html.css")]);
      const colorScheme = isDark() ? "dark" : "light";
      render = (patch) =>
        html(patch, {
          drawFileList: false,
          matching: "lines",
          outputFormat: "line-by-line",
          colorScheme,
        } as Parameters<typeof html>[1]);
    } catch (cause) {
      error = String(cause);
    }
  });

  function toggle(path: string) {
    open[path] = !open[path];
  }

  function statusMark(file: WorkFile): string {
    return { added: "A", modified: "M", deleted: "D", renamed: "R" }[file.status];
  }
</script>

<div class="files" data-testid="diff">
  {#if error}
    <p class="note" role="alert">The diff viewer could not load: {error}</p>
  {/if}
  {#each files as file (file.path)}
    <article class="file" data-path={file.path} data-open={open[file.path]}>
      <button
        type="button"
        class="head"
        aria-expanded={open[file.path]}
        onclick={() => toggle(file.path)}
      >
        <span class="caret" aria-hidden="true">{open[file.path] ? "▾" : "▸"}</span>
        <span class="mark {file.status}" title={file.status}>{statusMark(file)}</span>
        <span class="path mono">
          {#if file.old_path}{file.old_path} → {/if}{file.path}
        </span>
        <span class="counts mono"><span class="add">+{file.insertions}</span> <span class="del">−{file.deletions}</span></span>
        <span class="chip">{CATEGORY_TEXT[file.category]}</span>
        {#if countOf(file.path) > 0}
          <span class="chip review" title="The AI review has something to say here">
            review: {countOf(file.path)}
          </span>
        {/if}
      </button>
      {#if onedit && usable.length > 0 && file.status !== "deleted"}
        <div class="tools" data-testid="file-editors">
          {#each usable as editor (editor.name)}
            <button type="button" class="open" onclick={() => onedit?.(editor.name, file.path, 1)}
              >Open in {editor.name}</button
            >
          {/each}
        </div>
      {/if}
      {#if open[file.path]}
        {#if file.why}<!-- eslint-disable-next-line svelte/no-at-html-tags -->
          <p class="why">{@html inline(file.why)}</p>{/if}
        {#if placed[file.path] && placed[file.path]!.header.length > 0}
          <ul class="onfile" data-testid="file-findings">
            {#each placed[file.path]!.header as finding, i (i)}
              <li class="vf-item {finding.severity}">
                <span class="vf-label">AI opinion</span>
                <span class="vf-sev">{SEVERITY_TEXT[finding.severity] ?? finding.severity}</span>
                <span class="vf-where">{whereText(finding)}</span>
                <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                <span class="vf-summary">{@html inline(finding.summary)}</span>
              </li>
            {/each}
          </ul>
        {/if}
        {#if file.binary}
          <p class="note">Binary file.</p>
        {:else if !file.patch}
          <p class="note">Not shown: the diff is too large.</p>
        {:else if render}
          <!-- diff2html escapes the code it shows. -->
          <div class="patch" use:place={{
              byLine: placed[file.path]?.inline ?? new Map(),
              html: file.patch,
              ops: oncomment ? opsOf(file.path) : null,
            }}>
            <!-- eslint-disable-next-line svelte/no-at-html-tags -->
            {@html render(patchOf(file))}
          </div>
          {#if file.truncated}<p class="note">This file's diff is cut: it is too long to show whole.</p>{/if}
        {:else if !error}
          <p class="note">Loading the viewer…</p>
        {/if}
      {/if}
    </article>
  {/each}
  {#if truncated}
    <p class="note">The change is larger than what is shown here; open it in your editor for the rest.</p>
  {/if}
</div>

<style>
  .files {
    display: grid;
    gap: 8px;
  }
  .file {
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
  }
  .head {
    all: unset;
    box-sizing: border-box;
    display: flex;
    align-items: baseline;
    gap: 8px;
    width: 100%;
    padding: 7px 10px;
    cursor: pointer;
    background: var(--bg);
  }
  .head:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .caret {
    color: var(--muted);
    width: 1em;
  }
  .mark {
    font-weight: 700;
    width: 1.2em;
    text-align: center;
    color: var(--waiting);
  }
  .mark.added {
    color: var(--finished);
  }
  .mark.deleted {
    color: var(--danger);
  }
  .path {
    flex: 1;
    overflow-wrap: anywhere;
  }
  .add {
    color: var(--finished);
  }
  .del {
    color: var(--danger);
  }
  .chip {
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 8px;
    font-size: 0.78rem;
    color: var(--muted);
  }
  .why {
    margin: 0;
    padding: 6px 12px;
    color: var(--muted);
    font-size: 0.88rem;
    border-bottom: 1px solid var(--border);
  }
  .patch {
    overflow-x: auto;
  }
  .tools {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 4px 10px;
    border-bottom: 1px solid var(--border);
  }
  .tools .open,
  .patch :global(.vc-box button) {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 6px;
    padding: 1px 9px;
    font-size: 0.8rem;
    cursor: pointer;
  }
  .tools .open:hover,
  .patch :global(.vc-box button:hover) {
    border-color: var(--accent);
  }
  /* The person's comments, inserted into the diff table by inline.ts. */
  .patch :global(tr.volee-comment > td) {
    padding: 6px 10px;
    background: var(--bg);
  }
  .patch :global(.vc-item) {
    display: flex;
    gap: 10px;
    align-items: baseline;
    border-left: 4px solid var(--accent);
    padding: 4px 10px;
    margin: 2px 0;
    font-family: system-ui, sans-serif;
    font-size: 0.88rem;
    color: var(--text);
    white-space: normal;
  }
  .patch :global(.vc-label) {
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
    font-weight: 700;
  }
  .patch :global(.vc-text) {
    flex: 1;
    overflow-wrap: anywhere;
  }
  .patch :global(.vc-form) {
    display: grid;
    gap: 6px;
    font-family: system-ui, sans-serif;
  }
  .patch :global(.vc-form textarea) {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    font-size: 0.88rem;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
  }
  .patch :global(.vc-actions) {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip.review {
    color: var(--waiting);
    border-color: var(--waiting);
  }
  .onfile {
    list-style: none;
    margin: 0;
    padding: 6px 12px;
    display: grid;
    gap: 6px;
    border-bottom: 1px solid var(--border);
  }
  .onfile .vf-item {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    align-items: baseline;
  }
  /* The findings inserted into the diff table by inline.ts. */
  .patch :global(tr.volee-finding > td) {
    padding: 6px 10px;
    background: var(--bg);
  }
  .patch :global(.vf-item) {
    border-left: 4px solid var(--danger);
    padding: 4px 10px;
    margin: 2px 0;
    font-family: system-ui, sans-serif;
    font-size: 0.88rem;
    color: var(--text);
    white-space: normal;
    text-align: left;
  }
  .patch :global(.vf-item.nit),
  .patch :global(.vf-item.pre_existing) {
    border-left-color: var(--muted);
  }
  .patch :global(.vf-head) {
    display: flex;
    gap: 10px;
    align-items: baseline;
  }
  :global(.vf-label),
  :global(.vf-sev) {
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
    font-weight: 700;
  }
  :global(.vf-where) {
    font-family: ui-monospace, Menlo, monospace;
    font-size: 0.85em;
  }
  .patch :global(.vf-scenario) {
    color: var(--muted);
  }
  .note {
    color: var(--muted);
    margin: 0;
    padding: 8px 12px;
  }
</style>
