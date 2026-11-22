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
