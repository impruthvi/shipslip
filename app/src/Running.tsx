import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Announce } from "./Announcer";
import { deploy } from "./deployApi";
import { Mark, StepIcon } from "./Mark";
import { ConnectionGap, ReattachedNote } from "./ConnectionGap";
import { StepTime } from "./StepTime";
import { applyBatch, headlineLine, progressLine, runRows, watching, type RunLog } from "./run";
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
  onLeave,
}: {
  info: RunInfo;
  announce: Announce;
  onEnd: (terminal: Terminal) => void;
  /** Back to the map; the run keeps going on the server and here. */
  onLeave: () => void;
}) {
  const [log, setLog] = useState<RunLog | null>(null);
  const [lost, setLost] = useState<CommandError | null>(null);
  const [stopping, setStopping] = useState<"afterStep" | "watch" | null>(null);
  const [stopError, setStopError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [watchStart, setWatchStart] = useState<number | null>(null);
  /** What the effect below has already announced. */
  const seen = useRef({ started: new Set<string>(), ended: new Set<string>(), watchStart });
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
  const headline = state?.headline ?? null;
  const gap = state?.connectionGap ?? null;
  const gapLostAt = gap?.lostAtMs ?? null;
  const headlineText = headline ? headlineLine(headline) : null;
  const failedStep = headline?.kind === "failed" ? String(headline.step) : null;

  useEffect(() => {
    if (failedStep !== null) setOpen((prev) => ({ ...prev, [failedStep]: true }));
  }, [failedStep]);

  useEffect(() => {
    if (headlineText) props.current.announce(headlineText, true);
  }, [headlineText]);

  useEffect(() => {
    if (gapLostAt !== null) {
      setNow(Date.now());
      props.current.announce("Connection lost. Shipslip will reconnect to watch. It will not run any step again.", true);
    }
  }, [gapLostAt]);

  // Announcements as each step starts and ends.
  useEffect(() => {
    if (!state) return;
    const { started, ended } = seen.current;
    for (const row of rows) {
      const status = row.progress?.status;
      const timing = row.progress?.timing;
      const numbered = /^\d+$/.test(row.index) && row.index !== "0";
      if (status === "Running" && !started.has(row.index)) {
        started.add(row.index);
        props.current.announce(
          numbered
            ? `Step ${row.index} of ${info.steps.length} started: ${row.command}`
            : `${row.command} started`,
        );
      } else if (
        status &&
        status !== "Running" &&
        status !== "Pending" &&
        started.has(row.index) &&
        !ended.has(row.index)
      ) {
        ended.add(row.index);
        if (status === "Ok") {
          props.current.announce(
            `${numbered ? `Step ${row.index}` : row.command} done${
              timing?.kind === "took" ? ` in ${spoken(timing.durationMs)}` : ""
            }`,
          );
        }
      }
    }
    if (watching(state) && seen.current.watchStart === null) {
      const at = Date.now();
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
        {state && !terminal && (
          headline ? (
            <div className="run-outcome">
              <h2 className={`run-progress${headline.kind === "failed" ? " tone-bad" : headline.kind === "stopped" ? " tone-attention" : ""}`}>
                {headlineText}
              </h2>
              {watchingNow && !gap && watchLeft !== null && (
                <p className="muted">Watching the log · {countdown(watchLeft)} left</p>
              )}
            </div>
          ) : !gap && (
            <p className="run-progress">
              {[progressLine(info, state, rows), elapsed(Math.max(now, info.startedAtMs) - info.startedAtMs)]
                .filter(Boolean)
                .join(" · ")}
            </p>
          )
        )}
        {gap && !terminal && (
          <div className="run-outcome">
            <p className="run-progress"><ConnectionGap gap={gap} now={now} /></p>
            <p className="muted">
              {gap.target.kind === "logWatch"
                ? "Shipslip will reconnect to watch the log. It will not run any step again."
                : "Shipslip will reconnect to watch the step and will not run it again."}
            </p>
          </div>
        )}
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
              const inGap = gap && gap.target.kind !== "logWatch" && row.owns(gap.target);
              const lines = log?.output.filter((line) => row.owns(line.source)) ?? [];
              const expanded = lines.length > 0 && (open[row.index] ?? status === "Running");
              return (
                <li key={row.index}>
                  <button
                    type="button"
                    className="run-step-head"
                    aria-expanded={lines.length > 0 ? expanded : undefined}
                    onClick={() => setOpen((prev) => ({ ...prev, [row.index]: !expanded }))}
                  >
                    <span className="run-step-mark">
                      {inGap ? <Mark kind="notKnown" size={12} /> : <StepMark status={status} />}
                    </span>
                    <span className="recipe-index mono muted">{row.index}</span>
                    <code>{row.command}</code>
                    {row.progress?.timing && (
                      <StepTime timing={row.progress.timing} className="elapsed muted" />
                    )}
                  </button>
                  {row.progress?.reattachedAtMs != null && (
                    <p className="run-step-note"><ReattachedNote at={row.progress.reattachedAtMs} /></p>
                  )}
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
            {watchingNow && !headline && !gap && watchLeft !== null && <p>{countdown(watchLeft)} left</p>}
            {state.watchReattachedAtMs !== null && <p><ReattachedNote at={state.watchReattachedAtMs} /></p>}
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
          info.operation !== "up" && !headline && (
            <button
              type="button"
              className="button"
              disabled={stopping !== null || !state || terminal !== null}
              onClick={() => stop("afterStep")}
            >
              {stopping === "afterStep" ? "Stopping after this step…" : "Stop after this step"}
            </button>
          )
        )}
        {stopping === "watch" && watchingNow && <span className="muted">Stopping the log watch…</span>}
        {stopError && <span className="tone-bad">{stopError}</span>}
        <div className="footer-end">
          <button type="button" className="button" onClick={onLeave}>
            Leave running
          </button>
        </div>
      </footer>
    </>
  );
}

