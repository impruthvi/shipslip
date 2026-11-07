import { actionConsequence } from "./DeployEntry";
import { TypedConfirm } from "./TypedConfirm";
import type { RecoveryReview } from "./types";

export type RecoveryBusy = "confirming" | "releasing" | null;

/** Attach or Up: exactly what will run, and the typed confirm for production. */
export function RecoveryConfirm({
  review,
  busy,
  onBack,
  onConfirm,
  onFromStep,
}: {
  review: RecoveryReview;
  busy: RecoveryBusy;
  onBack: () => void;
  onConfirm: (typedEnv: string | null) => void;
  /** Up only: release this review and prepare run-from-step instead. */
  onFromStep: (step: number) => void;
}) {
  const attach = review.operation === "attach";
  const watched = review.watched;
  const fromStep = review.fromStep;
  // A recipe step by number; `php artisan down` or `up` by its command.
  const watchedName = (step: string) =>
    watched?.index == null ? <code>{watched?.command}</code> : `${step} ${watched.index}`;

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
        {review.notRun.length > 0 && (
          <section className="workspace-section" aria-labelledby="recovery-not-run">
            <h2 id="recovery-not-run" className="caps">
              Did not run
            </h2>
            <p>These steps did not run. The app will come up without them.</p>
            <ol className="recipe mono">
              {review.notRun.map((step, i) => (
                <li key={i}>
                  <span className="recipe-index muted">{step.index}</span>
                  <code>{step.command}</code>
                </li>
              ))}
            </ol>
            {fromStep !== null && (
              <div className="recovery-action">
                <button
                  type="button"
                  className="button"
                  disabled={busy !== null}
                  aria-describedby="recovery-from-step-does"
                  onClick={() => onFromStep(fromStep)}
                >
                  Run from step {fromStep} instead…
                </button>
                <p id="recovery-from-step-does" className="muted recovery-does">
                  {actionConsequence({ kind: "fromStep", step: fromStep, command: "" })}
                </p>
              </div>
            )}
          </section>
        )}
        {!attach && <p className="workspace-section muted">Takes the deploy lock when you confirm.</p>}
      </div>
      <div className="workspace-footer">
        <button type="button" className="button" disabled={busy !== null} onClick={onBack}>
          Back
        </button>
        {busy === "releasing" && <span className="muted">Closing…</span>}
        <TypedConfirm
          expected={review.production ? review.env : null}
          verb="continue"
          action={
            busy === "confirming" ? "Starting…" : attach ? "Attach and continue" : "Bring app out of maintenance"
          }
          busy={busy !== null}
          onConfirm={onConfirm}
        />
      </div>
    </>
  );
}
