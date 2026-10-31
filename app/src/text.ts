import { Fragment, createElement, type ReactNode } from "react";

/** Library sentences mark commands with backticks: show them as code, as text. */
export function withCode(text: string): ReactNode[] {
  return text
    .split("`")
    .map((part, i) =>
      i % 2 === 1 ? createElement("code", { key: i }, part) : createElement(Fragment, { key: i }, part),
    );
}

/** "Stopped in maintenance…" → "stopped in maintenance…", for mid-sentence use. */
export function lowerFirst(text: string): string {
  return text.charAt(0).toLowerCase() + text.slice(1);
}

/** `/Users/ana/code/shop` → `~/code/shop`, then the middle elided past `max`. */
/** The end of a path, which is what tells two checkouts apart. */
export function tailPath(path: string, max = 22): string {
  const home = path.replace(/^\/Users\/[^/]+(?=\/|$)/, "~");
  if (home.length <= max) return home;
  const parts = home.split("/");
  let tail = parts[parts.length - 1];
  if (tail.length + 2 > max) return `…${tail.slice(tail.length - max + 1)}`;
  // Whole folder names only, so the cut never lands mid-name.
  for (let i = parts.length - 2; i > 0 && `…/${parts[i]}/${tail}`.length <= max; i--) {
    tail = `${parts[i]}/${tail}`;
  }
  return `…/${tail}`;
}

/** The folder a checkout sits in. */
export function parentDir(path: string): string {
  const cut = path.replace(/\/+$/, "").lastIndexOf("/");
  return cut > 0 ? path.slice(0, cut) : path;
}

export function shortPath(path: string, max = 22): string {
  const home = path.replace(/^\/Users\/[^/]+(?=\/|$)/, "~");
  if (home.length <= max) {
    return home;
  }
  const keep = max - 1;
  const tail = Math.ceil(keep / 2);
  return `${home.slice(0, keep - tail)}…${home.slice(home.length - tail)}`;
}
