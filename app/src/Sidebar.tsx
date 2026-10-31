import type { ReactNode, Ref } from "react";
import { Flap, reason, status, type StatusTone } from "./Cell";
import { Footer } from "./Footer";
import { Mark } from "./Mark";
import { ProblemsStrip } from "./ProblemsStrip";
import { twinFolder } from "./text";
import { elapsed } from "./time";
import type { RunInfo } from "./types";
import type { MapState } from "./useReleaseMap";

const rank: Record<StatusTone, number> = { bad: 0, attention: 1, running: 2, ink: 3, muted: 4 };

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
  /** A deploy screen is open: it owns navigation, so the lists only show. */
  busy: boolean;
}

/** Which checkout a same-named app is, kept whole so it never breaks mid-name. */
function Folder({ path }: { path: string | null }) {
  return path && <span className="side-folder"> · in {path}</span>;
}

/** A sidebar line: a button, or plain text while a deploy screen is open. */
function Item({
  busy,
  onClick,
  buttonRef,
  children,
}: {
  busy: boolean;
  onClick: () => void;
  buttonRef?: Ref<HTMLButtonElement>;
  children: ReactNode;
}) {
  if (busy) return <div className="side-item side-item-static">{children}</div>;
  return (
    <button type="button" className="side-item" ref={buttonRef} onClick={onClick}>
      {children}
    </button>
  );
}

export function Sidebar({ state, onRetry, runs, now, onFollow, runsButton, onProblem, live, busy }: Props) {
  const map = state.phase === "shown" && state.loaded.map.screen !== "firstRun" ? state.loaded.map : null;
  const labels = map?.rows.map((row) => row.label) ?? [];
  const attention = map
    ? map.rows
        .flatMap((row, r) =>
          row.cells.flatMap((cell, c) => {
            if (cell.kind !== "present" || !cell.needsAttention) return [];
            const env = map.envs[c];
            const running = live[`${row.repoRoot}\n${env}`];
            return [{ row, cell, r, c, env, said: status(cell, running), why: reason(cell, running) }];
          }),
        )
        .sort((a, b) => rank[a.said.tone] - rank[b.said.tone])
    : [];
  return (
    <nav className="sidebar" aria-label="Sidebar">
      <div className="titlebar-drag" data-tauri-drag-region />
      <section className="side-section" aria-labelledby="side-runs">
        <h2 id="side-runs" className="side-title">
          Deploys running
        </h2>
        {runs.length === 0 ? (
          <p className="side-empty muted">None started here</p>
        ) : (
          <ul className="side-list">
            {runs.map((run, index) => (
              <li key={run.handle}>
                <Item
                  busy={busy}
                  onClick={() => onFollow(run)}
                  buttonRef={index === 0 ? runsButton : undefined}
                >
                  <span className="side-line">
                    <span className="side-mark" aria-hidden>
                      <Mark kind="live" />
                    </span>
                    <span className="side-name" title={run.checkout}>
                      {run.project}
                    </span>
                  </span>{" "}
                  <span className="side-why">
                    <span className="side-why-text muted">
                      {run.env}
                      <Folder path={twinFolder(
                          [...labels, ...runs.filter((x) => x.checkout !== run.checkout).map((x) => x.project)],
                          run.project,
                          run.checkout,
                        )} />
                      {" · "}
                      <span className="side-meta">{elapsed(now - run.startedAtMs)}</span>
                    </span>
                  </span>
                </Item>
              </li>
            ))}
          </ul>
        )}
      </section>
      {map && (
        <section className="side-section" aria-labelledby="side-attention">
          <h2 id="side-attention" className="side-title">
            Needs attention{attention.length > 0 && <span className="side-count">{attention.length}</span>}
          </h2>
          {attention.length === 0 ? (
            <p className="side-empty muted">Nothing on this Mac</p>
          ) : (
            <ul className="side-list">
              {attention.map(({ row, cell, r, c, env, said, why }) => (
                <li key={`${row.repoRoot}\n${env}`}>
                  <Item busy={busy} onClick={() => onProblem(r, c)}>
                    <span className="side-line">
                      <span className="side-mark" aria-hidden>
                        <Mark kind={cell.mark} />
                      </span>
                      <span className="side-name" title={row.repoRoot}>
                        {row.label}
                      </span>
                    </span>{" "}
                    <span className="side-why" title={why?.text}>
                      <Flap status={said} />{" "}
                      <span className="side-why-text muted">
                        {env}
                        <Folder path={twinFolder(labels, row.label, row.repoRoot)} />
                      </span>
                    </span>
                  </Item>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}
      {map && !busy && (
        <ProblemsStrip
          problems={map.problems}
          onSelect={onProblem}
          cellName={(row, col) => `${map.rows[row]?.label} · ${map.envs[col]}`}
        />
      )}
      <Footer state={state} onRetry={onRetry} />
    </nav>
  );
}
