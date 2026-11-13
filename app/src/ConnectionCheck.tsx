import { useCallback, useEffect, useId, useRef, useState } from "react";
import { CommandBox } from "./CommandBox";
import { StepIcon } from "./Mark";
import { errorMessage } from "./text";
import type { CheckLine, CheckView } from "./types";

/** Starts one check, calling `onLines` as each probe moves; resolves to the end. */
export type StartCheck = (onLines: (view: CheckView) => void) => Promise<CheckView>;

export interface CheckRun {
  view: CheckView | null;
  running: boolean;
  error: string | null;
  run: (start: StartCheck) => void;
  /** Forgets the result, for a different target. */
  reset: () => void;
}

/** One check at a time; a result from a run started before `reset` is dropped. */
export function useConnectionCheck(): CheckRun {
  const [view, setView] = useState<CheckView | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const current = useRef(0);
  useEffect(() => {
    const at = current;
    return () => {
      at.current += 1;
    };
  }, []);
  const run = useCallback((start: StartCheck) => {
    const id = ++current.current;
    setRunning(true);
    setError(null);
    setView(null);
    start((lines) => id === current.current && setView(lines))
      .then((end) => id === current.current && setView(end))
      .catch((failure: unknown) => id === current.current && setError(errorMessage(failure)))
      .finally(() => id === current.current && setRunning(false));
  }, []);
  const reset = useCallback(() => {
    current.current += 1;
    setView(null);
    setRunning(false);
    setError(null);
  }, []);
  return { view, running, error, run, reset };
}

function LineIcon({ line }: { line: CheckLine }) {
  switch (line.state) {
    case "pass":
      return <StepIcon kind="ok" label="passed" />;
    case "fail":
      return <StepIcon kind="unknown" label="did not pass" />;
    case "running":
      return <StepIcon kind="running" label="running" />;
    case "notRun":
      return <StepIcon kind="notRun" label="not run" />;
    case "waiting":
      return <StepIcon kind="notRun" label="waiting" />;
  }
}

/** The read-only connection check: a button the person presses, a boxed
 * stop for a changed host key, and one ruled line per probe. */
export function ConnectionCheck({
  alias,
  check,
  start,
}: {
  /** The ssh alias, before the check names it. */
  alias: string | null;
  check: CheckRun;
  /** `null`: the check can't start yet. */
  start: StartCheck | null;
}) {
  const stopId = useId();
  const { view, running, error } = check;
  const shown = view?.alias ?? alias;
  return (
    <div className="check">
      <div className="check-head">
        <p className="muted">
          Read only. Connects
          {shown ? (
            <>
              {" "}
              as <code className="mono">{shown}</code>
            </>
          ) : (
            " with the environment's ssh alias"
          )}{" "}
          and looks; nothing on the server changes.
        </p>
        <button
          type="button"
          className="button"
          disabled={running || start === null}
          onClick={() => start && check.run(start)}
        >
          {running ? "Checking…" : view || error ? "Check again" : "Check connection"}
        </button>
      </div>
      {view?.hostKeyChanged && (
        <section className="check-stop" role="alert" aria-labelledby={stopId}>
          <h4 id={stopId} className="check-stop-title">
            Host key changed. Shipslip stops here.
          </h4>
          <p>
            The host key for <span className="mono">{view.alias}</span> is not the one ~/.ssh/known_hosts records.
            Find out why the key changed before going on.
          </p>
        </section>
      )}
      {error && (
        <p className="sheet-refusal" role="alert">
          {error}
        </p>
      )}
      {view && (
        <ul className="check-list" aria-label="Connection check" aria-busy={running}>
          {view.lines.map((line) => (
            <li key={line.label} className="check-line" data-state={line.state}>
              <span className="check-icon">
                <LineIcon line={line} />
              </span>
              <span className="check-label">{line.label}</span>
              <span className={`check-reason ${line.state === "fail" ? "tone-attention" : "muted"}`}>
                {line.reason}
              </span>
              {line.command && (
                <div className="check-command">
                  <CommandBox prefix="" command={line.command} copyText={line.command} />
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
      {view?.finished && !running && (
        <p className="visually-hidden" role="status">
          {view.lines.every((line) => line.state === "pass")
            ? "Connection check passed."
            : `Connection check done: ${view.lines.filter((line) => line.state === "fail").length} did not pass.`}
        </p>
      )}
    </div>
  );
}

/** Which of a config's environments the check reaches. */
export function EnvPicker({
  envs,
  value,
  disabled,
  onChange,
}: {
  envs: string[];
  value: string | null;
  disabled: boolean;
  onChange: (env: string) => void;
}) {
  const id = useId();
  return (
    <div className="sheet-field">
      <label htmlFor={id} className="sheet-label">
        Environment
      </label>
      <select
        id={id}
        className="sheet-input check-env"
        value={value ?? ""}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
      >
        {envs.map((name) => (
          <option key={name} value={name}>
            {name}
          </option>
        ))}
      </select>
    </div>
  );
}

/**
 * "Check connection" from the checkout menu: the same check for one of the
 * checkout's environments, in a dialog over the pane.
 */
export function CheckConnectionDialog({
  envs,
  start,
  onClose,
}: {
  envs: string[];
  start: (env: string) => StartCheck;
  onClose: () => void;
}) {
  const [env, setEnv] = useState(envs[0] ?? "");
  const check = useConnectionCheck();
  const heading = useRef<HTMLHeadingElement>(null);
  const titleId = useId();
  useEffect(() => {
    heading.current?.focus();
  }, []);
  const closeRef = useRef(onClose);
  useEffect(() => {
    closeRef.current = onClose;
  });
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      closeRef.current();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
  return (
    <div className="sheet-backdrop">
      <div role="dialog" aria-modal="true" aria-labelledby={titleId} className="sheet check-dialog">
        <div className="sheet-main">
          <div className="sheet-body">
            <h3 id={titleId} className="sheet-heading" ref={heading} tabIndex={-1}>
              Check connection
            </h3>
            {envs.length > 1 && (
              <EnvPicker
                envs={envs}
                value={env}
                disabled={check.running}
                onChange={(name) => {
                  check.reset();
                  setEnv(name);
                }}
              />
            )}
            <ConnectionCheck alias={null} check={check} start={env ? start(env) : null} />
          </div>
          <div className="sheet-footer">
            <span className="sheet-footer-end">
              <button type="button" className="button" onClick={onClose}>
                Close
              </button>
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
