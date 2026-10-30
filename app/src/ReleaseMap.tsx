import { useRef, useState, type KeyboardEvent } from "react";
import { Cell, cellName } from "./Cell";
import type { CompareLine, Pair } from "./compare";
import { SourceMarker, TargetMarker } from "./ComparePicker";
import { first, move, type Pos } from "./gridKeys";
import { Legend } from "./Legend";
import type { PaneView } from "./slipTarget";
import type { Selection } from "./selection";
import { shortPath } from "./text";
import type { MapView } from "./types";

export interface Compare {
  pair: Pair;
  /** By row index. */
  lines: Record<number, CompareLine>;
}

interface Props {
  map: MapView;
  compare?: Compare | null;
  selected?: Selection | null;
  onSelect?: (row: number, col: number, view?: PaneView) => void;
  /** Enter or Space on a cell: select it and move focus to its receipt. */
  onEnter?: (row: number, col: number) => void;
  /** Focus moved to a cell (keys or mouse). */
  onCellFocus?: () => void;
  /** Outcome lines of cells with a run going now, by `repoRoot\nenv`. */
  live?: Record<string, string>;
}

/** The focused cell by identity, so it survives a refresh that moves rows. */
interface FocusKey {
  repoRoot: string;
  env: string;
}

function resolve(map: MapView, key: FocusKey | null, selected: Selection | null): Pos | null {
  if (key) {
    const row = map.rows.findIndex((r) => r.repoRoot === key.repoRoot);
    const col = map.envs.indexOf(key.env);
    if (row >= 0 && col >= 0 && map.rows[row].cells[col].kind === "present") return { row, col };
  }
  return selected ?? first(map);
}

export function ReleaseMap({
  map,
  selected = null,
  onSelect,
  onEnter,
  onCellFocus,
  compare = null,
  live = {},
}: Props) {
  const source = compare ? map.envs.indexOf(compare.pair.a) : -1;
  const target = compare ? map.envs.indexOf(compare.pair.b) : -1;
  const grid = useRef<HTMLDivElement>(null);
  const [focusKey, setFocusKey] = useState<FocusKey | null>(null);
  const active = resolve(map, focusKey, selected);

  const focusCell = ({ row, col }: Pos) => {
    grid.current?.querySelector<HTMLElement>(`[data-row="${row}"][data-col="${col}"]`)?.focus();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const cell = (event.target as HTMLElement).closest<HTMLElement>("[data-row]");
    if (!cell) return;
    const from = { row: Number(cell.dataset.row), col: Number(cell.dataset.col) };
    if (event.key === "Enter" || event.key === " ") {
      // A link inside the cell handles its own Enter.
      if (event.target !== cell) return;
      event.preventDefault();
      onEnter?.(from.row, from.col);
      return;
    }
    const to = move(map, from, event.key, event.ctrlKey || event.metaKey);
    if (to) {
      event.preventDefault();
      focusCell(to);
    } else if (event.key.startsWith("Arrow")) {
      event.preventDefault();
    }
  };

  return (
    <>
      <div className="grid-scroll">
        <div className="grid" role="grid" aria-label="Release map" ref={grid} onKeyDown={onKeyDown}>
          <div className="grid-heads" role="row">
            <div className="grid-app caps muted" role="columnheader">
              App
            </div>
            {map.envs.map((env, col) => (
              <div className="grid-env caps grid-head" role="columnheader" key={env}>
                {col === source && <SourceMarker />}
                {col === target && <TargetMarker />}
                {env}
              </div>
            ))}
          </div>
          {map.rows.map((row, r) => (
            <div
              className={`grid-row${row.pathMissing ? " row-gone" : ""}${selected?.row === r ? " grid-row-open" : ""}`}
              role="row"
              key={row.repoRoot}
            >
              <div className="grid-app" role="rowheader">
                <div className="app-name">{row.label}</div>
                <div className="mono muted app-path" title={row.repoRoot}>
                  {shortPath(row.repoRoot)}
                </div>
                {row.pathMissing && <div className="muted app-gone">checkout not found</div>}
              </div>
              {row.cells.map((cell, col) => {
                const isSelected = selected?.row === r && selected.col === col;
                const isPresent = cell.kind === "present";
                const compareLine = col === source ? compare?.lines[r] : undefined;
                const liveLine = live[`${row.repoRoot}\n${map.envs[col]}`];
                return (
                  <div
                    className="grid-env"
                    role="gridcell"
                    key={col}
                    data-row={isPresent ? r : undefined}
                    data-col={isPresent ? col : undefined}
                    tabIndex={isPresent ? (active?.row === r && active.col === col ? 0 : -1) : undefined}
                    aria-label={cellName(row.label, map.envs[col], cell, compareLine, liveLine)}
                    aria-selected={isPresent ? isSelected : undefined}
                    onFocus={
                      isPresent
                        ? () => {
                            setFocusKey({ repoRoot: row.repoRoot, env: map.envs[col] });
                            onCellFocus?.();
                          }
                        : undefined
                    }
                    onClick={isPresent ? () => onSelect?.(r, col) : undefined}
                  >
                    {cell.kind === "present" && (
                      <Cell
                        cell={cell}
                        selected={isSelected}
                        compareLine={compareLine}
                        onOpen={onSelect && ((view) => onSelect(r, col, view))}
                        live={liveLine}
                        expanded={selected?.row === r}
                      />
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      </div>
      <Legend compare={compare !== null} />
    </>
  );
}
