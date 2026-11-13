import { useState, type ReactNode, type Ref } from "react";
import { Flap, reason, status, type StatusTone } from "./Cell";
import { Footer } from "./Footer";
import { Mark, markLabel } from "./Mark";
import { ConnectionGap } from "./ConnectionGap";
import { ProblemsStrip } from "./ProblemsStrip";
import { twinFolder } from "./text";
import { elapsed } from "./time";
import type { MapView, RunInfo } from "./types";
import type { MapState } from "./useReleaseMap";

const rank: Record<StatusTone, number> = { bad: 0, attention: 1, running: 2, ink: 3, muted: 4 };
/** From this many items, Needs attention shows only the worst few so Projects stays in view. */
const collapseAttentionAt = 5;
const collapsedAttention = 3;

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
  /** The checkout shown in the main area; null on All projects. */
  current: string | null;
  onAllProjects: () => void;
  onCheckout: (repoRoot: string) => void;
  onAddProject: () => void;
  /** Focus comes back here when the Add project sheet closes. */
  addButton?: Ref<HTMLButtonElement>;
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
  className = "",
  current = false,
  title,
  children,
}: {
  busy: boolean;
  onClick: () => void;
  buttonRef?: Ref<HTMLButtonElement>;
  className?: string;
  /** The screen this line opens is shown. */
  current?: boolean;
  title?: string;
  children: ReactNode;
}) {
  if (busy) return <div className={`side-item side-item-static ${className}`} title={title}>{children}</div>;
  return (
    <button
      type="button"
      className={`side-item ${className}`}
      ref={buttonRef}
      aria-current={current ? "page" : undefined}
      title={title}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

/** One mark per environment, in map column order. The mark keeps its evidence
 * tone; the status word is only named on hover. */
function EnvMarks({ map, row, live }: { map: MapView; row: MapView["rows"][number]; live: Record<string, string> }) {
  return (
    <span className="side-marks">
      {row.cells.map((cell, col) => {
        if (cell.kind !== "present") return null;
        const env = map.envs[col];
        const tip = `${env}: ${status(cell, live[`${row.repoRoot}\n${env}`]).word}`;
        return (
          <span key={env} className="side-mark" role="img" title={tip} aria-label={`${tip}, ${markLabel[cell.mark]}`}>
            <span aria-hidden><Mark kind={cell.mark} size={8} /></span>
          </span>
        );
      })}
    </span>
  );
}

export function Sidebar({
  state,
  onRetry,
  runs,
  now,
  onFollow,
  runsButton,
  onProblem,
  live,
  busy,
  current,
  onAllProjects,
  onCheckout,
  onAddProject,
  addButton,
}: Props) {
  const [allAttention, setAllAttention] = useState(false);
  const map = state.phase === "shown" && state.loaded.map.screen !== "firstRun" ? state.loaded.map : null;
  const labels = map?.rows.map((row) => row.label) ?? [];
  const terminal = map?.rows.flatMap((row, rowIndex) =>
    row.cells.flatMap((cell, colIndex) => {
      const env = map.envs[colIndex];
      if (cell.kind !== "present" || !cell.terminalRun
        || runs.some((run) => run.checkout === row.repoRoot && run.env === env)) return [];
      return [{ row, rowIndex, colIndex, env, run: cell.terminalRun }];
    }),
  ) ?? [];
  const attention = map
    ? map.rows
        .flatMap((row, r) =>
          row.cells.flatMap((cell, c) => {
            const env = map.envs[c];
            if (cell.kind !== "present" || !cell.needsAttention || cell.terminalRun
              || live[`${row.repoRoot}\n${env}`]) return [];
            const running = live[`${row.repoRoot}\n${env}`];
            return [{ row, cell, r, c, env, said: status(cell, running), why: reason(cell, running) }];
          }),
        )
        .sort((a, b) => rank[a.said.tone] - rank[b.said.tone])
    : [];
  const collapsible = attention.length >= collapseAttentionAt;
  const attentionShown = collapsible && !allAttention ? attention.slice(0, collapsedAttention) : attention;
  return (
    <nav className="sidebar" aria-label="Sidebar">
      <div className="titlebar-drag" data-tauri-drag-region />
      <div className="side-top">
        <button type="button" className="button side-add" ref={addButton} disabled={busy} onClick={onAddProject}>
          Add project
        </button>
        <Item busy={busy} className="side-all" current={current === null} onClick={onAllProjects}>
          All projects
        </Item>
      </div>
      <section className="side-section" aria-labelledby="side-runs">
        <h2 id="side-runs" className="side-title">
          Deploys running
        </h2>
        {runs.length === 0 && terminal.length === 0 ? (
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
                      <Mark kind={run.connectionGap ? "notKnown" : "live"} />
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
                      {run.connectionGap
                        ? <ConnectionGap gap={run.connectionGap} now={now} />
                        : <span className="side-meta">{elapsed(now - run.startedAtMs)}</span>}
                    </span>
                  </span>
                </Item>
              </li>
            ))}
            {terminal.map(({ row, rowIndex, colIndex, env, run }, index) => (
              <li key={run.runId}>
                <Item
                  busy={busy}
                  onClick={() => onProblem(rowIndex, colIndex)}
                  buttonRef={runs.length === 0 && index === 0 ? runsButton : undefined}
                >
                  <span className="side-line">
                    <span className="side-mark" aria-hidden><Mark kind="live" /></span>
                    <span className="side-name" title={row.repoRoot}>{row.label}</span>
                  </span>{" "}
                  <span className="side-why">
                    <span className="side-why-text muted">
                      {env}<Folder path={twinFolder(labels, row.label, row.repoRoot)} />
                      {" · Running from the terminal"}
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
              {attentionShown.map(({ row, cell, r, c, env, said, why }) => (
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
          {collapsible && (
            <button
              type="button"
              className="link side-more"
              aria-expanded={allAttention}
              onClick={() => setAllAttention(!allAttention)}
            >
              {allAttention ? "Show less" : `Show all ${attention.length}`}
            </button>
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
      {map && (
        <section className="side-section" aria-labelledby="side-projects">
          <h2 id="side-projects" className="side-title">
            Projects
          </h2>
          <ul className="side-list">
            {map.rows.map((row) => (
              <li key={row.repoRoot}>
                <Item
                  busy={busy}
                  className="side-checkout"
                  current={current === row.repoRoot}
                  title={row.repoRoot}
                  onClick={() => onCheckout(row.repoRoot)}
                >
                  <span className="side-line">
                    <span className="side-name">
                      {row.label}
                      <Folder path={twinFolder(labels, row.label, row.repoRoot)} />
                    </span>
                    {row.pathMissing ? (
                      <span className="side-state muted">checkout not found</span>
                    ) : row.setupNotFinished ? (
                      <span className="side-state muted">Setup not finished</span>
                    ) : row.configUnreadable ? (
                      <span className="side-state tone-attention">.shipslip.toml can't be read</span>
                    ) : row.needsApproval ? (
                      <span className="side-state tone-attention">Needs approval</span>
                    ) : (
                      <EnvMarks map={map} row={row} live={live} />
                    )}
                  </span>
                </Item>
              </li>
            ))}
          </ul>
        </section>
      )}
      <Footer state={state} onRetry={onRetry} />
    </nav>
  );
}
