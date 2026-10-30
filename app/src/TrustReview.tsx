import { useState, type FormEvent } from "react";
import type { TrustReview as Review, TrustRow } from "./types";

function Value({ row, lines, mark }: { row: TrustRow; lines: string[]; mark: "+" | "−" | null }) {
  const sign = mark && (
    <span className="trust-mark muted" aria-hidden>
      {mark}
    </span>
  );
  if (row.steps) {
    return (
      <div className="trust-value">
        {sign}
        {lines.length === 0 ? (
          <span className="muted">none</span>
        ) : (
          <ol className="recipe mono">
            {lines.map((step, i) => (
              <li key={i}>
                <span className="recipe-index muted">{i + 1}</span>
                <code>{step}</code>
              </li>
            ))}
          </ol>
        )}
      </div>
    );
  }
  return (
    <div className="trust-value">
      {sign}
      <code className="mono">{lines.join(" ")}</code>
    </div>
  );
}

function Changed({ row }: { row: TrustRow }) {
  return (
    <li className="trust-row">
      <h3 className="trust-name mono">{row.name}</h3>
      <dl className="trust-values">
        {row.previous && (
          <>
            <dt className="muted">Previous</dt>
            <dd>
              <Value row={row} lines={row.previous} mark="−" />
            </dd>
          </>
        )}
        <dt className="muted">Proposed</dt>
        <dd>
          <Value row={row} lines={row.proposed} mark="+" />
        </dd>
      </dl>
    </li>
  );
}

export type TrustBusy = "approving" | null;

/** Every setting this env would run with, changed ones first, and the typed approval. */
export function TrustReview({
  review,
  busy,
  changedAgain,
  onBack,
  onApprove,
  onReviewAgain,
  onEdit,
}: {
  review: Review;
  busy: TrustBusy;
  /** The config changed after this review was shown. */
  changedAgain: boolean;
  onBack: () => void;
  onApprove: (typedEnv: string) => void;
  onReviewAgain: () => void;
  /** Opens `.shipslip.toml` in the editor; a saved change needs a new review.
   * Rejects with a `CommandError`. */
  onEdit: () => Promise<void>;
}) {
  const [typed, setTyped] = useState("");
  const [editError, setEditError] = useState<string | null>(null);
  const edit = () => {
    setEditError(null);
    onEdit().catch((error: unknown) =>
      setEditError(error && typeof error === "object" && "message" in error ? String(error.message) : String(error)),
    );
  };
  const [showUnchanged, setShowUnchanged] = useState(false);
  const changed = review.settings.filter((row) => row.changed);
  const unchanged = review.settings.filter((row) => !row.changed);
  const ready = typed === review.env && !changedAgain;

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (ready && busy === null) onApprove(typed);
  };

  return (
    <>
      <div className="workspace-body">
        <section className="workspace-section" aria-labelledby="trust-settings">
          <h2 id="trust-settings" className="caps">
            Settings
          </h2>
          <ul className="trust-rows">
            {changed.map((row) => (
              <Changed key={row.name} row={row} />
            ))}
          </ul>
          {unchanged.length > 0 && (
            <button
              type="button"
              className="more-button"
              aria-expanded={showUnchanged}
              onClick={() => setShowUnchanged(!showUnchanged)}
            >
              {unchanged.length} unchanged
            </button>
          )}
          {showUnchanged && (
            <ul className="trust-rows">
              {unchanged.map((row) => (
                <li key={row.name} className="trust-row">
                  <h3 className="trust-name mono">{row.name}</h3>
                  <Value row={row} lines={row.proposed} mark={null} />
                </li>
              ))}
            </ul>
          )}
          <p>
            <button type="button" className="more-button" onClick={edit}>
              Edit .shipslip.toml in your editor
            </button>
          </p>
          {editError && (
            <p className="muted" role="status">
              {editError}
            </p>
          )}
        </section>
        {changedAgain && (
          <p className="workspace-section">
            Config changed again; review the new version.{" "}
            <button type="button" className="button" onClick={onReviewAgain}>
              Review again
            </button>
          </p>
        )}
      </div>
      <form className="workspace-footer" onSubmit={submit}>
        <button type="button" className="button" disabled={busy !== null} onClick={onBack}>
          Back
        </button>
        <span className="muted footer-note">
          Approving lets you prepare a deploy. Deploying needs its own confirmation.
        </span>
        <div className="footer-end">
          <label className="typed-field">
            Type {review.env} to approve
            <input
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
              autoComplete="off"
              autoCapitalize="off"
              spellCheck={false}
              disabled={busy !== null}
            />
          </label>
          <button type="submit" className="button button-primary" disabled={!ready || busy !== null}>
            {busy === "approving" ? "Approving…" : "Approve configuration"}
          </button>
        </div>
      </form>
    </>
  );
}
