import { useCallback, useEffect, useRef, useState } from "react";
import { useAnnouncer } from "./Announcer";
import { ComparePicker } from "./ComparePicker";
import { DeployEntryBar, useDeployEntry } from "./DeployEntry";
import { Footer } from "./Footer";
import { MapArea } from "./MapArea";
import { useNarrow, usePaneWidth } from "./pane";
import { ReceiptPane, type SlipMeta } from "./ReceiptPane";
import { target, type PaneView } from "./slipTarget";
import { useCompare } from "./useCompare";
import { useReceipt, type Request } from "./useReceipt";
import { useReleaseMap } from "./useReleaseMap";
import { useSeen } from "./useSeen";
import { useSelection } from "./useSelection";
import { Workspace, type DeployTarget, type Leave } from "./Workspace";

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
  const narrow = useNarrow();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [paneView, setPaneView] = useState<{ key: string; view: PaneView } | null>(null);
  /** Set by a keyboard open; the pane focuses the slip once it shows that receipt. */
  const [focusReceipt, setFocusReceipt] = useState(false);
  const [workspace, setWorkspace] = useState<DeployTarget | null>(null);
  /** Leaving the workspace returns focus to "Review deploy…". */
  const [focusEntry, setFocusEntry] = useState(false);
  const entryButton = useRef<HTMLButtonElement>(null);
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
  const view = paneView?.key === selectedKey ? paneView.view : "code";
  const row = selected && map ? map.rows[selected.row] : undefined;
  const cell = selected ? row?.cells[selected.col] : undefined;
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
  );
  const onReview = () => {
    if (!loaded || !selected || !row || entry?.kind !== "ready") return;
    setWorkspace({
      generation: loaded.generation,
      row: selected.row,
      env: selected.env,
      label: row.label,
      production: entry.production,
    });
  };
  const onLeave = useCallback(
    (how: Leave) => {
      setWorkspace(null);
      setFocusEntry(true);
      if (how === "finished") {
        // The new receipt opens in the pane, on the same cell.
        setDrawerOpen(true);
        refresh();
      }
    },
    [refresh],
  );
  useEffect(() => {
    if (!focusEntry || !entryButton.current) return;
    entryButton.current.focus();
    setFocusEntry(false);
  }, [focusEntry, entry]);

  const onSelect = useCallback(
    (r: number, col: number, next: PaneView = "code") => {
      if (!map) return;
      select(r, col);
      setPaneView({ key: cellKey(map.rows[r].repoRoot, map.envs[col]), view: next });
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
      <main className="main">
        <header className="toolbar">
          <span className="wordmark">Shipslip</span>
          {map?.screen === "map" && !workspace && (
            <ComparePicker envs={map.envs} pair={pair} onChange={setPair} />
          )}
          {narrow && request && !workspace && (
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
        {workspace ? (
          <Workspace target={workspace} onLeave={onLeave} announce={announce} />
        ) : (
          <>
            <div className="title">
              <h1>Release map</h1>
              <p className="muted">
                Last recorded code on each app and environment. Nothing here is live.
              </p>
            </div>
            <div className="map-area">
              <MapArea
                state={state}
                onRetry={refresh}
                selected={selected}
                onSelect={onSelect}
                onEnter={onEnter}
                onProblem={onProblem}
                onCellFocus={onCellFocus}
                compare={pair ? { pair, lines } : null}
              />
            </div>
            <Footer state={state} onRetry={refresh} />
          </>
        )}
      </main>
      {showPane && request && (
        <ReceiptPane
          requestKey={request.key}
          focusSlip={focusReceipt}
          onSlipFocused={() => setFocusReceipt(false)}
          opening={request.meta}
          receipt={receipt}
          onView={onView}
          width={width}
          onResize={resize}
          drawer={narrow}
          entry={<DeployEntryBar entry={entry} onReview={onReview} buttonRef={entryButton} />}
        />
      )}
      {regions}
    </div>
  );
}
