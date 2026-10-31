import { useEffect, useRef, useState } from "react";
import { quit } from "./api";
import type { QuitRun } from "./types";

const VERB: Record<QuitRun["operation"], string> = {
  deploy: "deploying",
  up: "coming out of maintenance",
  attach: "finishing a resumed run",
};

/** Asks before Quit, Cmd-Q or Dock Quit cuts a run short. */
export default function QuitDialog() {
  const [runs, setRuns] = useState<QuitRun[] | null>(null);
  const [leaving, setLeaving] = useState(false);
  const leavingRef = useRef(false);
  const keep = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let gone = false;
    let stop: (() => void) | undefined;
    const ask = () => {
      if (leavingRef.current) return;
      void quit.status().then((live) => {
        // Checked again on request: a run that already finished just quits.
        if (live.length === 0) void quit.quitNow();
        else setRuns(live);
      });
    };
    void quit.onRequest(ask).then((unlisten) => (gone ? unlisten() : (stop = unlisten)));
    return () => {
      gone = true;
      stop?.();
    };
  }, []);

  useEffect(() => {
    if (!runs || leaving) return;
    keep.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setRuns(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [runs, leaving]);

  // While waiting, keep each run's stage current.
  useEffect(() => {
    if (!leaving) return;
    const timer = window.setInterval(() => {
      void quit.status().then((live) => live.length > 0 && setRuns(live));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [leaving]);

  if (!runs) return null;

  const [first] = runs;
  const title = leaving
    ? "Finishing safely…"
    : runs.length === 1
      ? `Quit while ${first.project} → ${first.env} is ${VERB[first.operation]}?`
      : `Quit while ${runs.length} runs are going?`;
  const detach = () => {
    leavingRef.current = true;
    setLeaving(true);
    void quit.detachAndQuit();
  };

  return (
    <div className="quit-backdrop">
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="quit-title"
        className="quit-dialog"
      >
        <h2 id="quit-title" className="quit-title">
          {title}
        </h2>
        <ul className="quit-runs">
          {runs.map((run) => (
            <li key={run.handle}>
              {(runs.length > 1 || leaving) && (
                <div className="quit-run-name">
                  {run.project} → {run.env} <span className="muted">· {run.stage}</span>
                </div>
              )}
              <p>{run.detach}</p>
            </li>
          ))}
        </ul>
        <p className="muted quit-now-note">
          Quit now: {[...new Set(runs.map((run) => run.quitNow))].join(" ")}
        </p>
        <div className="quit-actions">
          <button type="button" className="toolbar-button" onClick={() => void quit.quitNow()}>
            Quit now
          </button>
          {!leaving && (
            <>
              <button type="button" className="toolbar-button" onClick={detach}>
                Detach and quit
              </button>
              <button
                type="button"
                ref={keep}
                className="toolbar-button quit-primary"
                onClick={() => setRuns(null)}
              >
                Keep running
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
