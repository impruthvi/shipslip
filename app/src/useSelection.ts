import { useCallback, useEffect, useState } from "react";
import { loadStored, resolveSelection, store, type Selection } from "./selection";
import type { MapView } from "./types";

export function useSelection(map: MapView | null) {
  const [selected, setSelected] = useState<Selection | null>(null);

  useEffect(() => {
    if (!map) return;
    setSelected((prev) => {
      const next = resolveSelection(map, prev, prev ? null : loadStored());
      store(next);
      return next;
    });
  }, [map]);

  const select = useCallback(
    (row: number, col: number) => {
      const cell = map?.rows[row]?.cells[col];
      if (!map || cell?.kind !== "present") return;
      const next = { repoRoot: map.rows[row].repoRoot, env: map.envs[col], row, col };
      store(next);
      setSelected(next);
    },
    [map],
  );

  return { selected, select };
}
