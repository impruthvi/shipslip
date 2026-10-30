import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Announce } from "./Announcer";
import { deploy } from "./deployApi";
import { StepIcon } from "./Mark";
import { applyBatch, runRows, watching, type RunLog } from "./run";
import { countdown, elapsed, spoken } from "./time";
import type { CommandError, OutputLine, ProgressStatus, RunInfo, Terminal } from "./types";

const TICK_MS = 1000;

function StepMark({ status }: { status: ProgressStatus | undefined }) {
  switch (status) {
    case "Running":
      return <StepIcon kind="running" label="running now" />;
    case "Ok":
      return <StepIcon kind="ok" />;
    case "Failed":
      return <StepIcon kind="failed" />;
    case "Unknown":
      return <StepIcon kind="unknown" />;
    case "Skipped":
      return <StepIcon kind="skipped" />;
    default:
      return <StepIcon kind="notRun" label="not started" />;
  }
}

/** A step's output: follows new lines unless scrolled up. Untrusted text, shown as text. */
function Output({ label, lines }: { label: string; lines: OutputLine[] }) {
  const box = useRef<HTMLPreElement>(null);
  const [pinned, setPinned] = useState(true);
  useLayoutEffect(() => {
    if (pinned && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [lines, pinned]);
  const onScroll = () => {
    const el = box.current;
    if (el) setPinned(el.scrollHeight - el.scrollTop - el.clientHeight < 4);
  };
  return (
    <div className="run-output-box">
      <pre
        ref={box}
        className="run-output mono"
        role="region"
        aria-label={label}
        tabIndex={0}
        onScroll={onScroll}
      >
        {lines.map((line) => line.text).join("\n")}
      </pre>
      {!pinned && (
        <button type="button" className="toolbar-button jump-latest" onClick={() => setPinned(true)}>
          Jump to latest
        </button>
      )}
    </div>
  );
}

/** A run in progress: its steps as they go, then the log watch. */
export function Running({
  info,
  announce,
  onEnd,
}: {
  info: RunInfo;
  announce: Announce;
  onEnd: (terminal: Terminal) => void;
}) {
  const [log, setLog] = useState<RunLog | null>(null);
  const [lost, setLost] = useState<CommandError | null>(null);
  const [stopping, setStopping] = useState<"afterStep" | "watch" | null>(null);
  const [stopError, setStopError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [times, setTimes] = useState<Record<string, { start: number; end?: number }>>({});
  const [watchStart, setWatchStart] = useState<number | null>(null);
  /** What the effect below has already timed and announced. */
  const seen = useRef({ times, watchStart });
  const ended = useRef(false);
  const props = useRef({ announce, onEnd });
  useEffect(() => {
    props.current = { announce, onEnd };
  });

  useEffect(() => {
    let gone = false;
    let stop: (() => void) | undefined;
    deploy
      .follow(info.handle, (batch) => {
        if (!gone) setLog((prev) => applyBatch(prev, batch));
      })
      .then(
        (unsubscribe) => (gone ? unsubscribe() : (stop = unsubscribe)),
        (error: CommandError) => !gone && setLost(error),
      );
    return () => {
      gone = true;
      stop?.();
    };
  }, [info.handle]);

  const state = log?.state ?? null;
  const rows = useMemo(() => (state ? runRows(info, state) : []), [info, state]);
  const terminal = state?.terminal ?? null;

  // Times and announcements as each step starts and ends.
  useEffect(() => {
    if (!state) return;
    const at = Date.now();
    const next = { ...seen.current.times };
    let changed = false;
    for (const row of rows) {
      const status = row.progress?.status;
      const time = next[row.index];
      const numbered = /^\d+$/.test(row.index) && row.index !== "0";
      if (status === "Running" && !time) {
        next[row.index] = { start: at };
        changed = true;
        props.current.announce(
          numbered
            ? `Step ${row.index} of ${info.steps.length} started: ${row.command}`
            : `${row.command} started`,
        );
      } else if (status && status !== "Running" && status !== "Pending" && time && !time.end) {
        next[row.index] = { ...time, end: at };
        changed = true;
        if (status === "Ok") {
          props.current.announce(
            `${numbered ? `Step ${row.index}` : row.command} done in ${spoken(at - time.start)}`,
          );
        }
      }
    }
    if (changed) {
      seen.current.times = next;
      setTimes(next);
    }
    if (watching(state) && seen.current.watchStart === null) {
      seen.current.watchStart = at;
      setWatchStart(at);
      props.current.announce(`Log watch started, ${spoken(state.watchWindowMs ?? 0)}`);
    }
  }, [state, rows, info.steps.length]);

  useEffect(() => {
    if (ended.current) return;
    if (terminal) {
      ended.current = true;
      props.current.onEnd(terminal);
    } else if (lost) {
      ended.current = true;
      props.current.onEnd({ kind: "error", reason: lost.message });
    }
  }, [terminal, lost]);

  useEffect(() => {
    if (terminal) return;
    const timer = window.setInterval(() => setNow(Date.now()), TICK_MS);
    return () => window.clearInterval(timer);
  }, [terminal]);

  const watchingNow = state !== null && watching(state);
  const stop = (what: "afterStep" | "watch") => {
    setStopping(what);
    setStopError(null);
    deploy.stop(info.handle, what).catch((error: CommandError) => {
      setStopping(null);
      setStopError(error.message);
    });
  };
  const logLines = log?.output.filter((line) => line.source.kind === "log") ?? [];
  const watchLeft =
    state?.watchWindowMs != null && watchStart !== null
      ? state.watchWindowMs - (Math.max(now, watchStart) - watchStart)
      : null;

  return (
    <>
      <div className="workspace-body">
        <section className="workspace-section" aria-labelledby="run-steps">
          <h2 id="run-steps" className="caps">
            Steps
          </h2>
          {!state && <p className="muted">Connecting to {info.sshAlias}…</p>}
          {log && log.dropped > 0 && (
            <p className="muted">Earlier output not kept: {log.dropped} lines</p>
          )}
          <ol className="run-steps">
            {rows.map((row) => {
              const status = row.progress?.status;
              const lines = log?.output.filter((line) => row.owns(line.source)) ?? [];
              const expanded = lines.length > 0 && (open[row.index] ?? status === "Running");
              const time = times[row.index];
              return (
                <li key={row.index}>
                  <button
                    type="button"
                    className="run-step-head"
                    aria-expanded={lines.length > 0 ? expanded : undefined}
                    onClick={() => setOpen((prev) => ({ ...prev, [row.index]: !expanded }))}
                  >
                    <span className="run-step-mark">
                      <StepMark status={status} />
                    </span>
                    <span className="recipe-index mono muted">{row.index}</span>
                    <code>{row.command}</code>
                    {time && (
                      <span className="elapsed muted">{elapsed((time.end ?? Math.max(now, time.start)) - time.start)}</span>
                    )}
                  </button>
                  {expanded && <Output label={`Output of step ${row.index}`} lines={lines} />}
                </li>
              );
            })}
          </ol>
        </section>
        {state?.watchWindowMs != null && (
          <section className="workspace-section" aria-labelledby="run-watch">
            <h2 id="run-watch" className="caps">
              Log watch
            </h2>
            {watchingNow && watchLeft !== null && <p>{countdown(watchLeft)} left</p>}
            {logLines.length === 0 ? (
              <p className="muted">No new errors so far</p>
            ) : (
              <ul className="log-errors">
                {logLines.map((line) => (
                  <li key={line.sequence}>
                    <span className="chip">NEW</span> <code>{line.text}</code>
                    {line.source.kind === "log" && line.source.fileLine && (
                      <span className="mono muted"> {line.source.fileLine}</span>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </section>
        )}
      </div>
      <footer className="workspace-footer">
        {watchingNow ? (
          <button
            type="button"
            className="button"
            disabled={stopping === "watch"}
            onClick={() => stop("watch")}
          >
            Stop watching
          </button>
        ) : (
          // Bringing the app up is one step: there is no step after it to stop before.
          info.operation !== "up" && (
            <button
              type="button"
              className="button"
              disabled={stopping !== null || !state || terminal !== null}
              onClick={() => stop("afterStep")}
            >
              Stop after this step
            </button>
          )
        )}
        {stopping && (
          <span className="muted">
            {stopping === "watch" && watchingNow ? "Stopping the log watch…" : "Stopping after this step…"}
          </span>
        )}
        {stopError && <span className="tone-bad">{stopError}</span>}
      </footer>
    </>
  );
}

