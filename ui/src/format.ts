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

/** "3 files, +120 −8". */
export function diffSummary(files: number, insertions: number, deletions: number): string {
  const noun = files === 1 ? "file" : "files";
  return `${files} ${noun}, +${insertions} −${deletions}`;
}

/** The home directory shortened, for a path shown to a person. */
export function shortPath(path: string): string {
  return path.replace(/^\/(Users|home)\/[^/]+/, "~");
}
