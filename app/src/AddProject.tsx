import { useEffect, useRef, useState } from "react";
import { api } from "./api";
import { deploy } from "./deployApi";
import { StepIcon } from "./Mark";
import { TrustSettings } from "./TrustReview";
import { TypedConfirm } from "./TypedConfirm";
import type { FolderEnv, FolderView, Loaded, TrustReview } from "./types";

/** The check step lands with the connection check; until then an existing
 * config goes straight from folder to approval. */
type Step = "folder" | "approval";
const steps: { step: Step; label: string }[] = [
  { step: "folder", label: "Folder" },
  { step: "approval", label: "Approval" },
];

const message = (error: unknown) =>
  error && typeof error === "object" && "message" in error ? String(error.message) : String(error);

function FolderResult({ folder }: { folder: FolderView }) {
  switch (folder.kind) {
    case "cancelled":
      return null;
    case "notGit":
      return (
        <p className="sheet-refusal" role="alert">
          <span className="mono">{folder.folder}</span> is not inside a git repository. Shipslip deploys
          from a git checkout, so choose the folder of the app's repository.
        </p>
      );
    case "noConfig":
      return (
        <p className="sheet-refusal" role="alert">
          This checkout has no .shipslip.toml yet. Run <code className="mono">slip init</code> in{" "}
          <span className="mono">{folder.checkout}</span>, then choose it again.
        </p>
      );
    case "unreadable":
      return (
        <div className="sheet-refusal" role="alert">
          <p>Shipslip can't read this checkout's .shipslip.toml. Fix it, then choose the folder again.</p>
          <pre className="sheet-detail mono">{folder.message.join("\n")}</pre>
        </div>
      );
    case "config":
      return (
        <section className="sheet-section" aria-labelledby="sheet-envs">
          <h3 id="sheet-envs" className="caps muted">
            Environments in .shipslip.toml
          </h3>
          <p className="muted">Read from the file. Edit .shipslip.toml to change them.</p>
          <ul className="sheet-envs">
            {folder.envs.map((env) => (
              <EnvLine key={env.name} env={env} />
            ))}
          </ul>
        </section>
      );
  }
}

function EnvLine({ env }: { env: FolderEnv }) {
  return (
    <li className="sheet-env">
      <span className="sheet-env-name mono">{env.name}</span>
      <span className="sheet-env-target mono muted">
        {env.target} · {env.branch}
        {env.production && " · production"}
      </span>
      <span className={env.approved ? "muted" : "tone-attention"}>{env.approved ? "Approved" : "Needs approval"}</span>
    </li>
  );
}

/** One environment's approval: today's settings review, typed. */
type Approval =
  | { kind: "loading"; env: string }
  | { kind: "review"; review: TrustReview; busy: boolean; error: string | null }
  | { kind: "failed"; env: string; error: string };

/**
 * The Add project sheet: pick a folder, then approve its `.shipslip.toml`.
 * The checkout is added when the approval step opens, so "Finish later" from
 * there leaves it listed as "Needs approval".
 */
