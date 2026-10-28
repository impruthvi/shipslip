import { useState, type FormEvent } from "react";
import { clock } from "./time";
import type { DeployReview } from "./types";

const FIRST_COMMITS = 5;

export type PreviewBusy = "confirming" | "releasing" | null;

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** What the reviewed deploy will do, and the typed confirm for production. */
export function Preview({
  review,
  lockedAt,
  busy,
  onBack,
  onDeploy,
}: {
  review: DeployReview;
  lockedAt: Date;
  busy: PreviewBusy;
  onBack: () => void;
  onDeploy: (typedEnv: string | null) => void;
}) {
  const [typed, setTyped] = useState("");
  const [allCommits, setAllCommits] = useState(false);
  const count = review.commits.length + review.moreCommits;
  const listed = allCommits ? review.commits : review.commits.slice(0, FIRST_COMMITS);
  const hidden = count - listed.length;
  const ready = !review.production || typed === review.env;
  const facts = [
    plural(review.steps.length, "step"),
    review.maintenance && "maintenance on",
    review.watchMs !== null && `log watch ${Math.round(review.watchMs / 60_000)} min`,
  ].filter(Boolean);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (ready && busy === null) onDeploy(review.production ? typed : null);
  };

  return (
    <>
      <div className="workspace-body">
        <div className="workspace-section">
          <p>
            <span className="mono preview-shas">
              {review.fromSha.slice(0, 7)} → {review.targetSha.slice(0, 7)}
            </span>{" "}
            <span className="muted">· {plural(count, "commit")}</span>
          </p>
          <p className="muted">{facts.join(" · ")}</p>
        </div>
        <section className="workspace-section" aria-labelledby="preview-commits">
          <h2 id="preview-commits" className="caps">
            Commits
          </h2>
          {count === 0 ? (
            <p>Already up to date</p>
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
        <section className="workspace-section" aria-labelledby="preview-steps">
          <h2 id="preview-steps" className="caps">
            Steps
          </h2>
          <ol className="recipe mono">
            {review.steps.map((step, i) => (
              <li key={i}>
                <span className="recipe-index muted">{i + 1}</span>
                <code>{step}</code>
              </li>
            ))}
          </ol>
        </section>
        <p className="workspace-section muted">
          Server lock held since {clock(lockedAt)} · Back releases it
        </p>
      </div>
      <form className="workspace-footer" onSubmit={submit}>
        <button type="button" className="button" disabled={busy !== null} onClick={onBack}>
          Back
        </button>
        {busy === "releasing" && <span className="muted">Releasing the server lock…</span>}
        {count > 0 && (
          <div className="footer-end">
            {review.production && (
              <label className="typed-field">
                Type {review.env} to deploy
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
              {busy === "confirming" ? "Starting…" : `Deploy to ${review.env}`}
            </button>
          </div>
        )}
      </form>
    </>
  );
}
