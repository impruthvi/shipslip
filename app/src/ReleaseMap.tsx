import { Cell } from "./Cell";
import type { CompareLine, Pair } from "./compare";
import { SourceMarker, TargetMarker } from "./ComparePicker";
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
}

export function ReleaseMap({ map, selected = null, onSelect, compare = null }: Props) {
  const source = compare ? map.envs.indexOf(compare.pair.a) : -1;
  const target = compare ? map.envs.indexOf(compare.pair.b) : -1;
  return (
    <>
      <div className="grid-scroll">
        <div className="grid" role="grid" aria-label="Release map">
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
            <div className={`grid-row${row.pathMissing ? " row-gone" : ""}`} role="row" key={row.repoRoot}>
              <div className="grid-app" role="rowheader">
                <div className="app-name">{row.label}</div>
                <div className="mono muted app-path" title={row.repoRoot}>
                  {shortPath(row.repoRoot)}
                </div>
                {row.pathMissing && <div className="muted app-gone">checkout not found</div>}
              </div>
              {row.cells.map((cell, col) => {
                const isSelected = selected?.row === r && selected.col === col;
                return (
                  <div
                    className="grid-env"
                    role="gridcell"
                    key={col}
                    aria-selected={cell.kind === "present" ? isSelected : undefined}
                    onClick={cell.kind === "present" ? () => onSelect?.(r, col) : undefined}
                  >
                    {cell.kind === "present" && (
                      <Cell
                        cell={cell}
                        selected={isSelected}
                        compareLine={col === source ? compare?.lines[r] : undefined}
                        onOpen={onSelect && ((view) => onSelect(r, col, view))}
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
