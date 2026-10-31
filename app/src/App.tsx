import { useCallback, useEffect, useRef, useState } from "react";
import { useAnnouncer } from "./Announcer";
import { ComparePicker } from "./ComparePicker";
import { liveText } from "./Cell";
import { DeployEntryBar, useDeployEntry, type EntryChoice } from "./DeployEntry";
import { InspectorHead } from "./Inspector";
import { MapArea } from "./MapArea";
import { useNarrow, usePaneWidth } from "./pane";
import { ReceiptPane, type SlipMeta } from "./ReceiptPane";
import { Sidebar } from "./Sidebar";
import { target, type PaneView } from "./slipTarget";
import { useCompare } from "./useCompare";
import { useReceipt, type Request } from "./useReceipt";
import { useReleaseMap } from "./useReleaseMap";
import { useSeen } from "./useSeen";
import { useLiveRuns } from "./useLiveRuns";
import { useSelection } from "./useSelection";
import { Workspace, type DeployTarget, type Leave, type Start } from "./Workspace";
import { notify } from "./notify";
import { deploy } from "./deployApi";
import type { RunInfo } from "./types";

const cellKey = (repoRoot: string, env: string) => `${repoRoot}\n${env}`;

function focusCell(row: number, col: number) {
  document.querySelector<HTMLElement>(`[role="gridcell"][data-row="${row}"][data-col="${col}"]`)?.focus();
}

