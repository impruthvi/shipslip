import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { api } from "./api";
import { deploy } from "./deployApi";
import { StepIcon } from "./Mark";
import { errorMessage } from "./text";
import { CheckSummary, ConnectionCheck, EnvPicker, useConnectionCheck } from "./ConnectionCheck";
import { chosenAlias, freshChoice, SshAliasStep, type SshChoice } from "./SshAliasStep";
import { TrustSettings } from "./TrustReview";
import { TypedConfirm } from "./TypedConfirm";
import type { AnswerField, Answers, FolderEnv, FolderView, Loaded, SshAliases, TrustReview } from "./types";

/** An existing config goes folder → check → approval; a new one folder →
 * environment → ssh alias → check → approval. */
type Step = "folder" | "environment" | "ssh" | "check" | "approval";
const label: Record<Step, string> = {
  folder: "Folder",
  environment: "Environment",
  ssh: "SSH alias",
  check: "Connection check",
  approval: "Approval",
};
const stepsFor = (folder: FolderView | null): Step[] =>
  folder?.kind === "newConfig"
    ? ["folder", "environment", "ssh", "check", "approval"]
    : ["folder", "check", "approval"];

const freshAnswers = (branch: string | null): Answers => ({
  env: "staging",
  sshAlias: "",
  path: "",
  branch: branch ?? "",
  production: false,
  maintenance: true,
  smokeUrl: "",
});

