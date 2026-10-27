import { useEffect } from "react";
import { api } from "./api";

/** Reports the run whose receipt the pane shows, and again each time the
 * window gains focus: a run's Dock badge clears once it is seen focused. */
export function useSeen(runId: string | null) {
  useEffect(() => {
    if (runId === null) return;
    const seen = () => void api.receiptSeen(runId).catch(() => {});
    seen();
    let stop: (() => void) | undefined;
    let gone = false;
    void api.onFocus(seen).then((unlisten) => (gone ? unlisten() : (stop = unlisten)));
    return () => {
      gone = true;
      stop?.();
    };
  }, [runId]);
}
