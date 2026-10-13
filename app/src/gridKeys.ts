import type { MapView } from "./types";

export interface Pos {
  row: number;
  col: number;
}

const present = (map: MapView, row: number, col: number) =>
  map.rows[row]?.cells[col]?.kind === "present";

function scan(count: number, from: number, step: number, ok: (index: number) => boolean): number {
  for (let index = from + step; index >= 0 && index < count; index += step) {
    if (ok(index)) return index;
  }
  return -1;
}

/** Where a key moves focus from `from`, skipping absent cells; `null` when
 * the key is not a grid key or there is nowhere to go. */
export function move(map: MapView, from: Pos, key: string, ctrl = false): Pos | null {
  const rows = map.rows.length;
  const cols = map.envs.length;
  const inRow = (step: number, start = from.col) =>
    scan(cols, start, step, (col) => present(map, from.row, col));
  const inCol = (step: number) => scan(rows, from.row, step, (row) => present(map, row, from.col));
  const at = (row: number, col: number) => (row >= 0 && col >= 0 ? { row, col } : null);
  switch (key) {
    case "ArrowRight":
      return at(from.row, inRow(1));
    case "ArrowLeft":
      return at(from.row, inRow(-1));
    case "ArrowDown":
      return at(inCol(1), from.col);
    case "ArrowUp":
      return at(inCol(-1), from.col);
    case "Home":
      return ctrl ? first(map) : at(from.row, inRow(1, -1));
    case "End":
      return ctrl ? last(map) : at(from.row, inRow(-1, cols));
    default:
      return null;
  }
}

/** The first cell that can take focus, reading order. */
export function first(map: MapView): Pos | null {
  for (let row = 0; row < map.rows.length; row++) {
    for (let col = 0; col < map.envs.length; col++) {
      if (present(map, row, col)) return { row, col };
    }
  }
  return null;
}

function last(map: MapView): Pos | null {
  for (let row = map.rows.length - 1; row >= 0; row--) {
    for (let col = map.envs.length - 1; col >= 0; col--) {
      if (present(map, row, col)) return { row, col };
    }
  }
  return null;
}
