import { useCallback, useEffect, useRef, useState } from "react";
import { AddProject } from "./AddProject";
import { useAnnouncer } from "./Announcer";
import { CheckoutPane } from "./CheckoutPane";
import { ComparePicker } from "./ComparePicker";
import { liveText } from "./Cell";
import { DeployEntryBar, useDeployEntry, type EntryChoice } from "./DeployEntry";
import { InspectorHead } from "./Inspector";
import { MapArea } from "./MapArea";
import { useNarrow, usePaneWidth } from "./pane";
import { ReceiptPane, type SlipMeta } from "./ReceiptPane";
import { Sidebar } from "./Sidebar";
import { roleOf, target, type PaneView } from "./slipTarget";
import { useCompare } from "./useCompare";
import { useReceipt, type Request } from "./useReceipt";
import { useReleaseMap } from "./useReleaseMap";
import { useSeen } from "./useSeen";
import { useLiveRuns } from "./useLiveRuns";
import { useSelection } from "./useSelection";
import { Workspace, type DeployTarget, type Leave, type Next, type Start } from "./Workspace";
import { notify } from "./notify";
import { twinFolder } from "./text";
import { deploy } from "./deployApi";
import type { Loaded, ReadyEntry, RunInfo } from "./types";

const cellKey = (repoRoot: string, env: string) => `${repoRoot}\n${env}`;

/** What the main area shows: the release map, or one checkout. */
type Screen = { kind: "all" } | { kind: "checkout"; repoRoot: string };

function focusCell(row: number, col: number) {
  document.querySelector<HTMLElement>(`[role="gridcell"][data-row="${row}"][data-col="${col}"]`)?.focus();
}

