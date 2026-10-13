import type { MapView } from "./types";

/** A selected cell: its identity, and where it was in the map it came from. */
export interface Selection {
  repoRoot: string;
  env: string;
  row: number;
  col: number;
}

export type CellKey = Pick<Selection, "repoRoot" | "env">;

const STORAGE_KEY = "shipslip.selection";

function present(map: MapView, row: number, col: number): boolean {
  return map.rows[row]?.cells[col]?.kind === "present";
}

function at(map: MapView, row: number, col: number): Selection {
  return { repoRoot: map.rows[row].repoRoot, env: map.envs[col], row, col };
}

function nearest(count: number, from: number, ok: (index: number) => boolean): number {
  for (let distance = 0; distance < count; distance++) {
    for (const index of [from - distance, from + distance]) {
      if (index >= 0 && index < count && ok(index)) return index;
    }
  }
  return -1;
}

function find(map: MapView, key: CellKey): Selection | null {
  const row = map.rows.findIndex((r) => r.repoRoot === key.repoRoot);
  const col = map.envs.indexOf(key.env);
  return row >= 0 && col >= 0 && present(map, row, col) ? at(map, row, col) : null;
}

/** On launch: the remembered cell, else the first cell that needs attention
 * (rows whose checkout is gone are skipped), else the top-left cell. */
function launch(map: MapView, stored: CellKey | null): Selection | null {
  const remembered = stored && find(map, stored);
  if (remembered) return remembered;
  const cells = map.rows.flatMap((row, r) => row.cells.map((cell, c) => ({ row, cell, r, c })));
  const attention = cells.find(
    ({ row, cell }) => !row.pathMissing && cell.kind === "present" && cell.needsAttention,
  );
  const first = attention ?? cells.find(({ cell }) => cell.kind === "present");
  return first ? at(map, first.r, first.c) : null;
}

/** The selection for a new map. A cell that is gone gives way to the nearest
 * cell in its row, then in its column, then the launch choice. */
export function resolveSelection(
  map: MapView,
  prev: Selection | null,
  stored: CellKey | null,
): Selection | null {
  if (!prev) return launch(map, stored);
  const same = find(map, prev);
  if (same) return same;
  const row = map.rows.findIndex((r) => r.repoRoot === prev.repoRoot);
  const envCol = map.envs.indexOf(prev.env);
  if (row >= 0) {
    const col = nearest(map.envs.length, envCol >= 0 ? envCol : prev.col, (c) => present(map, row, c));
    if (col >= 0) return at(map, row, col);
  }
  if (envCol >= 0) {
    const r = nearest(map.rows.length, prev.row, (i) => present(map, i, envCol));
    if (r >= 0) return at(map, r, envCol);
  }
  return launch(map, null);
}

export function loadStored(): CellKey | null {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
    return typeof value?.repoRoot === "string" && typeof value?.env === "string" ? value : null;
  } catch {
    return null;
  }
}

export function store(selection: Selection | null) {
  if (selection) {
    const key: CellKey = { repoRoot: selection.repoRoot, env: selection.env };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(key));
  }
}
