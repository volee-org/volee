<script lang="ts">
  import { onMount } from "svelte";

  import { inline } from "./markdown";
  import { CATEGORY_TEXT, initialOpenFiles, patchOf, type WorkFile } from "./work";

  interface Props {
    files: WorkFile[];
    truncated: boolean;
  }
  let { files, truncated }: Props = $props();

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
      </button>
      {#if open[file.path]}
        {#if file.why}<!-- eslint-disable-next-line svelte/no-at-html-tags -->
          <p class="why">{@html inline(file.why)}</p>{/if}
        {#if file.binary}
          <p class="note">Binary file.</p>
        {:else if !file.patch}
          <p class="note">Not shown: the diff is too large.</p>
        {:else if render}
          <!-- diff2html escapes the code it shows. -->
          <!-- eslint-disable-next-line svelte/no-at-html-tags -->
          <div class="patch">{@html render(patchOf(file))}</div>
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
  .note {
    color: var(--muted);
    margin: 0;
    padding: 8px 12px;
  }
</style>
