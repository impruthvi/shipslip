import type { Ref } from "react";
import { Flap, status } from "./Cell";
import { Footer } from "./Footer";
import { Mark } from "./Mark";
import { ProblemsStrip } from "./ProblemsStrip";
import { shortPath } from "./text";
import { elapsed } from "./time";
import type { RunInfo } from "./types";
import type { MapState } from "./useReleaseMap";

interface Props {
  state: MapState;
  onRetry: () => void;
  runs: RunInfo[];
  now: number;
  onFollow: (run: RunInfo) => void;
  /** The first run's button, for focus coming back from a run left going. */
  runsButton: Ref<HTMLButtonElement>;
  /** Select a cell and move focus to it. */
  onProblem: (row: number, col: number) => void;
  /** Outcome lines of cells with a run going now, by `repoRoot\nenv`. */
  live: Record<string, string>;
  /** A deploy screen is open: it owns navigation until it is left. */
  busy: boolean;
}

export function Sidebar({ state, onRetry, runs, now, onFollow, runsButton, onProblem, live, busy }: Props) {
  const map =
    !busy && state.phase === "shown" && state.loaded.map.screen !== "firstRun" ? state.loaded.map : null;
  const attention = map
    ? map.rows.flatMap((row, r) =>
        row.cells.flatMap((cell, c) =>
          cell.kind === "present" && cell.needsAttention ? [{ row, cell, r, c, env: map.envs[c] }] : [],
        ),
      )
    : [];
  const labels = map?.rows.map((row) => row.label) ?? [];
  const twin = (label: string) => labels.indexOf(label) !== labels.lastIndexOf(label);
  return (
    <nav className="sidebar" aria-label="Sidebar">
      <div className="titlebar-drag" data-tauri-drag-region />
      {!busy && (
        <section className="side-section" aria-labelledby="side-runs">
          <h2 id="side-runs" className="side-title caps muted">
            Deploys running
          </h2>
          {runs.length === 0 ? (
            <p className="side-empty muted">None started here</p>
          ) : (
            <ul className="side-list">
              {runs.map((run, index) => (
                <li key={run.handle}>
                  <button
                    type="button"
                    className="side-item"
                    ref={index === 0 ? runsButton : undefined}
                    onClick={() => onFollow(run)}
                  >
                    <span className="side-mark" aria-hidden>
                      <Mark kind="live" />
                    </span>
                    <span className="side-name">
                      {run.project} → {run.env}
                    </span>{" "}
                    <span className="side-meta muted">{elapsed(now - run.startedAtMs)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}
      {map && (
        <section className="side-section" aria-labelledby="side-attention">
          <h2 id="side-attention" className="side-title caps muted">
            Needs attention{attention.length > 0 && <span className="side-count">{attention.length}</span>}
          </h2>
          {attention.length === 0 ? (
            <p className="side-empty muted">Nothing on this Mac</p>
          ) : (
            <ul className="side-list">
              {attention.map(({ row, cell, r, c, env }) => (
                <li key={`${row.repoRoot}\n${env}`}>
                  <button type="button" className="side-item" onClick={() => onProblem(r, c)}>
                    <span className="side-mark" aria-hidden>
                      <Mark kind={cell.mark} />
                    </span>
                    <span className="side-name" title={row.repoRoot}>
                      {row.label} · {env}
                      {twin(row.label) && <span className="muted"> · {shortPath(row.repoRoot)}</span>}
                    </span>{" "}
                    <Flap status={status(cell, live[`${row.repoRoot}\n${env}`])} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}
      {map && <ProblemsStrip problems={map.problems} onSelect={onProblem} />}
      <Footer state={state} onRetry={onRetry} />
    </nav>
  );
}
