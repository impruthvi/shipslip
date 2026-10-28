import { useEffect, useState, type Ref } from "react";
import { deploy } from "./deployApi";
import type { DeployEntry } from "./types";

export interface EntryAt {
  generation: number;
  row: number;
  env: string;
}

/** Asks Rust what "Review deploy…" offers for the selected cell. */
export function useDeployEntry(at: EntryAt | null): DeployEntry | null {
  const key = at ? `${at.generation}\n${at.row}\n${at.env}` : null;
  const [answer, setAnswer] = useState<{ key: string; entry: DeployEntry } | null>(null);
  const generation = at?.generation;
  const row = at?.row;
  const env = at?.env;
  useEffect(() => {
    if (key === null || generation === undefined || row === undefined || env === undefined) return;
    let gone = false;
    deploy.entry(generation, row, env).then(
      (entry) => !gone && setAnswer({ key, entry }),
      // A stale map is reloaded, which asks again.
      () => !gone && setAnswer({ key, entry: { kind: "hidden" } }),
    );
    return () => {
      gone = true;
    };
  }, [key, generation, row, env]);
  return answer?.key === key ? answer.entry : null;
}

/** "Review deploy…" at the top of the receipt pane, or why it is off. */
export function DeployEntryBar({
  entry,
  onReview,
  buttonRef,
}: {
  entry: DeployEntry | null;
  onReview: () => void;
  buttonRef?: Ref<HTMLButtonElement>;
}) {
  if (!entry || entry.kind === "hidden") return null;
  const reason =
    entry.kind === "unavailable"
      ? entry.reason
      : entry.kind === "running"
        ? "A deploy is running for this env"
        : null;
  return (
    <div className="deploy-entry">
      <button
        type="button"
        ref={buttonRef}
        className="button"
        disabled={reason !== null}
        aria-describedby={reason ? "deploy-entry-reason" : undefined}
        onClick={onReview}
      >
        Review deploy…
      </button>
      {reason && (
        <p id="deploy-entry-reason" className="deploy-entry-reason muted">
          {reason}
        </p>
      )}
    </div>
  );
}
