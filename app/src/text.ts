/** "Stopped in maintenance…" → "stopped in maintenance…", for mid-sentence use. */
export function lowerFirst(text: string): string {
  return text.charAt(0).toLowerCase() + text.slice(1);
}

/** `/Users/ana/code/shop` → `~/code/shop`, then the middle elided past `max`. */
export function shortPath(path: string, max = 28): string {
  const home = path.replace(/^\/Users\/[^/]+(?=\/|$)/, "~");
  if (home.length <= max) {
    return home;
  }
  const keep = max - 1;
  const tail = Math.ceil(keep / 2);
  return `${home.slice(0, keep - tail)}…${home.slice(home.length - tail)}`;
}
