import { useEffect, useRef, useState } from "react";
import { api } from "./api";
import type { SlipView } from "./types";

/** After this, a slow read dims the old slip and names the new one. */
export const SLOW_OPEN_MS = 150;

export type Opened =
  | SlipView
  /** The cell has no receipt on this Mac. */
  | { kind: "none" }
  /** The id was not in the map it came from. */
  | { kind: "notFound" }
  | { kind: "failed"; message: string };

export interface Request<M> {
  generation: number;
  id: number | null;
  /** What is being opened: cell and pane view. A refresh keeps the key. */
  key: string;
  /** Kept with the answer, so a slip stays with its own cell while the next opens. */
  meta: M;
}

export interface ReceiptState<M> {
  shown: { key: string; opened: Opened; meta: M } | null;
  /** True while a different receipt has been opening for over SLOW_OPEN_MS. */
  opening: boolean;
}

/** Opens `request`'s receipt. Late answers are dropped; a stale generation
 * asks for a map reload, after which a new request arrives. */
export function useReceipt<M>(request: Request<M> | null, onStale: () => void): ReceiptState<M> {
  const [shown, setShown] = useState<ReceiptState<M>["shown"]>(null);
  const [opening, setOpening] = useState(false);
  const latest = useRef(0);
  const shownKey = useRef<string | null>(null);
  const staleRef = useRef(onStale);
  useEffect(() => {
    staleRef.current = onStale;
  }, [onStale]);

  const generation = request?.generation;
  const id = request?.id;
  const key = request?.key;
  const meta = useRef(request?.meta);
  useEffect(() => {
    meta.current = request?.meta;
  });

  useEffect(() => {
    const token = ++latest.current;
    if (key === undefined || generation === undefined || id === undefined) return;
    const forSlip = meta.current as M;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const show = (opened: Opened) => {
      if (token !== latest.current) return;
      clearTimeout(timer);
      shownKey.current = key;
      setShown({ key, opened, meta: forSlip });
      setOpening(false);
    };
    if (id === null) {
      show({ kind: "none" });
      return;
    }
    if (shownKey.current !== key) {
      timer = setTimeout(() => token === latest.current && setOpening(true), SLOW_OPEN_MS);
    }
    api.loadReceipt(generation, id).then(
      (result) => {
        switch (result.kind) {
          case "opened":
            return show(result.slip);
          case "notFound":
            return show({ kind: "notFound" });
          case "stale":
            if (token === latest.current) staleRef.current();
        }
      },
      (error: unknown) => show({ kind: "failed", message: String(error) }),
    );
    return () => clearTimeout(timer);
  }, [generation, id, key]);

  return { shown, opening };
}
