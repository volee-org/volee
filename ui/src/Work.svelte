<script lang="ts">
  import { onMount, tick } from "svelte";

  import Diagram from "./Diagram.svelte";
  import DiffView from "./DiffView.svelte";
  import { diffSummary } from "./format";
  import Layer from "./Layer.svelte";
  import { inline, renderMarkdown } from "./markdown";
  import {
    fetchWork,
    initialOpen,
    type LayerId,
    LAYERS,
    moveLayer,
    nextClosed,
    planSourceText,
    stepMark,
    testsText,
    type Work,
  } from "./work";

  interface Props {
    token: string;
    id: string;
    title: string;
    terminal: string | null;
    onback: () => void;
    onterminal: (id: string) => void;
  }
  let { token, id, title, terminal, onback, onterminal }: Props = $props();

  let work = $state<Work | null>(null);
  let problem = $state("");
  let open = $state<Record<LayerId, boolean>>({
    summary: true,
    plan: false,
    points: false,
    files: false,
    diagram: false,
    diff: false,
  });
  let focus = $state<LayerId | null>("summary");

  onMount(async () => {
    const result = await fetchWork(token, id);
    if (!result.work) {
      problem = result.message;
      return;
    }
    work = result.work;
    open = initialOpen(result.work);
  });

  async function reveal(layer: LayerId) {
    focus = layer;
    await tick();
    document
      .querySelector(`[data-layer="${layer}"] .head`)
      ?.scrollIntoView({ block: "nearest", behavior: "smooth" });
    (document.querySelector(`[data-layer="${layer}"] .head`) as HTMLElement | null)?.focus({
      preventScroll: true,
    });
  }

  function toggle(layer: LayerId) {
    open[layer] = !open[layer];
    focus = layer;
  }

  /** Keyboard: j/k move between layers, Enter folds, n opens the next closed layer. */
  function onkeydown(event: KeyboardEvent) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const target = event.target as HTMLElement | null;
    if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA")) return;
    switch (event.key) {
      case "Escape":
      case "b":
        onback();
        break;
      case "j":
      case "ArrowDown":
        void reveal(moveLayer(focus, 1));
        break;
      case "k":
      case "ArrowUp":
        void reveal(moveLayer(focus, -1));
        break;
      case "n": {
        const next = nextClosed(open, focus);
        if (next) {
          open[next] = true;
          void reveal(next);
        }
        break;
      }
      case "o":
        for (const layer of LAYERS) open[layer] = true;
        break;
      case "c":
        for (const layer of LAYERS) open[layer] = layer === "summary";
        break;
      case "t":
        if (terminal) onterminal(id);
        break;
      case "Enter":
      case " ":
        // A focused button already toggles itself on Enter and Space.
        if (target?.tagName === "BUTTON") return;
        if (focus) toggle(focus);
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  let commandsShown = $derived(work ? work.commands.slice(-12) : []);
</script>

<svelte:window {onkeydown} />

<div class="screen" data-testid="work">
  <header class="bar">
    <button type="button" class="back" onclick={onback}>← Back</button>
    <h1>{title}</h1>
    {#if terminal}
      <button type="button" class="terminal" onclick={() => onterminal(id)}>Open terminal</button>
    {/if}
  </header>

  {#if problem}
    <p class="error" role="alert">This work cannot be shown: {problem}</p>
  {:else if !work}
    <p class="note">Reading the change…</p>
  {:else}
    <p class="facts">
      {#if work.diff.base && work.diff.branch}
        <span class="mono">{work.diff.branch}</span> into <span class="mono">{work.diff.base}</span> ·
      {/if}
      {work.commits.length}
      {work.commits.length === 1 ? "commit" : "commits"} ·
      <span class="mono">{diffSummary(work.diff.files, work.diff.insertions, work.diff.deletions)}</span> ·
      {testsText(work.tests)}
    </p>
    {#if !work.transcript.available}
      <p class="degraded" role="status" data-testid="degraded">
        Transcript unavailable ({work.transcript.reason}). Showing what git and the hooks know: no plan and no
        reasons.
      </p>
    {/if}
    {#if work.diff.error}
      <p class="error" role="alert">The diff could not be read: {work.diff.error}</p>
    {/if}

    <div class="layers">
      <Layer id="summary" title="Summary" hint={work.summary.line} open={open.summary} focused={focus === "summary"} ontoggle={() => toggle("summary")}>
        <p class="line">{work.summary.line}</p>
        {#if work.summary.text && work.summary.text !== work.summary.line}
          <!-- eslint-disable-next-line svelte/no-at-html-tags -->
          <div class="said md">{@html renderMarkdown(work.summary.text)}</div>
          <p class="source">The agent's own closing message.</p>
        {:else if work.summary.source === "git"}
          <p class="source">Taken from the commits: the agent left no closing message.</p>
        {/if}
        {#if work.transcript.prompts.length > 0}
          <h3>Asked</h3>
          <ul class="asked">
            {#each work.transcript.prompts as prompt, i (i)}
              <li>{prompt.text}</li>
            {/each}
          </ul>
        {/if}
        {#if work.commits.length > 0}
          <h3>Commits</h3>
          <ul class="commits mono">
            {#each work.commits as commit, i (i)}
              <li><span class="sha">{commit.sha}</span> {commit.subject}</li>
            {/each}
          </ul>
        {/if}
      </Layer>

      <Layer
        id="plan"
        title="Plan followed"
        hint={work.plan ? `${work.plan.steps.length} steps` : "none recorded"}
        open={open.plan}
        focused={focus === "plan"}
        ontoggle={() => toggle("plan")}
      >
        {#if work.plan}
          <p class="source">{planSourceText(work.plan.source)}</p>
          {#if work.plan.steps.length > 0}
            <ol class="steps">
              {#each work.plan.steps as step, i (i)}
                <li class={step.status}><span class="mark" aria-label={step.status}>{stepMark(step.status)}</span> {step.text}</li>
              {/each}
            </ol>
          {/if}
          {#if work.plan.text}
            <!-- eslint-disable-next-line svelte/no-at-html-tags -->
            <div class="said md">{@html renderMarkdown(work.plan.text)}</div>
          {/if}
        {:else}
          <p class="note">
            {work.transcript.available
              ? "The agent kept no task list and proposed no plan."
              : "No plan: the transcript is unavailable."}
          </p>
        {/if}
      </Layer>

      <Layer
        id="points"
        title="Open points"
        hint={work.open_points.length === 0 ? "none noticed" : `${work.open_points.length}`}
        open={open.points}
        focused={focus === "points"}
        ontoggle={() => toggle("points")}
      >
        {#if work.open_points.length === 0}
          <p class="note">Nothing left undone was stated or noticed.</p>
        {:else}
          <ul class="points">
            {#each work.open_points as point, i (i)}
              <li><span class="chip {point.kind}">{point.kind === "stated" ? "said by the agent" : "noticed by volee"}</span>
                <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                <span>{@html inline(point.text)}</span>
              </li>
            {/each}
          </ul>
        {/if}
        {#if commandsShown.length > 0}
          <h3>Commands run</h3>
          <ul class="commands">
            {#each commandsShown as command, i (i)}
              <li>
                <span class="out {command.outcome}" title={command.outcome}>{command.outcome === "ok" ? "✓" : command.outcome === "failed" ? "✗" : "?"}</span>
                <span class="mono">{command.command}</span>
                {#if command.is_test}<span class="chip">test</span>{/if}
              </li>
            {/each}
          </ul>
        {/if}
      </Layer>

      <Layer
        id="files"
        title="Files and why"
        hint={`${work.files.length} ${work.files.length === 1 ? "file" : "files"}`}
        open={open.files}
        focused={focus === "files"}
        ontoggle={() => toggle("files")}
      >
        {#if work.files.length === 0}
          <p class="note">No file changed.</p>
        {:else}
          <ul class="why">
            {#each work.files as file, i (i)}
              <li>
                <span class="mono path">{file.path}</span>
                <span class="mono counts"><span class="add">+{file.insertions}</span> <span class="del">−{file.deletions}</span></span>
                {#if file.touched_by.length > 0}<span class="chip">{file.touched_by.join(", ")}</span>{/if}
                {#if file.why}
                  <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                  <span class="reason">{@html inline(file.why)}</span>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
        {#if work.other_touched.length > 0}
          <p class="note">
            Touched by the agent but not in the committed diff:
            <span class="mono">{work.other_touched.join(", ")}</span>
          </p>
        {/if}
      </Layer>

      <Layer id="diagram" title="Diagram of the change" hint="files by folder, new imports as arrows" open={open.diagram} focused={focus === "diagram"} ontoggle={() => toggle("diagram")}>
        <Diagram source={work.diagram} />
      </Layer>

      <Layer id="diff" title="Guided diff" hint="source first, tests next, lock files folded" open={open.diff} focused={focus === "diff"} ontoggle={() => toggle("diff")}>
        {#if work.files.length === 0}
          <p class="note">No diff to show.</p>
        {:else}
          <DiffView files={work.files} truncated={work.diff.truncated} />
        {/if}
      </Layer>
    </div>

    <p class="keys">
      <kbd>j</kbd>/<kbd>k</kbd> layers · <kbd>Enter</kbd> fold · <kbd>n</kbd> next layer · <kbd>o</kbd> open all ·
      <kbd>c</kbd> close all · {#if terminal}<kbd>t</kbd> terminal · {/if}<kbd>Esc</kbd> back
    </p>
  {/if}
</div>

<style>
  .screen {
    display: grid;
    gap: 12px;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  h1 {
    margin: 0;
    flex: 1;
    font-size: 1.15rem;
    overflow-wrap: anywhere;
  }
  h3 {
    margin: 14px 0 4px;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
  }
  button.back,
  button.terminal {
    background: transparent;
    border: 1px solid var(--accent);
    color: var(--accent);
    border-radius: 7px;
    padding: 4px 12px;
    cursor: pointer;
  }
  button.back:hover,
  button.terminal:hover,
  button.back:focus-visible,
  button.terminal:focus-visible {
    background: var(--accent);
    color: var(--accent-text);
    outline: none;
  }
  .facts {
    margin: 0;
    color: var(--muted);
  }
  .layers {
    display: grid;
    gap: 10px;
  }
  .line {
    margin: 0 0 8px;
    font-weight: 600;
    font-size: 1.02rem;
  }
  .said {
    background: var(--bg);
    border-radius: 8px;
    padding: 10px 12px;
    margin: 0;
    overflow-wrap: anywhere;
  }
  .source,
  .note {
    margin: 6px 0 0;
    color: var(--muted);
    font-size: 0.88rem;
  }
  .md :global(p) {
    margin: 0 0 8px;
  }
  .md :global(p:last-child) {
    margin-bottom: 0;
  }
  .md :global(ul),
  .md :global(ol) {
    margin: 0 0 8px;
  }
  .md :global(pre) {
    overflow-x: auto;
    background: var(--surface);
    padding: 8px 10px;
    border-radius: 6px;
  }
  .md :global(code),
  .points :global(code),
  .reason :global(code) {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.9em;
  }
  ul,
  ol {
    margin: 0;
    padding-left: 1.2em;
  }
  .steps {
    list-style: none;
    padding: 0;
    display: grid;
    gap: 4px;
  }
  .steps .mark {
    display: inline-block;
    width: 1.4em;
    color: var(--muted);
  }
  .steps .done .mark {
    color: var(--finished);
  }
  .steps .in-progress .mark {
    color: var(--waiting);
  }
  .points,
  .commands,
  .why {
    list-style: none;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  .commits {
    list-style: none;
    padding: 0;
  }
  .sha {
    color: var(--muted);
  }
  .chip {
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 8px;
    font-size: 0.78rem;
    color: var(--muted);
  }
  .chip.stated {
    color: var(--waiting);
    border-color: var(--waiting);
  }
  .out.ok {
    color: var(--finished);
  }
  .out.failed {
    color: var(--danger);
  }
  .why li {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    align-items: baseline;
  }
  .reason {
    flex-basis: 100%;
    color: var(--muted);
    font-size: 0.88rem;
  }
  .add {
    color: var(--finished);
  }
  .del {
    color: var(--danger);
  }
  .degraded,
  .error {
    margin: 0;
    border-radius: 8px;
    padding: 10px 14px;
    border: 1px solid var(--waiting);
    background: var(--waiting-bg);
  }
  .error {
    border-color: var(--danger);
  }
  .keys {
    color: var(--muted);
    font-size: 0.82rem;
    margin: 6px 0 0;
  }
  kbd {
    font: inherit;
    font-family: ui-monospace, Menlo, monospace;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 5px;
    background: var(--surface);
  }
</style>
