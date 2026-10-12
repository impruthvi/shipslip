import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import { loadPair, SLOW_AFTER_MS, storePair, type CompareLine, type Pair } from "./compare";
import type { Loaded } from "./types";

interface Lines {
  /** The pair these lines are for. */
  pair: string;
  /** By checkout, so a refresh that moves rows keeps each row's answer. */
  byRepo: Record<string, CompareLine>;
}

const pairKey = (pair: Pair) => `${pair.a}\n${pair.b}`;

/** The picked pair and one compare line per row, for the shown map. A refresh
 * keeps the answers it already has while it asks again, so it stays silent. */
export function useCompare(loaded: Loaded | null) {
  const envs = loaded?.map.envs;
  const [pair, setPairState] = useState<Pair | null>(null);
  const [lines, setLines] = useState<Lines>({ pair: "", byRepo: {} });

  // A new map keeps the pair only if both envs are still there.
  useEffect(() => {
    if (!envs) return;
    setPairState((prev) => {
      const next = prev && envs.includes(prev.a) && envs.includes(prev.b) ? prev : loadPair(envs);
      if (!next) storePair(null);
      return next;
    });
  }, [envs]);

  const setPair = useCallback((next: Pair | null) => {
    storePair(next);
    setPairState(next);
  }, []);

  useEffect(() => {
    if (!loaded || !pair) {
      setLines({ pair: "", byRepo: {} });
      return;
    }
    const { generation, map } = loaded;
    const a = map.envs.indexOf(pair.a);
    const b = map.envs.indexOf(pair.b);
    if (a < 0 || b < 0) return;
    const key = pairKey(pair);
    let current = true;
    const update = (repoRoot: string, line: CompareLine) =>
      setLines((prev) => (prev.pair === key ? { pair: key, byRepo: { ...prev.byRepo, [repoRoot]: line } } : prev));
    setLines((prev) => ({
      pair: key,
      byRepo: Object.fromEntries(
        map.rows.map((row) => {
          const kept = prev.pair === key ? prev.byRepo[row.repoRoot] : undefined;
          return [row.repoRoot, kept?.status === "done" ? kept : { status: "pending" }];
        }),
      ),
    }));
    const timer = setTimeout(() => {
      if (!current) return;
      setLines((prev) => ({
        ...prev,
        byRepo: Object.fromEntries(
          Object.entries(prev.byRepo).map(([repo, line]) => [
            repo,
            line.status === "pending" ? { status: "slow" } : line,
          ]),
        ),
      }));
    }, SLOW_AFTER_MS);
    map.rows.forEach(({ repoRoot }, row) => {
      api.compare(generation, row, a, b).then(
        (result) => {
          const ours =
            result.kind === "compared" &&
            result.generation === generation &&
            result.row === row &&
            result.a === a &&
            result.b === b;
          if (!current || !ours) return;
          const { text, available } = result.comparison;
          update(repoRoot, { status: "done", text, available });
        },
        (error: unknown) => {
          if (!current) return;
          update(repoRoot, { status: "done", text: `cannot compare: ${String(error)}`, available: false });
        },
      );
    });
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [loaded, pair]);

  const rowLines: Record<number, CompareLine> = {};
  if (loaded && pair && lines.pair === pairKey(pair)) {
    loaded.map.rows.forEach((row, index) => {
      const line = lines.byRepo[row.repoRoot];
      if (line) rowLines[index] = line;
    });
  }
  return { pair, setPair, lines: rowLines };
}
