import { useCallback, useEffect, useState } from "react";
import { ComparePicker } from "./ComparePicker";
import { Footer } from "./Footer";
import { MapArea } from "./MapArea";
import { useNarrow, usePaneWidth } from "./pane";
import { ReceiptPane, type SlipMeta } from "./ReceiptPane";
import { target, type PaneView } from "./slipTarget";
import { useCompare } from "./useCompare";
import { useReceipt, type Request } from "./useReceipt";
import { useReleaseMap } from "./useReleaseMap";
import { useSelection } from "./useSelection";

const cellKey = (repoRoot: string, env: string) => `${repoRoot}\n${env}`;

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

  const onSelect = useCallback(
    (r: number, col: number, next: PaneView = "code") => {
      if (!map) return;
      select(r, col);
      setPaneView({ key: cellKey(map.rows[r].repoRoot, map.envs[col]), view: next });
      setDrawerOpen(true);
    },
    [map, select],
  );
  const onView = useCallback(
    (next: PaneView) => selectedKey && setPaneView({ key: selectedKey, view: next }),
    [selectedKey],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey && event.key.toLowerCase() === "i") {
        event.preventDefault();
        setDrawerOpen((open) => !open);
      } else if (event.key === "Escape") {
        setDrawerOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const showPane = request !== null && (!narrow || drawerOpen);
  return (
    <div className="window">
      <main className="main">
        <header className="toolbar">
          <span className="wordmark">Shipslip</span>
          {map?.screen === "map" && <ComparePicker envs={map.envs} pair={pair} onChange={setPair} />}
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
            compare={pair ? { pair, lines } : null}
          />
        </div>
        <Footer state={state} onRetry={refresh} />
      </main>
      {showPane && request && (
        <ReceiptPane
          opening={request.meta}
          receipt={receipt}
          onView={onView}
          width={width}
          onResize={resize}
          drawer={narrow}
        />
      )}
    </div>
  );
}
