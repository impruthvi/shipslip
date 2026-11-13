import { useCallback, useEffect, useId, useRef, useState } from "react";
import { CommandBox } from "./CommandBox";
import { StepIcon } from "./Mark";
import { errorMessage, hostKeyUnverifiedBody, withCode } from "./text";
import type { CheckLine, CheckView } from "./types";

/** Starts one check, calling `onLines` as each probe moves; resolves to the end. */
export type StartCheck = (onLines: (view: CheckView) => void) => Promise<CheckView>;

/** An environment whose check stopped at the host key. */
export interface EnvHostKeyStop {
  env: string;
  view: CheckView;
}

export interface CheckRun {
  view: CheckView | null;
  running: boolean;
  error: string | null;
  /** Per environment: the last finished check, or `null` for one started
   * that did not finish. An environment never checked is absent. */
  finished: Record<string, CheckView | null>;
  /** Per environment, its shown or last finished check that stopped at the
   * host key: no approval while any. */
  hostKeyStops: EnvHostKeyStop[];
  run: (env: string, start: StartCheck) => void;
  /** Clears the shown check, for a different environment; `finished` stays. */
  reset: () => void;
  /** Clears the shown check and every environment's, for a different target. */
  forgetAll: () => void;
}

/** One check at a time; a result from a run started before `reset` or
 * `forgetAll` is dropped. */
export function useConnectionCheck(): CheckRun {
  const [view, setView] = useState<CheckView | null>(null);
  const [shownEnv, setShownEnv] = useState<string | null>(null);
  const [finished, setFinished] = useState<Record<string, CheckView | null>>({});
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const current = useRef(0);
  useEffect(() => {
    const at = current;
    return () => {
      at.current += 1;
    };
  }, []);
  const run = useCallback((env: string, start: StartCheck) => {
    const id = ++current.current;
    setRunning(true);
    setError(null);
    setView(null);
    setShownEnv(env);
    setFinished((results) => ({ ...results, [env]: null }));
    start((lines) => id === current.current && setView(lines))
      .then((end) => {
        if (id !== current.current) return;
        setView(end);
        if (end.finished) setFinished((results) => ({ ...results, [env]: end }));
      })
      .catch((failure: unknown) => id === current.current && setError(errorMessage(failure)))
      .finally(() => id === current.current && setRunning(false));
  }, []);
  const reset = useCallback(() => {
    current.current += 1;
    setView(null);
    setShownEnv(null);
    setRunning(false);
    setError(null);
  }, []);
  const forgetAll = useCallback(() => {
    reset();
    setFinished({});
  }, [reset]);
  const hostKeyStops = Object.keys(finished).flatMap((env) => {
    const shown = env === shownEnv ? view : finished[env];
    return shown?.hostKeyStop ? [{ env, view: shown }] : [];
  });
  return { view, running, error, finished, hostKeyStops, run, reset, forgetAll };
}

function LineIcon({ line }: { line: CheckLine }) {
  switch (line.state) {
    case "pass":
      return <StepIcon kind="ok" label="passed" />;
    case "fail":
      return <StepIcon kind="attention" />;
    case "running":
      return <StepIcon kind="running" label="running" />;
    case "notRun":
      return <StepIcon kind="notRun" label="not run" />;
    case "waiting":
      return <StepIcon kind="notRun" label="waiting" />;
  }
}

function HostKeyStopBox({ stop: { env, view } }: { stop: EnvHostKeyStop }) {
  const titleId = useId();
  return (
    <section className="check-stop" role="alert" aria-labelledby={titleId}>
      <h4 id={titleId} className="check-stop-title">
        {view.hostKeyStop === "changed"
          ? `Host key changed for ${env}. Shipslip stops here.`
          : `Shipslip can't verify the host key for ${env}`}
      </h4>
      {view.hostKeyStop === "changed" ? (
        <p>
          The host key for <span className="mono">{view.alias}</span> is not the one ~/.ssh/known_hosts records. Find out why the key changed before
          going on.
        </p>
      ) : (
        <p>{withCode(hostKeyUnverifiedBody(view.alias))}</p>
      )}
    </section>
  );
}

/** The read-only connection check: a button the person presses, a boxed
 * stop for each environment stopped at the host key, and one ruled line per probe. */
export function ConnectionCheck({
  env,
  alias,
  check,
  start,
}: {
  /** The environment checked; its result is kept under this name. */
  env: string;
  /** The ssh alias, before the check names it. */
  alias: string | null;
  check: CheckRun;
  /** `null`: the check can't start yet. */
  start: StartCheck | null;
}) {
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
          and looks. Shipslip's commands only read; starting <code className="mono">php artisan</code> can let Laravel refresh its own cache
          files.
        </p>
        <button
          type="button"
          className="button"
          disabled={running || start === null}
          onClick={() => start && check.run(env, start)}
        >
          {running ? "Checking…" : view || error ? "Check again" : "Check connection"}
        </button>
      </div>
      {check.hostKeyStops.map((stop) => (
        <HostKeyStopBox key={stop.env} stop={stop} />
      ))}
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

/** The check beside the approval: its lines, or that it was not run or did
 * not finish. */
export function CheckSummary({ env, check }: { env: string; check: CheckRun }) {
  const titleId = useId();
  const view = check.finished[env];
  return (
    <section className="check-summary" aria-labelledby={titleId}>
      <h4 id={titleId} className="caps muted check-summary-title">
        Connection check
      </h4>
      {view ? (
        <ul className="check-summary-list">
          {view.lines.map((line) => (
            <li key={line.label} className="check-summary-line" data-state={line.state}>
              <span className="check-icon">
                <LineIcon line={line} />
              </span>
              <span className="check-summary-label">{line.label}</span>
              {line.state !== "pass" && (
                <span className={`check-summary-reason ${line.state === "fail" ? "tone-attention" : "muted"}`}>
                  {line.reason}
                </span>
              )}
            </li>
          ))}
        </ul>
      ) : view === null ? (
        <p className="muted">
          Did not finish. Check the connection for <span className="mono">{env}</span> again from the check step.
        </p>
      ) : (
        <p className="muted">
          Not run. Shipslip did not check the connection for <span className="mono">{env}</span>.
        </p>
      )}
    </section>
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
            <ConnectionCheck env={env} alias={null} check={check} start={env ? start(env) : null} />
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
