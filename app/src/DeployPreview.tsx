import { useState, type FormEvent } from "react";
import { firstStep } from "./run";
import { clock } from "./time";
import type { DeployReview, RunPlan } from "./types";

const FIRST_COMMITS = 5;

export type PreviewBusy = "confirming" | "releasing" | null;

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** The primary button: what confirming starts. */
export function planAction(plan: RunPlan, env: string): string {
  if (plan === "Deploy") return `Deploy to ${env}`;
  if (plan === "Rerun") return "Rerun all steps";
  return `Run from step ${plan.FromStep}`;
}

/** Schema changes Shipslip cannot take back. */
const MIGRATES = /\bartisan\s+migrate\b/;

/** What confirming risks, said before the confirm button. */
export function risks(review: DeployReview): string[] {
  const first = firstStep(review.runPlan);
  const lines: string[] = [];
  if (review.maintenance) {
    lines.push(
      "The app is in maintenance mode from before the first step until every step succeeds. If a step fails, it stays in maintenance mode.",
    );
  }
  review.steps.forEach((step, i) => {
    if (i + 1 >= first && MIGRATES.test(step)) {
      lines.push(`Step ${i + 1} changes the database (${step}). Shipslip cannot undo it.`);
    }
  });
  lines.push("A failed step stops the run. Nothing is retried or rolled back automatically.");
  return lines;
}

/** What the reviewed deploy will do, and the typed confirm for production. */
export function Preview({
  review,
  lockedAt,
  busy,
  onBack,
  onDeploy,
  onRerun,
}: {
  review: DeployReview;
  lockedAt: Date;
  busy: PreviewBusy;
  onBack: () => void;
  onDeploy: (typedEnv: string | null) => void;
  /** "Already up to date": release this preview and prepare a rerun. */
  onRerun: () => void;
}) {
  const [typed, setTyped] = useState("");
  const [allCommits, setAllCommits] = useState(false);
  const count = review.commits.length + review.moreCommits;
  const listed = allCommits ? review.commits : review.commits.slice(0, FIRST_COMMITS);
  const hidden = count - listed.length;
  const ready = !review.production || typed === review.env;
  const deploying = review.runPlan === "Deploy";
  const first = firstStep(review.runPlan);
  const running = deploying ? review.steps.length : review.steps.length - Math.max(first, 1) + 1;
  const facts = [
    plural(running, "step"),
    review.maintenance && "maintenance on",
    review.watchMs !== null && `log watch ${Math.round(review.watchMs / 60_000)} min`,
  ].filter(Boolean);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (ready && busy === null) onDeploy(review.production ? typed : null);
  };
  const canRun = !deploying || count > 0;

  return (
    <>
      <div className="workspace-body">
        <div className="workspace-section">
          {deploying ? (
            <p>
              <span className="mono preview-shas">
                {review.fromSha.slice(0, 7)} → {review.targetSha.slice(0, 7)}
              </span>{" "}
              <span className="muted">· {plural(count, "commit")}</span>
            </p>
          ) : (
            <p>
              On the deployed commit <span className="mono preview-shas">{review.targetSha.slice(0, 7)}</span>
            </p>
          )}
          <p className="muted">{facts.join(" · ")}</p>
        </div>
        {deploying && (
        <section className="workspace-section" aria-labelledby="preview-commits">
          <h2 id="preview-commits" className="caps">
            Commits
          </h2>
          {count === 0 ? (
            <p>
              Already up to date{" "}
              <button type="button" className="button" disabled={busy !== null} onClick={onRerun}>
                Rerun…
              </button>
            </p>
          ) : (
            <ul className="commit-list mono">
              {listed.map((commit, i) => (
                <li key={i}>
                  <code>{commit}</code>
                </li>
              ))}
            </ul>
          )}
          {hidden > 0 &&
            (allCommits || review.commits.length <= FIRST_COMMITS ? (
              <p className="muted">and {hidden} more</p>
            ) : (
              <button type="button" className="more-button" onClick={() => setAllCommits(true)}>
                and {hidden} more
              </button>
            ))}
        </section>
        )}
        <section className="workspace-section" aria-labelledby="preview-steps">
          <h2 id="preview-steps" className="caps">
            Steps
          </h2>
          <ol className="recipe mono">
            {review.maintenance && (
              <li>
                <span className="recipe-index muted">on</span>
                <code>php artisan down</code>
              </li>
            )}
            {deploying && (
              <li>
                <span className="recipe-index muted">0</span>
                <code>git merge --ff-only {review.targetSha.slice(0, 7)}</code>
              </li>
            )}
            {review.steps.map((step, i) => {
              const skipped = i + 1 < first;
              return (
                <li key={i} className={skipped ? "step-skipped" : undefined}>
                  <span className="recipe-index muted">{i + 1}</span>
                  <code>{step}</code>
                  {skipped && <span className="muted step-skip-note">skipped</span>}
                </li>
              );
            })}
            {review.maintenance && (
              <li>
                <span className="recipe-index muted">off</span>
                <code>php artisan up</code>
                <span className="muted step-skip-note">once every step succeeds</span>
              </li>
            )}
            {review.watchMs !== null && (
              <li>
                <span className="recipe-index muted">log</span>
                <span className="recipe-watch">watch the Laravel log for {Math.round(review.watchMs / 60_000)} min</span>
              </li>
            )}
          </ol>
        </section>
        {canRun && (
          <section className="workspace-section preview-risks" aria-labelledby="preview-risks">
            <h2 id="preview-risks" className="caps">
              Before you confirm
            </h2>
            <ul>
              {risks(review).map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          </section>
        )}
        <p className="workspace-section muted">
          Server lock held since {clock(lockedAt)} · Back releases it
        </p>
      </div>
      <form className="workspace-footer" onSubmit={submit}>
        <button type="button" className="button" disabled={busy !== null} onClick={onBack}>
          Back
        </button>
        {busy === "releasing" && <span className="muted">Releasing the server lock…</span>}
        {canRun && (
          <div className="footer-end">
            {review.production && (
              <label className="typed-field">
                Type {review.env} to {deploying ? "deploy" : "continue"}
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
              {busy === "confirming" ? "Starting…" : planAction(review.runPlan, review.env)}
            </button>
          </div>
        )}
      </form>
    </>
  );
}
