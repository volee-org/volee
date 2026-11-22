<script lang="ts">
  import { onMount, tick } from "svelte";

  import {
    addComment,
    askFix,
    autoMergeText,
    deleteComment,
    type EditorOption,
    fixItems,
    type JournalEntry,
    journalOf,
    type LineComment,
    mergeWork,
    openEditor,
  } from "./actions";
  import Diagram from "./Diagram.svelte";
  import DiffView from "./DiffView.svelte";
  import FixPanel from "./FixPanel.svelte";
  import { diffSummary } from "./format";
  import Journal from "./Journal.svelte";
  import Layer from "./Layer.svelte";
  import { inline, renderMarkdown } from "./markdown";
  import { briefKey, fetchReview, requestReview, type Review, type ReviewBrief, reviewHint } from "./review";
  import ReviewLayer from "./ReviewLayer.svelte";
  import { LEVELS, type RiskLevel, riskText, setRisk } from "./risk";
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
    /** The card's review brief, pushed with the inbox: it changing is the cue to read the review again. */
    brief?: ReviewBrief | null;
    /** Changes when the agent's change moved (a commit, a saved file): the diff is read again. */
    stamp?: string;
    /** What was done on finished changes, newest first. */
    journal?: JournalEntry[];
    now?: number;
    onback: () => void;
    onterminal: (id: string) => void;
  }
  let {
    token,
    id,
    title,
    terminal,
    brief = null,
    stamp = "",
    journal = [],
    now = Date.now(),
    onback,
    onterminal,
  }: Props = $props();

  let comments = $state<LineComment[]>([]);
  let editors = $state<EditorOption[]>([]);
  let fixOpen = $state(false);
  let merging = $state(false);
  let merged = $state(false);
  /** `m` once arms the merge, `m` again does it: a key must not merge by accident. */
  let armed = $state(false);
  let armTimer: ReturnType<typeof setTimeout> | undefined;
  let said = $state<{ ok: boolean; text: string } | null>(null);

  async function merge() {
    if (merging || merged) return;
    armed = false;
    merging = true;
    said = null;
    const result = await mergeWork(token, id);
    merging = false;
    merged = result.ok;
    said = { ok: result.ok, text: result.message };
  }

  function armMerge() {
    if (merging || merged) return;
    if (armed) {
      void merge();
      return;
    }
    armed = true;
    said = { ok: true, text: "Press m again to merge." };
    clearTimeout(armTimer);
    armTimer = setTimeout(() => {
      armed = false;
      if (said?.text === "Press m again to merge.") said = null;
    }, 4000);
  }

  async function sendFix(text: string) {
    return askFix(token, id, text);
  }

  async function saveComment(file: string, line: number, text: string) {
    const result = await addComment(token, id, { file, line, text });
    if (result.comment) comments = [...comments, result.comment];
    else said = { ok: false, text: result.message };
  }

  async function removeComment(commentId: number) {
    if (await deleteComment(token, id, commentId)) comments = comments.filter((comment) => comment.id !== commentId);
  }

  async function edit(editor: string, file: string, line: number) {
    const result = await openEditor(token, id, { editor, file, line });
    said = { ok: result.ok, text: result.message };
  }

  let review = $state<Review | null>(null);
  let reviewProblem = $state("");
  let asking = $state(false);
  let reviewOpened = false;

  const items = $derived(fixItems(review?.findings ?? [], comments));
  const itemsKey = $derived(`${comments.map((comment) => comment.id).join(",")}|${review?.findings.length ?? 0}`);

  /** Reads the review again; the layer opens once, the first time there is something to show. */
  async function readReview() {
    const result = await fetchReview(token, id);
    review = result.review;
    if (!result.review) reviewProblem = result.message;
    if (result.review && !reviewOpened && ["pending", "running", "failed", "done"].includes(result.review.status)) {
      reviewOpened = true;
      open.review = true;
    }
  }

  // The daemon pushes the card's brief over the events stream each time the
  // review moves (queued, running, done, failed): read the whole review then.
  // No polling.
  $effect(() => {
    void briefKey(brief);
    void readReview();
  });

  async function runReview(retry: boolean) {
    asking = true;
    reviewProblem = "";
    const result = await requestReview(token, id, retry);
    asking = false;
    if (!result.ok && result.message) reviewProblem = result.message;
    await readReview();
  }

  let work = $state<Work | null>(null);
  let problem = $state("");
  let open = $state<Record<LayerId, boolean>>({
    summary: true,
    review: false,
    plan: false,
    points: false,
    files: false,
    diagram: false,
    diff: false,
  });
  let focus = $state<LayerId | null>("summary");
  let riskProblem = $state("");
  let changingRisk = $state(false);

  /** Raises or lowers the risk of this tip by hand (null: back to the rules); the layers follow. */
  async function changeRisk(level: RiskLevel | null) {
    if (!work) return;
    changingRisk = true;
    riskProblem = "";
    const result = await setRisk(token, id, level);
    changingRisk = false;
    if (!result.risk) {
      riskProblem = result.message;
      return;
    }
    work = { ...work, risk: result.risk };
    open = initialOpen(work, review);
  }

  /** Reads the work again after a change of the agent's tree; what is open stays open. */
  async function reload() {
    const result = await fetchWork(token, id);
    if (!result.work) return;
    work = result.work;
    comments = result.work.comments ?? [];
    editors = result.work.editors ?? [];
  }

  onMount(async () => {
    const result = await fetchWork(token, id);
    if (!result.work) {
      problem = result.message;
      return;
    }
    work = result.work;
    comments = result.work.comments ?? [];
    editors = result.work.editors ?? [];
    const initial = initialOpen(result.work, review);
    open = { ...initial, review: initial.review || open.review };
  });

  let seenStamp: string | null = null;
  $effect(() => {
    const current = stamp;
    if (seenStamp !== null && seenStamp !== current && work) void reload();
    seenStamp = current;
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
      case "m":
        armMerge();
        break;
      case "f":
        fixOpen = true;
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
    {#if work.risk}
      <div class="risk {work.risk.level}" data-testid="risk" data-level={work.risk.level} data-overridden={work.risk.overridden}>
        <span class="level">{riskText(work.risk)}</span>
        <span class="why">{work.risk.reasons.join(" · ")}</span>
        <span class="control" role="group" aria-label="Set the risk by hand">
          {#each LEVELS as level (level)}
            <button
              type="button"
              data-level={level}
              class:current={work.risk.level === level}
              disabled={changingRisk || work.risk.level === level}
              onclick={() => changeRisk(level)}>{level}</button
            >
          {/each}
          {#if work.risk.overridden}
            <button type="button" data-testid="risk-reset" disabled={changingRisk} onclick={() => changeRisk(null)}
              >Back to the rules ({work.risk.computed})</button
            >
          {/if}
        </span>
        {#each work.risk.notes as note (note)}
          <p class="risknote">{note}</p>
        {/each}
        {#each work.risk.errors as error (error)}
          <p class="risknote failed">{error}</p>
        {/each}
        {#if riskProblem}<p class="risknote failed" role="alert">{riskProblem}</p>{/if}
      </div>
    {/if}
    <div class="actions" data-testid="actions">
      <div class="buttons">
        <button
          type="button"
          class="merge"
          data-testid="merge"
          disabled={merging || merged || work.agent?.finished_work === false}
          onclick={merge}
          title="The daemon merges it by the repository's method, with delivery evidence"
          >{merged ? "Merged" : merging ? "Merging…" : `Merge into ${work.diff.base ?? "the base"}`}</button
        >
        <button type="button" data-testid="ask-fix" aria-expanded={fixOpen} onclick={() => (fixOpen = !fixOpen)}>
          Ask for a fix{comments.length > 0 ? ` (${comments.length} ${comments.length === 1 ? "comment" : "comments"})` : ""}
        </button>
      </div>
      {#if work.risk}<p class="automerge" data-testid="automerge">{autoMergeText(work.risk.auto_merge)}</p>{/if}
      {#if said}
        <p class="said-action {said.ok ? 'ok' : 'failed'}" role="status" data-testid="action-message">{said.text}</p>
      {/if}
      {#if fixOpen}
        {#key itemsKey}
          <FixPanel
            {items}
            onsend={sendFix}
            onterminal={terminal ? () => onterminal(id) : null}
            onclose={() => (fixOpen = false)}
          />
        {/key}
      {/if}
    </div>
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
        id="review"
        title="AI review (an opinion)"
        hint={reviewHint(review)}
        open={open.review}
        focused={focus === "review"}
        ontoggle={() => toggle("review")}
      >
        <ReviewLayer {review} problem={reviewProblem} {asking} onrun={runReview} />
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
          <DiffView
            files={work.files}
            truncated={work.diff.truncated}
            findings={review?.findings ?? []}
            {comments}
            {editors}
            oncomment={saveComment}
            ondeletecomment={removeComment}
            onedit={edit}
          />
        {/if}
      </Layer>
    </div>

    <Journal entries={journalOf(journal, work.diff.branch)} {now} title="Done on this change" />

    <p class="keys">
      <kbd>j</kbd>/<kbd>k</kbd> layers · <kbd>Enter</kbd> fold · <kbd>n</kbd> next layer · <kbd>o</kbd> open all ·
      <kbd>c</kbd> close all · <kbd>m</kbd> <kbd>m</kbd> merge · <kbd>f</kbd> ask for a fix ·
      {#if terminal}<kbd>t</kbd> terminal · {/if}<kbd>Esc</kbd> back
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
  .actions {
    display: grid;
    gap: 8px;
  }
  .actions .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .actions button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 7px;
    padding: 5px 14px;
    cursor: pointer;
  }
  .actions button.merge {
    border-color: var(--finished);
    color: var(--finished);
    font-weight: 600;
  }
  .actions button:hover:not(:disabled),
  .actions button:focus-visible {
    border-color: var(--accent);
    outline: none;
  }
  .actions button:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .automerge {
    margin: 0;
    color: var(--muted);
    font-size: 0.85rem;
  }
  .said-action {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .said-action.ok {
    color: var(--finished);
  }
  .said-action.failed {
    color: var(--danger);
  }
  .risk {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px 12px;
    border: 1px solid var(--border);
    border-left-width: 4px;
    border-radius: 8px;
    padding: 8px 12px;
    background: var(--surface);
  }
  .risk.low {
    border-left-color: var(--finished);
  }
  .risk.medium {
    border-left-color: var(--waiting);
  }
  .risk.high {
    border-left-color: var(--danger);
  }
  .risk[data-overridden="true"] {
    border-style: dashed;
    border-left-style: solid;
  }
  .risk .level {
    font-weight: 600;
  }
  .risk .why {
    color: var(--muted);
    flex: 1;
    min-width: 12ch;
    overflow-wrap: anywhere;
  }
  .risk .control {
    display: inline-flex;
    gap: 6px;
  }
  .risk .control button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 6px;
    padding: 1px 9px;
    font-size: 0.82rem;
    cursor: pointer;
  }
  .risk .control button:hover:not(:disabled),
  .risk .control button:focus-visible {
    border-color: var(--accent);
    outline: none;
  }
  .risk .control button:disabled {
    cursor: default;
    opacity: 0.55;
  }
  .risknote {
    flex-basis: 100%;
    margin: 0;
    color: var(--muted);
    font-size: 0.85rem;
  }
  .risknote.failed {
    color: var(--danger);
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
