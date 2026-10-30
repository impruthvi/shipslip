import { useEffect, useState, type Ref } from "react";
import { deploy } from "./deployApi";
import { withCode } from "./text";
import type { DeployEntry, RecoveryAction } from "./types";

export interface EntryAt {
  generation: number;
  row: number;
  env: string;
}

/** Asks Rust what "Review deploy…" offers for the selected cell. A new
 * `version` asks again, e.g. after an approval or a run ends. */
export function useDeployEntry(at: EntryAt | null, version = 0): DeployEntry | null {
  const cell = at ? `${at.generation}\n${at.row}\n${at.env}` : null;
  const key = cell === null ? null : `${cell}\n${version}`;
  const [answer, setAnswer] = useState<{ cell: string; entry: DeployEntry } | null>(null);
  const generation = at?.generation;
  const row = at?.row;
  const env = at?.env;
  useEffect(() => {
    if (cell === null || generation === undefined || row === undefined || env === undefined) return;
    let gone = false;
    deploy.entry(generation, row, env).then(
      (entry) => !gone && setAnswer({ cell, entry }),
      // A stale map is reloaded, which asks again.
      () => !gone && setAnswer({ cell, entry: { kind: "hidden" } }),
    );
    return () => {
      gone = true;
    };
  }, [key, cell, generation, row, env]);
  // While asking again, the last answer for the same cell stays up.
  return answer?.cell === cell ? answer.entry : null;
}

export function actionLabel(action: RecoveryAction): string {
  switch (action.kind) {
    case "fromStep":
      return `Run from step ${action.step}…`;
    case "attach":
      return "Attach…";
    case "up":
      return "Bring app out of maintenance…";
  }
}

export type EntryChoice =
  | { kind: "review" }
  | { kind: "rerun" }
  | { kind: "fromStep"; step: number }
  | { kind: "attach" }
  | { kind: "up" };

/** "Review deploy…" at the top of the receipt pane, or why it is off; above it,
 * what the env's newest run left to do. */
export function DeployEntryBar({
  entry,
  notice,
  onChoose,
  onOpenConfig,
  buttonRef,
}: {
  entry: DeployEntry | null;
  /** A pencil line: an approval, or a preview a reload closed. */
  notice?: string | null;
  onChoose: (choice: EntryChoice) => void;
  /** Opens `.shipslip.toml`; rejects with a `CommandError`. */
  onOpenConfig: () => Promise<void>;
  buttonRef?: Ref<HTMLButtonElement>;
}) {
  const [menu, setMenu] = useState(false);
  const [configError, setConfigError] = useState<string | null>(null);
  const openConfig = () => {
    setConfigError(null);
    onOpenConfig().catch((error: unknown) =>
      setConfigError(error && typeof error === "object" && "message" in error ? String(error.message) : String(error)),
    );
  };
  if (!entry || entry.kind === "hidden") return null;
  const reason =
    entry.kind === "unavailable"
      ? entry.reason
      : entry.kind === "running"
        ? "A deploy is running for this env"
        : null;
  const ready = entry.kind === "ready" ? entry : null;
  const recovery = ready?.recovery ?? null;
  return (
    <div className="deploy-entry">
      {notice && <p className="deploy-entry-notice muted">{notice}</p>}
      {recovery && (
        <section className="recovery" aria-label="What the last run left">
          <p className="recovery-sentence">{withCode(recovery.sentence)}</p>
          {recovery.advice && <p className="muted">{withCode(recovery.advice)}</p>}
          {recovery.actions.map((action, i) => (
            <div className="recovery-action" key={action.kind}>
              <button
                type="button"
                className={`button${i === 0 ? " button-primary" : ""}`}
                onClick={() =>
                  onChoose(action.kind === "fromStep" ? { kind: "fromStep", step: action.step } : { kind: action.kind })
                }
              >
                {actionLabel(action)}
              </button>
              <p className="mono muted recovery-command">{action.command}</p>
            </div>
          ))}
        </section>
      )}
      <div className="deploy-entry-row">
        <button
          type="button"
          ref={buttonRef}
          className="button"
          disabled={reason !== null}
          aria-describedby={reason ? "deploy-entry-reason" : undefined}
          onClick={() => onChoose({ kind: "review" })}
        >
          Review deploy…
        </button>
        {ready && (
          <button
            type="button"
            className="button"
            aria-expanded={menu}
            aria-controls="deploy-entry-more"
            onClick={() => setMenu(!menu)}
          >
            More <span aria-hidden>▾</span>
          </button>
        )}
      </div>
      {ready && menu && (
        <div id="deploy-entry-more" className="deploy-entry-more" role="group" aria-label="More">
          {ready.steps.length > 0 && (
            <>
              <button type="button" className="button" onClick={() => onChoose({ kind: "rerun" })}>
                Rerun…
              </button>
              <button
                type="button"
                className="button"
                onClick={() => {
                  const from = recovery?.actions.find((a) => a.kind === "fromStep");
                  onChoose({ kind: "fromStep", step: from?.kind === "fromStep" ? from.step : 1 });
                }}
              >
                Run from step…
              </button>
            </>
          )}
          <button type="button" className="button" onClick={openConfig}>
            Open config
          </button>
        </div>
      )}
      {configError && (
        <p className="deploy-entry-reason muted" role="status">
          {configError}
        </p>
      )}
      {reason && (
        <p id="deploy-entry-reason" className="deploy-entry-reason muted">
          {reason}
        </p>
      )}
    </div>
  );
}
