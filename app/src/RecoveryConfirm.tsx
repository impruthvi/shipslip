import { useState, type FormEvent } from "react";
import type { RecoveryReview } from "./types";

export type RecoveryBusy = "confirming" | "releasing" | null;

/** Attach or Up: exactly what will run, and the typed confirm for production. */
export function RecoveryConfirm({
  review,
  busy,
  onBack,
  onConfirm,
}: {
  review: RecoveryReview;
  busy: RecoveryBusy;
  onBack: () => void;
  onConfirm: (typedEnv: string | null) => void;
}) {
  const [typed, setTyped] = useState("");
  const ready = !review.production || typed === review.env;
  const attach = review.operation === "attach";

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (ready && busy === null) onConfirm(review.production ? typed : null);
  };

  return (
    <>
      <div className="workspace-body">
        <div className="workspace-section">
          {attach ? (
            <>
              <p className="mono">
                Run {review.runId} · target {review.targetSha?.slice(0, 7)}
              </p>
              <p className="muted">Last phase: {review.phase}</p>
            </>
          ) : (
            <p>{review.phase}</p>
          )}
        </div>
        <section className="workspace-section" aria-labelledby="recovery-steps">
          <h2 id="recovery-steps" className="caps">
            Steps
          </h2>
          {review.remainingSteps.length === 0 ? (
            <p className="muted">No steps left to run</p>
          ) : (
            <ol className="recipe mono">
              {review.remainingSteps.map((step, i) => (
                <li key={i}>
                  <span className="recipe-index muted">{step.index ?? "off"}</span>
                  <code>{step.command}</code>
                </li>
              ))}
            </ol>
          )}
        </section>
      </div>
      <form className="workspace-footer" onSubmit={submit}>
        <button type="button" className="button" disabled={busy !== null} onClick={onBack}>
          Back
        </button>
        {busy === "releasing" && <span className="muted">Closing…</span>}
        <div className="footer-end">
          {review.production && (
            <label className="typed-field">
              Type {review.env} to continue
              <input
                value={typed}
                onChange={(event) => setTyped(event.target.value)}
                autoComplete="off"
                autoCapitalize="off"
                spellCheck={false}
                disabled={busy !== null}
              />
            </label>
          )}
          <button type="submit" className="button button-primary" disabled={!ready || busy !== null}>
            {busy === "confirming"
              ? "Starting…"
              : attach
                ? "Attach and continue"
                : "Bring app out of maintenance"}
          </button>
        </div>
      </form>
    </>
  );
}
