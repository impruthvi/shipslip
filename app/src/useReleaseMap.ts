import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import type { LoadResult, Loaded } from "./types";

export type MapState =
  | { phase: "loading" }
  /** No map has been read; the receipts root could not be read. */
  | { phase: "error"; message: string }
  | { phase: "shown"; loaded: Loaded; readAt: Date; failure: { message: string; at: Date } | null };

/** Folds one load answer into the state. Time moves only on a good read. */
export function applyLoad(prev: MapState, result: LoadResult, now: Date): MapState {
  switch (result.kind) {
    case "coalesced":
      return prev;
    case "loaded":
      if (prev.phase === "shown" && result.generation <= prev.loaded.generation) {
        return prev;
      }
      return { phase: "shown", loaded: result, readAt: now, failure: null };
    case "failed": {
      const failure = { message: result.message, at: now };
      if (prev.phase === "shown") {
        return { ...prev, failure };
      }
      if (result.current) {
        return { phase: "shown", loaded: result.current, readAt: now, failure };
      }
      return { phase: "error", message: result.message };
    }
  }
}

export function useReleaseMap() {
  const [state, setState] = useState<MapState>({ phase: "loading" });

  const refresh = useCallback(() => {
    api.loadMap().then(
      (result) => setState((prev) => applyLoad(prev, result, new Date())),
      (error: unknown) =>
        setState((prev) =>
          applyLoad(prev, { kind: "failed", message: String(error), current: null }, new Date()),
        ),
    );
  }, []);

  useEffect(() => {
    refresh();
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey && event.key.toLowerCase() === "r") {
        event.preventDefault();
        refresh();
      }
    };
    window.addEventListener("keydown", onKey);
    let unlisten: (() => void) | undefined;
    let gone = false;
    api.onFocus(refresh).then((stop) => (gone ? stop() : (unlisten = stop)));
    return () => {
      gone = true;
      unlisten?.();
      window.removeEventListener("keydown", onKey);
    };
  }, [refresh]);

  return { state, refresh };
}
