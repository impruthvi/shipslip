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
  const watched = review.watched;
  // A recipe step by number; `php artisan down` or `up` by its command.
  const watchedName = (step: string) =>
    watched?.index == null ? <code>{watched?.command}</code> : `${step} ${watched.index}`;

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (ready && busy === null) onConfirm(review.production ? typed : null);
  };

  return (
    <>
      <div className="workspace-body">
        <div className="workspace-section">
          {attach ? (
            <p className="mono">
              Run {review.runId} · target {review.targetSha?.slice(0, 7)}
            </p>
          ) : (
            <p>{review.phase}</p>
          )}
        </div>
        {watched && (
          <section className="workspace-section" aria-labelledby="recovery-watched">
            <h2 id="recovery-watched" className="caps">
              Watched
            </h2>
            <p>{watchedName("Step")} was running. Shipslip will watch it until it finishes and will not run it again.</p>
            <ol className="recipe mono">
              <li>
                <span className="recipe-index muted">watch</span>
                <code>{watched.command}</code>
              </li>
            </ol>
          </section>
        )}
        <section className="workspace-section" aria-labelledby="recovery-steps">
          <h2 id="recovery-steps" className="caps">
            {attach ? "Steps to run" : "Steps"}
          </h2>
          {review.remainingSteps.length === 0 ? (
            <p className="muted">No steps left to run</p>
          ) : (
            <>
              {watched && <p className="muted">If {watchedName("step")} succeeds, these run next:</p>}
              <ol className="recipe mono">
                {review.remainingSteps.map((step, i) => (
                  <li key={i}>
                    <span className="recipe-index muted">{step.index ?? "off"}</span>
                    <code>{step.command}</code>
                  </li>
                ))}
              </ol>
            </>
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