export default function App() {
  const { state, refresh } = useReleaseMap();
  const loaded = state.phase === "shown" ? state.loaded : null;
  const map = loaded?.map ?? null;
  const { selected, select } = useSelection(map);
  const { pair, setPair, lines } = useCompare(loaded);
  const { width, resize } = usePaneWidth();
  const narrow = useNarrow(width);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [paneView, setPaneView] = useState<{ key: string; view: PaneView } | null>(null);
  /** The cell whose run just ended: its new receipt slides up into the pane. */
  const [printFor, setPrintFor] = useState<string | null>(null);
  /** Set by a keyboard open; the pane focuses the slip once it shows that receipt. */
  const [focusReceipt, setFocusReceipt] = useState(false);
  const [workspace, setWorkspace] = useState<{ target: DeployTarget; start: Start } | null>(null);
  /** Leaving the workspace returns focus to the deploy button, or to the
   * sidebar's run link when a run keeps going. */
  const [focusEntry, setFocusEntry] = useState<"entry" | "runs" | null>(null);
  const entryButton = useRef<HTMLButtonElement>(null);
  const runsButton = useRef<HTMLButtonElement>(null);
  /** Asks for the deploy entry again after approvals and runs. */
  const [entryVersion, setEntryVersion] = useState(0);
  const [approved, setApproved] = useState<{ key: string; env: string } | null>(null);
  const { announce, regions } = useAnnouncer();
  const selectedRef = useRef(selected);
  const narrowRef = useRef(narrow);
  const workspaceRef = useRef(workspace);
  useEffect(() => {
    selectedRef.current = selected;
    narrowRef.current = narrow;
    workspaceRef.current = workspace;
  }, [selected, narrow, workspace]);

  const selectedKey = selected ? cellKey(selected.repoRoot, selected.env) : null;
  const row = selected && map ? map.rows[selected.row] : undefined;
  const cell = selected ? row?.cells[selected.col] : undefined;
  // Latest risk first: a failed last attempt opens before the older success.
  const riskFirst: PaneView = cell?.kind === "present" && cell.lastAttempt?.outcome.tone === "bad" ? "last" : "code";
  const view = paneView?.key === selectedKey ? paneView.view : riskFirst;
  let request: Request<SlipMeta> | null = null;
  if (loaded && selected && row && cell?.kind === "present") {
    const opens = target(cell, view);
    request = {
      generation: loaded.generation,
      id: opens.id,
      key: `${selectedKey}\n${view}`,
      meta: { cell, label: row.label, env: selected.env, role: opens.role },
    };
  }
  const receipt = useReceipt(request, refresh);
  const entry = useDeployEntry(
    loaded && selected && !workspace
      ? { generation: loaded.generation, row: selected.row, env: selected.env }
      : null,
    entryVersion,
  );
  // A run that ended away from the workspace shows on its cell; the selection stays.
  const { live, now } = useLiveRuns(
    useCallback(
      (ended: RunInfo[]) => {
        refresh();
        setEntryVersion((v) => v + 1);
        // A run followed in the workspace tells its own end.
        if (workspaceRef.current) return;
        for (const run of ended) {
          void notify(`${run.project} → ${run.env}`, "The run ended. Its receipt is on the release map.");
        }
      },
      [refresh],
    ),
  );
  const liveLines: Record<string, string> = {};
  for (const run of live.runs) liveLines[cellKey(run.checkout, run.env)] = liveText(run, now);
  const closed = live.closedPreviews.find(
    (preview) => selected && cellKey(preview.checkout, preview.env) === selectedKey,
  );
  const notice =
    approved && approved.key === selectedKey
      ? `Configuration approved for ${approved.env}`
      : closed
        ? closed.released
          ? "The preview closed when the window reloaded. The server lock was released; nothing ran."
          : "The preview closed when the window reloaded. Nothing ran, but the server lock could not be released."
        : null;

  const onChoose = (choice: EntryChoice) => {
    if (!loaded || !selected || !row || entry?.kind !== "ready") return;
    const target = {
      generation: loaded.generation,
      row: selected.row,
      env: selected.env,
      label: row.label,
      production: entry.production,
    };
    let start: Start;
    if (entry.needsTrust) start = { kind: "trust" };
    else if (choice.kind === "review") start = { kind: "deploy", plan: "Deploy" };
    else if (choice.kind === "rerun") start = { kind: "deploy", plan: "Rerun" };
    else if (choice.kind === "fromStep") start = { kind: "pick", steps: entry.steps, step: choice.step };
    else start = { kind: choice.kind };
    setApproved(null);
    setWorkspace({ target, start });
  };
  /** The toolbar's way back to a run: its cell is selected, then its run shown. */
  const follow = (run: RunInfo) => {
    const r = map?.rows.findIndex((x) => x.repoRoot === run.checkout) ?? -1;
    const col = map?.envs.indexOf(run.env) ?? -1;
    if (map && r >= 0 && col >= 0 && map.rows[r].cells[col].kind === "present") select(r, col);
    setWorkspace({
      target: {
        generation: loaded?.generation ?? 0,
        row: r,
        env: run.env,
        label: r >= 0 && map ? map.rows[r].label : run.project,
        production: run.production,
      },
      start: { kind: "follow", info: run },
    });
  };
  const onLeave = useCallback(
    (how: Leave) => {
      const left = workspaceRef.current;
      setWorkspace(null);
      setFocusEntry(how === "away" ? "runs" : "entry");
      setEntryVersion((v) => v + 1);
      if (how === "approved" && left) {
        setApproved({ key: selectedKey ?? "", env: left.target.env });
      }
      if (how === "finished") {
        // The new receipt opens in the pane, on the same cell, risk first.
        setPaneView(null);
        setPrintFor(selectedKey);
        setDrawerOpen(true);
        refresh();
      }
    },
    [refresh, selectedKey],
  );
  useEffect(() => {
    const button = focusEntry === "runs" ? runsButton.current : entryButton.current;
    if (!focusEntry || !button) return;
    button.focus();
    setFocusEntry(null);
  }, [focusEntry, entry, live]);

  const onSelect = useCallback(
    (r: number, col: number, next?: PaneView) => {
      if (!map) return;
      select(r, col);
      setPrintFor(null);
      setPaneView(next ? { key: cellKey(map.rows[r].repoRoot, map.envs[col]), view: next } : null);
      setDrawerOpen(true);
      setFocusReceipt(false);
    },
    [map, select],
  );
  const onEnter = useCallback(
    (r: number, col: number) => {
      onSelect(r, col);
      setFocusReceipt(true);
    },
    [onSelect],
  );
  const onProblem = useCallback(
    (r: number, col: number) => {
      onSelect(r, col);
      focusCell(r, col);
    },
    [onSelect],
  );
  /** Focus moved on in the grid: a pending keyboard open no longer owns it. */
  const onCellFocus = useCallback(() => setFocusReceipt(false), []);
  const onView = useCallback(
    (next: PaneView) => selectedKey && setPaneView({ key: selectedKey, view: next }),
    [selectedKey],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // The workspace owns Escape while it is open; the pane is hidden.
      if (workspaceRef.current) return;
      const inPane = (event.target as Element | null)?.closest?.(".pane");
      const cell = selectedRef.current;
      if (event.metaKey && event.key.toLowerCase() === "i") {
        event.preventDefault();
        setDrawerOpen((open) => !open);
        // Closing the drawer around the focus would drop it on the page.
        if (inPane && narrowRef.current && cell) focusCell(cell.row, cell.col);
      } else if (event.key === "Escape") {
        setDrawerOpen(false);
        if (inPane && cell) focusCell(cell.row, cell.col);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const showPane = !workspace && request !== null && (!narrow || drawerOpen);
  const shown = showPane && receipt.shown?.key === request?.key ? receipt.shown : null;
  useSeen(shown?.opened.kind === "receipt" ? shown.opened.runId : null);
  return (
    <div className="window">
      <Sidebar
        state={state}
        onRetry={refresh}
        runs={live.runs}
        busy={workspace !== null}
        now={now}
        onFollow={follow}
        runsButton={runsButton}
        onProblem={onProblem}
        live={liveLines}
      />
      <main className="main">
        {!workspace && (
          <header className="toolbar" data-tauri-drag-region>
            <h1 className="toolbar-title" data-tauri-drag-region>
              Release map
            </h1>
            <p className="toolbar-subtitle muted" data-tauri-drag-region>
              What this Mac recorded for each app and environment
            </p>
            {map?.screen === "map" && (
              <ComparePicker envs={map.envs} pair={pair} onChange={setPair} />
            )}
            {narrow && request && (
              <button
                type="button"
                className="toolbar-button"
                aria-expanded={drawerOpen}
                onClick={() => setDrawerOpen(!drawerOpen)}
              >
                Receipt <kbd className="muted">⌘I</kbd>
              </button>
            )}
          </header>
        )}
        {workspace ? (
          <Workspace
            target={workspace.target}
            start={workspace.start}
            onLeave={onLeave}
            announce={announce}
          />
        ) : (
          <>
            <div className="map-area">
              <MapArea
                state={state}
                onRetry={refresh}
                selected={selected}
                onSelect={onSelect}
                onEnter={onEnter}
                onCellFocus={onCellFocus}
                compare={pair ? { pair, lines } : null}
                live={liveLines}
              />
            </div>
          </>
        )}
      </main>
      {showPane && request && (
        <ReceiptPane
          requestKey={request.key}
          focusSlip={focusReceipt}
          onSlipFocused={() => setFocusReceipt(false)}
          opening={request.meta}
          print={printFor === selectedKey}
          receipt={receipt}
          onView={onView}
          width={width}
          onResize={resize}
          drawer={narrow}
          head={
            <InspectorHead
              cell={request.meta.cell}
              label={request.meta.label}
              env={request.meta.env}
              production={entry?.kind === "ready" && entry.production}
            />
          }
          entry={
            <DeployEntryBar
              entry={entry}
              target={{ label: request.meta.label, env: request.meta.env }}
              notice={notice}
              onChoose={onChoose}
              onOpenConfig={() =>
                loaded && selected
                  ? deploy.openConfig(loaded.generation, selected.row, selected.env)
                  : Promise.resolve()
              }
              buttonRef={entryButton}
            />
          }
        />
      )}
      {regions}
    </div>
  );
}