export function AddProject({
  loaded,
  onAdded,
  onClose,
  onDone,
}: {
  /** The map now shown; the approval reads the new checkout's row from it. */
  loaded: Loaded | null;
  /** The checkout is on the projects list: the map should be read again. */
  onAdded: () => void;
  /** Finish later. */
  onClose: () => void;
  /** Every environment approved: open the checkout. */
  onDone: (checkout: string) => void;
}) {
  const [step, setStep] = useState<Step>("folder");
  const [folder, setFolder] = useState<FolderView | null>(null);
  const [picking, setPicking] = useState(false);
  const [adding, setAdding] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);
  const [checkout, setCheckout] = useState<string | null>(null);
  /** Environments still to approve, in config order; the first is shown. */
  const [pending, setPending] = useState<string[]>([]);
  const [approval, setApproval] = useState<Approval | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);



  const choose = () => {
    setPicking(true);
    setAddError(null);
    api
      .pickFolder()
      .then((picked) => {
        if (!alive.current) return;
        // Closing the panel keeps what was picked before.
        if (picked.kind !== "cancelled") setFolder(picked);
      })
      .catch((error: unknown) => alive.current && setAddError(message(error)))
      .finally(() => alive.current && setPicking(false));
  };

  const next = () => {
    if (folder?.kind !== "config") return;
    setAdding(true);
    setAddError(null);
    api
      .addCheckout()
      .then((added) => {
        if (!alive.current) return;
        setCheckout(added);
        setPending(folder.envs.filter((env) => !env.approved).map((env) => env.name));
        setApproval(null);
        setStep("approval");
        onAdded();
      })
      .catch((error: unknown) => alive.current && setAddError(message(error)))
      .finally(() => alive.current && setAdding(false));
  };

  const row = checkout && loaded ? loaded.map.rows.findIndex((r) => r.repoRoot === checkout) : -1;
  const env = pending[0] ?? null;
  // Each step, and each environment approved in turn, focuses its heading.
  useEffect(() => {
    heading.current?.focus();
  }, [step, env]);
  const onMap = row >= 0 && loaded !== null && env !== null && loaded.map.envs.includes(env);

  // The review asks once per environment; a newer map read keeps it open.
  const latest = useRef({ loaded, row });
  useEffect(() => {
    latest.current = { loaded, row };
  });
  const [attempt, setAttempt] = useState(0);
  const reviewFor = step === "approval" && onMap ? env : null;
  useEffect(() => {
    const { loaded: map, row: at } = latest.current;
    if (reviewFor === null || !map) return;
    let current = true;
    setApproval({ kind: "loading", env: reviewFor });
    deploy
      .reviewTrust(map.generation, at, reviewFor)
      .then((r) => current && setApproval({ kind: "review", review: r, busy: false, error: null }))
      .catch((error: unknown) => current && setApproval({ kind: "failed", env: reviewFor, error: message(error) }));
    return () => {
      current = false;
    };
  }, [reviewFor, attempt]);
  const review = () => setAttempt((n) => n + 1);

  const approve = () => {
    if (approval?.kind !== "review" || checkout === null) return;
    const { review: shown } = approval;
    setApproval({ ...approval, busy: true, error: null });
    deploy
      .approveTrust(shown.handle, shown.env)
      .then(() => {
        if (!alive.current) return;
        const rest = pending.slice(1);
        if (rest.length === 0) onDone(checkout);
        else setPending(rest);
      })
      .catch(
        (error: unknown) =>
          alive.current && setApproval({ kind: "review", review: shown, busy: false, error: message(error) }),
      );
  };

  const done = (s: Step) => steps.findIndex((x) => x.step === s) < steps.findIndex((x) => x.step === step);
  const path = folder && folder.kind !== "cancelled" ? ("checkout" in folder ? folder.checkout : folder.folder) : null;
  const busy = picking || adding || (approval?.kind === "review" && approval.busy);
  // Escape is Finish later, and like it waits while something is in flight.
  const busyRef = useRef(false);
  useEffect(() => {
    busyRef.current = busy;
  });
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || busyRef.current) return;
      event.preventDefault();
      onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const last = pending.length <= 1;

  return (
    <div className="sheet-backdrop">
      <div role="dialog" aria-modal="true" aria-labelledby="sheet-title" className="sheet">
        <nav className="sheet-rail" aria-label="Add project steps">
          <h2 id="sheet-title" className="sheet-title">
            Add project
          </h2>
          {path && (
            <p className="sheet-path mono muted" title={path}>
              {path}
            </p>
          )}
          <ol className="sheet-steps">
            {steps.map(({ step: s, label }, index) => {
              const isDone = done(s);
              const content = (
                <>
                  <span className="sheet-step-icon">
                    {isDone ? <StepIcon kind="ok" label="done" /> : <span className="muted">{index + 1}</span>}
                  </span>
                  {label}
                </>
              );
              return (
                <li key={s}>
                  {isDone && !busy ? (
                    <button type="button" className="sheet-step" onClick={() => setStep(s)}>
                      {content}
                    </button>
                  ) : (
                    <span className="sheet-step" aria-current={s === step ? "step" : undefined}>
                      {content}
                    </span>
                  )}
                </li>
              );
            })}
          </ol>
        </nav>
        <div className="sheet-main">
          <div className="sheet-body">
            {step === "folder" ? (
              <>
                <h3 className="sheet-heading" ref={heading} tabIndex={-1}>
                  Choose the project's folder
                </h3>
                <p className="muted">
                  The folder of a Laravel app's git checkout on this Mac. Shipslip reads its .shipslip.toml.
                </p>
                <p>
                  <button type="button" className="button" onClick={choose} disabled={busy}>
                    {picking ? "Choosing…" : folder ? "Choose another folder…" : "Choose folder…"}
                  </button>
                </p>
                {folder && <FolderResult folder={folder} />}
              </>
            ) : (
              <>
                <h3 className="sheet-heading" ref={heading} tabIndex={-1}>
                  {env ? `Approve ${env}` : "Add project"}
                </h3>
                {env === null ? (
                  <p className="muted">Every environment in .shipslip.toml is already approved on this Mac.</p>
                ) : (
                  <p className="muted">
                    Deploys use these settings only once you approve them.
                    {pending.length > 1 && ` ${pending.length} environments to approve.`}
                  </p>
                )}
                {env !== null && (approval?.kind === "loading" || !onMap) && (
                  <p className="muted" role="status">
                    {onMap ? "Reading .shipslip.toml…" : "Waiting for the map to list this checkout…"}
                  </p>
                )}
                {approval?.kind === "failed" && (
                  <p className="sheet-refusal" role="alert">
                    {approval.error}{" "}
                    <button type="button" className="link" onClick={review}>
                      Try again
                    </button>
                  </p>
                )}
                {approval?.kind === "review" && (
                  <TrustSettings
                    review={approval.review}
                    headingId="sheet-settings"
                    onEdit={() =>
                      loaded && row >= 0 && env ? deploy.openConfig(loaded.generation, row, env) : Promise.resolve()
                    }
                  />
                )}
                {approval?.kind === "review" && approval.error && (
                  <p className="sheet-refusal" role="alert">
                    {approval.error}{" "}
                    <button type="button" className="link" onClick={review}>
                      Review again
                    </button>
                  </p>
                )}
              </>
            )}
            {addError && (
              <p className="sheet-refusal" role="alert">
                {addError}
              </p>
            )}
          </div>
          <div className="sheet-footer">
            <button type="button" className="button" onClick={onClose} disabled={busy}>
              Finish later
            </button>
            <span className="sheet-footer-end">
              {step === "folder" ? (
                <button
                  type="button"
                  className="button button-primary"
                  disabled={folder?.kind !== "config" || busy}
                  onClick={next}
                >
                  {adding ? "Adding…" : "Continue"}
                </button>
              ) : env === null ? (
                <button
                  type="button"
                  className="button button-primary"
                  disabled={checkout === null}
                  onClick={() => checkout && onDone(checkout)}
                >
                  Add project
                </button>
              ) : (
                <TypedConfirm
                  key={env}
                  expected={env}
                  verb="approve"
                  action={
                    approval?.kind === "review" && approval.busy
                      ? "Approving…"
                      : last
                        ? "Approve and add"
                        : `Approve ${env}`
                  }
                  busy={busy}
                  blocked={approval?.kind !== "review"}
                  onConfirm={approve}
                />
              )}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