const message = errorMessage;

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
    case "newConfig":
      return (
        <p className="muted">
          This checkout has no .shipslip.toml yet. Shipslip asks the <code className="mono">slip init</code>{" "}
          questions for one environment and shows the file before writing it.
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

function Field({
  label: text,
  hint,
  problem,
  children,
}: {
  label: string;
  hint?: string;
  problem?: string;
  children: (props: { id: string; "aria-describedby"?: string; "aria-invalid"?: boolean }) => ReactNode;
}) {
  const id = useId();
  const noteId = `${id}-note`;
  return (
    <div className="sheet-field">
      <label htmlFor={id} className="sheet-label">
        {text}
      </label>
      {children({
        id,
        "aria-describedby": problem || hint ? noteId : undefined,
        "aria-invalid": problem ? true : undefined,
      })}
      {problem ? (
        <span id={noteId} className="sheet-hint tone-attention">
          {problem}
        </span>
      ) : (
        hint && (
          <span id={noteId} className="sheet-hint muted">
            {hint}
          </span>
        )
      )}
    </div>
  );
}

/** The seven `slip init` questions for one environment. */
function AnswersForm({
  answers,
  problems,
  disabled,
  onChange,
}: {
  answers: Answers;
  problems: Partial<Record<AnswerField, string>>;
  disabled: boolean;
  onChange: (change: Partial<Answers>) => void;
}) {
  const text =
    (field: AnswerField & keyof Answers, mono: boolean, placeholder?: string) =>
    // Called by Field as a function, so the input is not remounted per keystroke.
    (props: { id: string }) => (
        <input
          {...props}
          className={`sheet-input${mono ? " mono" : ""}`}
          name={field}
          value={answers[field]}
          placeholder={placeholder}
          onChange={(event) => onChange({ [field]: event.target.value })}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          disabled={disabled}
        />
    );
  return (
    <div className="sheet-form">
      <Field label="Environment name" hint="Letters, digits, - and _" problem={problems.env}>
        {text("env", false)}
      </Field>
      <Field
        label="Remote path"
        hint="Absolute path of the app's git repository on the server"
        problem={problems.path}
      >
        {text("path", true, "/var/www/app")}
      </Field>
      <Field label="Branch" hint="The branch deploys check out" problem={problems.branch}>
        {text("branch", true)}
      </Field>
      <label className="sheet-check">
        <input
          type="checkbox"
          checked={answers.production}
          onChange={(event) => onChange({ production: event.target.checked })}
          disabled={disabled}
        />
        <span>
          Production <span className="muted">· deploys ask you to type the environment name</span>
        </span>
      </label>
      <label className="sheet-check">
        <input
          type="checkbox"
          checked={answers.maintenance}
          onChange={(event) => onChange({ maintenance: event.target.checked })}
          disabled={disabled}
        />
        <span>
          Maintenance mode <span className="muted">· php artisan down before the steps, up once they succeed</span>
        </span>
      </label>
      <Field
        label="Smoke URL"
        hint="Optional. Requested from this Mac after the steps; 2xx passes"
        problem={problems.smokeUrl}
      >
        {text("smokeUrl", true, "https://example.com/health")}
      </Field>
    </div>
  );
}

/** Frontend setup the recipe leaves out: read and ticked before approving. */
function Warnings({
  warnings,
  read,
  disabled,
  onRead,
}: {
  warnings: string[];
  read: boolean;
  disabled: boolean;
  onRead: (read: boolean) => void;
}) {
  const titleId = useId();
  return (
    <section className="sheet-warnings" aria-labelledby={titleId}>
      <h4 id={titleId} className="sheet-warnings-title">
        Read before approving
      </h4>
      {warnings.map((warning) => (
        <p key={warning}>{warning}</p>
      ))}
      <label className="sheet-check">
        <input type="checkbox" checked={read} onChange={(event) => onRead(event.target.checked)} disabled={disabled} />
        <span>I read this. The recipe has no frontend steps until I add them.</span>
      </label>
    </section>
  );
}

/** One environment's approval: today's settings review, typed. */
type Approval =
  | { kind: "loading"; env: string }
  | { kind: "review"; review: TrustReview; busy: boolean; error: string | null }
  | { kind: "failed"; env: string; error: string };

/**
 * The Add project sheet: pick a folder, then approve its `.shipslip.toml`,
 * or answer the `slip init` questions and approve the config they render.
 * The checkout is added when the folder step is left, so "Finish later" after
 * it leaves the checkout listed as "Needs approval" or "Setup not finished".
 */
export function AddProject({
  loaded,
  reopen = null,
  onAdded,
  onClose,
  onDone,
}: {
  /** The map now shown; the approval reads the new checkout's row from it. */
  loaded: Loaded | null;
  /** A "Setup not finished" map row: opens at the environment step. */
  reopen?: { generation: number; row: number } | null;
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
  /** A new config: the answers, what Rust said about them, and the text. */
  const [answers, setAnswers] = useState<Answers>(() => freshAnswers(null));
  const [problems, setProblems] = useState<Partial<Record<AnswerField, string>>>({});
  /** The first answer to fix takes focus once the fields are enabled again. */
  const [focusField, setFocusField] = useState<AnswerField | null>(null);
  const [rendering, setRendering] = useState(false);
  const [config, setConfig] = useState<{ lines: string[]; warnings: string[] } | null>(null);
  const [warningsRead, setWarningsRead] = useState(false);
  const [writing, setWriting] = useState(false);
  /** Written, but approving failed: the checkout reads "Needs approval". */
  const [unapproved, setUnapproved] = useState<string | null>(null);
  /** The ssh alias step: `~/.ssh/config` read once per sheet, and the pick. */
  const [aliases, setAliases] = useState<SshAliases | null>(null);
  const [sshChoice, setSshChoice] = useState<SshChoice>(() => freshChoice(""));
  /** The read-only connection check, and for an existing config the
   * environment it checks. */
  const check = useConnectionCheck();
  const [checkEnv, setCheckEnv] = useState<string | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const form = useRef<HTMLDivElement>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);



  const picked = (view: FolderView) => {
    setFolder(view);
    setAnswers(freshAnswers(view.kind === "newConfig" ? view.branch : null));
    setProblems({});
    setConfig(null);
    setSshChoice(freshChoice(view.kind === "newConfig" ? view.project : ""));
  };

  const choose = () => {
    setPicking(true);
    setAddError(null);
    api
      .pickFolder()
      .then((view) => {
        if (!alive.current) return;
        // Closing the panel keeps what was picked before.
        if (view.kind !== "cancelled") picked(view);
      })
      .catch((error: unknown) => alive.current && setAddError(message(error)))
      .finally(() => alive.current && setPicking(false));
  };

  // A "Setup not finished" checkout: its folder again, at the environment step.
  const reopenRef = useRef(reopen);
  useEffect(() => {
    const from = reopenRef.current;
    if (!from) return;
    setPicking(true);
    api
      .reopenSetup(from.generation, from.row)
      .then((view) => {
        if (!alive.current) return;
        picked(view);
        if (view.kind === "newConfig") {
          setCheckout(view.checkout);
          setStep("environment");
        }
      })
      .catch((error: unknown) => alive.current && setAddError(message(error)))
      .finally(() => alive.current && setPicking(false));
  }, []);

  const next = () => {
    if (folder?.kind !== "config" && folder?.kind !== "newConfig") return;
    setAdding(true);
    setAddError(null);
    api
      .addCheckout()
      .then((added) => {
        if (!alive.current) return;
        setCheckout(added);
        if (folder.kind === "config") {
          const unapproved = folder.envs.filter((env) => !env.approved).map((env) => env.name);
          setPending(unapproved);
          setApproval(null);
          setCheckEnv(unapproved[0] ?? folder.envs[0]?.name ?? null);
          check.forgetAll();
          setStep("check");
        } else {
          setStep("environment");
        }
        onAdded();
      })
      .catch((error: unknown) => alive.current && setAddError(message(error)))
      .finally(() => alive.current && setAdding(false));
  };

  useEffect(() => {
    if (focusField === null || rendering) return;
    form.current?.querySelector<HTMLInputElement>(`input[name="${focusField}"]`)?.focus();
    setFocusField(null);
  }, [focusField, rendering]);

  const change = (update: Partial<Answers>) => {
    setAnswers((current) => ({ ...current, ...update }));
    setProblems((current) => {
      const rest = { ...current };
      for (const field of Object.keys(update)) delete rest[field as AnswerField];
      return rest;
    });
  };

  useEffect(() => {
    if (step !== "ssh" || aliases !== null) return;
    let current = true;
    api
      .sshAliases()
      .then((read) => {
        if (!current) return;
        setAliases(read);
        setSshChoice((choice) =>
          choice.draft.identityFile === "" && read.identityFiles.length > 0
            ? { ...choice, draft: { ...choice.draft, identityFile: read.identityFiles[0] } }
            : choice,
        );
      })
      .catch((error: unknown) => current && setAliases({ aliases: [], identityFiles: [], error: message(error) }));
    return () => {
      current = false;
    };
  }, [step, aliases]);

  /** Renders the config. From the environment step only its own answers
   * count: the alias is the next step's. */
  const render = (from: "environment" | "ssh") => {
    const asked = { ...answers, sshAlias: from === "ssh" ? (chosenAlias(sshChoice) ?? "") : answers.sshAlias };
    setAnswers(asked);
    setRendering(true);
    setAddError(null);
    api
      .renderConfig(asked)
      .then((text) => {
        if (!alive.current) return;
        if (text.kind === "invalid") {
          const own = text.problems.filter((p) => p.field !== "sshAlias");
          if (own.length > 0) {
            setProblems(Object.fromEntries(own.map((p) => [p.field, p.message])));
            setFocusField(own[0].field);
            setStep("environment");
          } else if (from === "environment") {
            setStep("ssh");
          } else {
            setAddError(text.problems.map((p) => p.message).join(" "));
          }
          return;
        }
        if (from === "environment") {
          setStep("ssh");
          return;
        }
        setConfig({ lines: text.lines, warnings: text.warnings });
        setWarningsRead(false);
        check.forgetAll();
        setStep("check");
      })
      .catch((error: unknown) => alive.current && setAddError(message(error)))
      .finally(() => alive.current && setRendering(false));
  };

  const write = () => {
    setWriting(true);
    setAddError(null);
    api
      .addWithConfig()
      .then((added) => {
        if (!alive.current) return;
        if (added.approvalError === null) {
          onDone(added.checkout);
          return;
        }
        setCheckout(added.checkout);
        setUnapproved(added.approvalError);
        onAdded();
      })
      .catch((error: unknown) => alive.current && setAddError(message(error)))
      .finally(() => alive.current && setWriting(false));
  };

  const newConfig = folder?.kind === "newConfig";
  const row = checkout && loaded ? loaded.map.rows.findIndex((r) => r.repoRoot === checkout) : -1;
  const env = pending[0] ?? null;
  // Each step, and each environment approved in turn, focuses its heading.
  useEffect(() => {
    heading.current?.focus();
  }, [step, env]);
  const onMap = row >= 0 && loaded !== null && env !== null && loaded.map.envs.includes(env);
  const checkOnMap = row >= 0 && loaded !== null && checkEnv !== null && loaded.map.envs.includes(checkEnv);
  const checkAlias =
    folder?.kind === "config" ? folder.envs.find((e) => e.name === checkEnv)?.sshAlias : undefined;
  const newEnv = answers.env.trim();
  const checkedEnv = newConfig ? newEnv : checkEnv;

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

  const steps = stepsFor(folder);
  const done = (s: Step) => steps.indexOf(s) < steps.indexOf(step);
  const path = folder && folder.kind !== "cancelled" ? ("checkout" in folder ? folder.checkout : folder.folder) : null;
  const busy = picking || adding || rendering || writing || (approval?.kind === "review" && approval.busy);
  // Once the file is written, the answers can't change it.
  const locked = busy || unapproved !== null;
  const blockedByWarnings = config !== null && config.warnings.length > 0 && !warningsRead;
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
            {steps.map((s, index) => {
              const isDone = done(s);
              const content = (
                <>
                  <span className="sheet-step-icon">
                    {isDone ? <StepIcon kind="ok" label="done" /> : <span className="muted">{index + 1}</span>}
                  </span>
                  {label[s]}
                </>
              );
              return (
                <li key={s}>
                  {isDone && !locked ? (
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
            ) : step === "environment" ? (
              <>
                <h3 className="sheet-heading" ref={heading} tabIndex={-1}>
                  Describe one environment
                </h3>
                <p className="muted">
                  The same questions as <code className="mono">slip init</code>. Add more environments later in
                  .shipslip.toml.
                </p>
                <div ref={form}>
                  <AnswersForm answers={answers} problems={problems} disabled={busy} onChange={change} />
                </div>
              </>
            ) : step === "ssh" ? (
              <>
                <h3 className="sheet-heading" ref={heading} tabIndex={-1}>
                  Pick the ssh alias
                </h3>
                <SshAliasStep aliases={aliases} choice={sshChoice} disabled={busy} onChange={setSshChoice} />
              </>
            ) : step === "check" ? (
              <>
                <h3 className="sheet-heading" ref={heading} tabIndex={-1}>
                  Check the connection
                </h3>
                {folder?.kind === "config" && folder.envs.length > 1 && (
                  <EnvPicker
                    envs={folder.envs.map((e) => e.name)}
                    value={checkEnv}
                    disabled={check.running}
                    onChange={(name) => {
                      check.reset();
                      setCheckEnv(name);
                    }}
                  />
                )}
                {!newConfig && !checkOnMap && (
                  <p className="muted" role="status">
                    Waiting for the map to list this checkout…
                  </p>
                )}
                <ConnectionCheck
                  env={checkedEnv ?? ""}
                  alias={newConfig ? answers.sshAlias : (checkAlias ?? null)}
                  check={check}
                  start={
                    newConfig
                      ? (onLines) => api.checkRendered(onLines)
                      : checkOnMap && loaded && checkEnv
                        ? (onLines) => api.checkConnection(loaded.generation, row, checkEnv, onLines)
                        : null
                  }
                />
              </>
            ) : newConfig ? (
              <>
                <h3 className="sheet-heading" ref={heading} tabIndex={-1}>
                  Approve {newEnv}
                </h3>
                <p className="muted">
                  Shipslip writes this .shipslip.toml to the checkout, then approves it. Deploys use these settings
                  only once they are approved.
                </p>
                {config && config.warnings.length > 0 && (
                  <Warnings
                    warnings={config.warnings}
                    read={warningsRead}
                    disabled={locked}
                    onRead={setWarningsRead}
                  />
                )}
                {config && (
                  <div className="sheet-approval">
                    <pre className="sheet-config mono" aria-label=".shipslip.toml">
                      {config.lines.join("\n")}
                    </pre>
                    <CheckSummary env={newEnv} check={check} />
                  </div>
                )}
                {unapproved !== null && (
                  <div className="sheet-refusal" role="alert">
                    <p>
                      Shipslip wrote .shipslip.toml but could not approve it. The checkout reads “Needs approval”
                      until it is approved.
                    </p>
                    <pre className="sheet-detail mono">{unapproved}</pre>
                  </div>
                )}
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
                  <div className="sheet-approval">
                    <TrustSettings
                      review={approval.review}
                      headingId="sheet-settings"
                      onEdit={() =>
                        loaded && row >= 0 && env ? deploy.openConfig(loaded.generation, row, env) : Promise.resolve()
                      }
                    />
                    <CheckSummary env={approval.review.env} check={check} />
                  </div>
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
                  disabled={(folder?.kind !== "config" && !newConfig) || busy}
                  onClick={next}
                >
                  {adding ? "Adding…" : "Continue"}
                </button>
              ) : step === "environment" ? (
                <button
                  type="button"
                  className="button button-primary"
                  disabled={busy}
                  onClick={() => render("environment")}
                >
                  {rendering ? "Checking…" : "Continue"}
                </button>
              ) : step === "ssh" ? (
                <button
                  type="button"
                  className="button button-primary"
                  disabled={busy || chosenAlias(sshChoice) === null}
                  onClick={() => render("ssh")}
                >
                  {rendering ? "Checking…" : "Continue"}
                </button>
              ) : step === "check" ? (
                <button
                  type="button"
                  className="button button-primary"
                  disabled={busy || check.running || check.hostKeyStopped}
                  onClick={() => setStep("approval")}
                >
                  Continue
                </button>
              ) : newConfig && unapproved !== null ? (
                <button
                  type="button"
                  className="button button-primary"
                  disabled={checkout === null}
                  onClick={() => checkout && onDone(checkout)}
                >
                  Open checkout
                </button>
              ) : newConfig ? (
                <TypedConfirm
                  expected={newEnv}
                  verb="approve"
                  action={writing ? "Approving…" : "Approve and add"}
                  busy={busy}
                  blocked={config === null || blockedByWarnings}
                  onConfirm={write}
                />
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