/** A map read while the workspace is open moves the target to the newer map. */
function targetOnMap(target: DeployTarget, loaded: Loaded | null): DeployTarget {
  const row = loaded?.map.rows.findIndex((r) => r.repoRoot === target.checkout) ?? -1;
  return loaded && row >= 0 ? { ...target, generation: loaded.generation, row } : target;
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
  const [screenShown, setScreenShown] = useState<Screen>({ kind: "all" });
  /** Counts sidebar moves; each one focuses the new screen's heading. */
  const [moved, setMoved] = useState(0);
  /** A sidebar problem's cell, focused once the map has drawn it. */
  const [focusGrid, setFocusGrid] = useState<{ row: number; col: number } | null>(null);
  /** In a checkout: the receipt its pane shows, by cell. `id` null opens the
   * cell's own receipt, latest risk first. */
  const [opened, setOpened] = useState<{ key: string; id: number | null } | null>(null);
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
  const heading = useRef<HTMLHeadingElement>(null);
  /** The Add project sheet; `reopen` opens a "Setup not finished" row. */
  const [adding, setAdding] = useState<{ reopen: { generation: number; row: number } | null } | null>(null);
  const addButton = useRef<HTMLButtonElement>(null);
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
  const checkoutRow =
    screenShown.kind === "checkout" && map ? map.rows.findIndex((r) => r.repoRoot === screenShown.repoRoot) : -1;
  // A checkout gone from the map leaves its screen for All projects.
  const inCheckout = checkoutRow >= 0;
  // Latest risk first: a failed last attempt opens before the older success.
  const riskFirst: PaneView = cell?.kind === "present" && cell.lastAttempt?.outcome.tone === "bad" ? "last" : "code";
  const view = paneView?.key === selectedKey ? paneView.view : riskFirst;
  const picked = inCheckout && opened?.key === selectedKey ? opened : null;
  let request: Request<SlipMeta> | null = null;
  if (loaded && selected && row && cell?.kind === "present" && (!inCheckout || picked)) {
    const opens = picked && picked.id !== null ? { id: picked.id, role: roleOf(cell, picked.id) } : target(cell, view);
    request = {
      generation: loaded.generation,
      id: opens.id,
      key: `${selectedKey}\n${picked && picked.id !== null ? `receipt ${picked.id}` : view}`,
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
  const { live, now, forget } = useLiveRuns(
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

  const start = (r: number, env: string, entry: ReadyEntry, choice: EntryChoice) => {
    if (!loaded || !map) return;
    const row = map.rows[r];
    const target = {
      generation: loaded.generation,
      row: r,
      checkout: row.repoRoot,
      env,
      label: row.label,
      folder: twinFolder(map.rows.map((x) => x.label), row.label, row.repoRoot),
      production: entry.production,
    };
    let next: Next;
    if (choice.kind === "review") next = { kind: "deploy", plan: "Deploy" };
    else if (choice.kind === "rerun") next = { kind: "deploy", plan: "Rerun" };
    else if (choice.kind === "fromStep") next = { kind: "pick", steps: entry.steps, step: choice.step };
    else next = { kind: choice.kind };
    // A config that needs approval is reviewed first, then the choice goes on.
    const first: Start = entry.needsTrust ? { kind: "trust", then: next } : next;
    setApproved(null);
    setWorkspace({ target, start: first });
  };
  const onChoose = (choice: EntryChoice) => {
    if (selected && entry?.kind === "ready") start(selected.row, selected.env, entry, choice);
  };
  /** A checkout's Deploy: its cell becomes the selection, so the run's
   * receipt opens there when it ends. */
  /** Selects the shown checkout's cell for `env`; false when it has none. */
  const selectInCheckout = (env: string) => {
    const col = map?.envs.indexOf(env) ?? -1;
    if (!inCheckout || col < 0) return false;
    select(checkoutRow, col);
    return true;
  };
  const onCheckoutDeploy = (env: string, entry: ReadyEntry) => {
    if (selectInCheckout(env)) start(checkoutRow, env, entry, { kind: "review" });
  };
  const onOpenReceipt = (env: string, id: number) => {
    if (!map || !selectInCheckout(env)) return;
    setOpened({ key: cellKey(map.rows[checkoutRow].repoRoot, env), id });
    setPrintFor(null);
    setDrawerOpen(true);
    setFocusReceipt(false);
  };
  const go = (next: Screen) => {
    setScreenShown(next);
    setOpened(null);
    setFocusEntry(null);
    setMoved((n) => n + 1);
  };
  useEffect(() => {
    if (moved > 0) heading.current?.focus();
  }, [moved]);
  /** The toolbar's way back to a run: its cell is selected, then its run shown. */
  const follow = (run: RunInfo) => {
    const r = map?.rows.findIndex((x) => x.repoRoot === run.checkout) ?? -1;
    const col = map?.envs.indexOf(run.env) ?? -1;
    if (map && r >= 0 && col >= 0 && map.rows[r].cells[col].kind === "present") select(r, col);
    setWorkspace({
      target: {
        generation: loaded?.generation ?? 0,
        row: r,
        checkout: run.checkout,
        env: run.env,
        label: r >= 0 && map ? map.rows[r].label : run.project,
        folder: map ? twinFolder(map.rows.map((x) => x.label), run.project, run.checkout) : null,
        production: run.production,
      },
      start: { kind: "follow", info: run },
    });
  };
  const onLeave = useCallback(
    (how: Leave) => {
      const left = workspaceRef.current;
      setWorkspace(null);
      refresh();
      setFocusEntry(how === "away" ? "runs" : "entry");
      setEntryVersion((v) => v + 1);
      if (how === "approved" && left) {
        setApproved({ key: selectedKey ?? "", env: left.target.env });
      }
      if (how === "finished") {
        // The new receipt opens in the pane, on the same cell, risk first.
        setPaneView(null);
        setOpened(selectedKey ? { key: selectedKey, id: null } : null);
        setPrintFor(selectedKey);
        setDrawerOpen(true);
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
      // Problems point at the map's cells.
      setScreenShown({ kind: "all" });
      onSelect(r, col);
      setFocusGrid({ row: r, col });
    },
    [onSelect],
  );
  useEffect(() => {
    if (!focusGrid) return;
    focusCell(focusGrid.row, focusGrid.col);
    setFocusGrid(null);
  }, [focusGrid]);
  /** Focus moved on in the grid: a pending keyboard open no longer owns it. */
  const onCellFocus = useCallback(() => setFocusReceipt(false), []);
  const onView = useCallback(
    (next: PaneView) => {
      if (!selectedKey) return;
      setPaneView({ key: selectedKey, view: next });
      setOpened((prev) => (prev?.key === selectedKey ? { key: selectedKey, id: null } : prev));
    },
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
  const drawerButton = narrow && request && (
    <button
      type="button"
      className="toolbar-button"
      aria-expanded={drawerOpen}
      onClick={() => setDrawerOpen(!drawerOpen)}
    >
      Receipt <kbd className="muted">⌘I</kbd>
    </button>
  );
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
        current={inCheckout && screenShown.kind === "checkout" ? screenShown.repoRoot : null}
        onAllProjects={() => go({ kind: "all" })}
        onCheckout={(repoRoot) => {
          const row = map?.rows.findIndex((r) => r.repoRoot === repoRoot) ?? -1;
          if (loaded && row >= 0 && loaded.map.rows[row].setupNotFinished) {
            setAdding({ reopen: { generation: loaded.generation, row } });
          } else go({ kind: "checkout", repoRoot });
        }}
        onAddProject={() => setAdding({ reopen: null })}
        addButton={addButton}
      />
      <main className="main">
        {!workspace && !inCheckout && (
          <header className="toolbar" data-tauri-drag-region>
            <h1 className="toolbar-title" ref={heading} tabIndex={-1} data-tauri-drag-region>
              All projects
            </h1>
            <p className="toolbar-subtitle muted" data-tauri-drag-region>
              What this Mac recorded for each app and environment
            </p>
            {map?.screen === "map" && (
              <ComparePicker envs={map.envs} pair={pair} onChange={setPair} />
            )}
            {drawerButton}
          </header>
        )}
        {workspace ? (
          <Workspace
            target={targetOnMap(workspace.target, loaded)}
            start={workspace.start}
            onLeave={onLeave}
            onRunEnded={forget}
            announce={announce}
          />
        ) : inCheckout && loaded ? (
          <CheckoutPane
            key={map?.rows[checkoutRow].repoRoot}
            generation={loaded.generation}
            map={loaded.map}
            row={checkoutRow}
            live={liveLines}
            folder={twinFolder(loaded.map.rows.map((x) => x.label), loaded.map.rows[checkoutRow].label, loaded.map.rows[checkoutRow].repoRoot)}
            entryVersion={entryVersion}
            openedReceipt={showPane && request ? (picked?.id ?? request.id) : null}
            onDeploy={onCheckoutDeploy}
            onOpenReceipt={onOpenReceipt}
            heading={heading}
            focusDeploy={
              focusEntry === "entry" && selected?.repoRoot === loaded.map.rows[checkoutRow].repoRoot
                ? selected.env
                : null
            }
            onDeployFocused={() => setFocusEntry(null)}
            onRemoved={() => {
              refresh();
              go({ kind: "all" });
            }}
            tools={drawerButton}
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
                onAddProject={() => setAdding({ reopen: null })}
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
          attachRunId={
            entry?.kind === "ready" && entry.recovery?.actions.some((a) => a.kind === "attach")
              ? entry.recovery.runId
              : null
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
              buttonRef={inCheckout ? undefined : entryButton}
            />
          }
        />
      )}
      {adding && (
        <AddProject
          loaded={loaded}
          reopen={adding.reopen}
          onAdded={refresh}
          onClose={() => {
            setAdding(null);
            addButton.current?.focus();
          }}
          onDone={(repoRoot) => {
            setAdding(null);
            refresh();
            go({ kind: "checkout", repoRoot });
          }}
        />
      )}
      {regions}
    </div>
  );
}
