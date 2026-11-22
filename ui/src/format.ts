/** "4 min", "2 h", "3 d": how long ago, at a glance. */
export function ago(since: number | null, now: number): string {
  if (since === null) return "unknown";
  const seconds = Math.max(0, Math.floor((now - since) / 1000));
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h`;
  return `${Math.round(hours / 24)} d`;
}

/** "waiting 4 min ago", "finished just now": what happened and how long ago. */
export function sinceText(verb: string, since: number | null, now: number): string {
  if (since === null) return verb;
  const how = ago(since, now);
  return how === "just now" ? `${verb} just now` : `${verb} ${how} ago`;
}

/** What a waiting card says it waits for. Claude Code's bare "blocked" says nothing. */
export function waitingText(waitingFor: string | null): string {
  if (!waitingFor || waitingFor === "blocked") return "Needs an answer from you";
  return `Waiting for ${waitingFor}`;
}

/** "3 files, +120 −8". */
export function diffSummary(files: number, insertions: number, deletions: number): string {
  const noun = files === 1 ? "file" : "files";
  return `${files} ${noun}, +${insertions} −${deletions}`;
}

/** The home directory shortened, for a path shown to a person. */
export function shortPath(path: string): string {
  return path.replace(/^\/(Users|home)\/[^/]+/, "~");
}

/** How a branch relates to its base, in words for a person. */
export interface Landing {
  /** "on master" when the agent works on the base itself, else "feat/x into main". */
  onBase: boolean;
  /** True when the base was guessed (nearest branch or repository default). */
  guessed: boolean;
  /** Why the base is a guess, for a tooltip. */
  why: string;
}

export function landing(branch: string | null, base: string | null, source: string | null): Landing {
  const guessed = source === "nearest" || source === "default";
  const why =
    source === "nearest"
      ? "Base guessed: the nearest branch"
      : source === "default"
        ? "Base guessed: the repository's default branch"
        : "";
  return { onBase: branch !== null && branch === base, guessed: guessed && base !== null, why };
}

/** "1:45", "12 min", "1 h 5 min": how long is left before the terminal takes over. */
export function remaining(expiresAt: number, now: number): string {
  const seconds = Math.max(0, Math.ceil((expiresAt - now) / 1000));
  if (seconds < 600) return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const rest = minutes % 60;
  return rest === 0 ? `${Math.floor(minutes / 60)} h` : `${Math.floor(minutes / 60)} h ${rest} min`;
}

/**
 * The answer sent for one question: the chosen labels, and any text the
 * person wrote under "Other". A multi-select answer joins its parts with a
 * comma and a space, which is how Claude Code words it itself.
 */
export function answerText(selected: string[], other: string, multi: boolean): string {
  const text = other.trim();
  if (!multi) return text !== "" ? text : (selected[0] ?? "");
  return [...selected, ...(text !== "" ? [text] : [])].join(", ");
}
