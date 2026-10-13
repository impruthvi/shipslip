import { ComparePicker } from "./ComparePicker";
import { Footer } from "./Footer";
import { MapArea } from "./MapArea";
import { useCompare } from "./useCompare";
import { useReleaseMap } from "./useReleaseMap";
import { useSelection } from "./useSelection";

export default function App() {
  const { state, refresh } = useReleaseMap();
  const loaded = state.phase === "shown" ? state.loaded : null;
  const map = loaded?.map ?? null;
  const { selected, select } = useSelection(map);
  const { pair, setPair, lines } = useCompare(loaded);
  return (
    <div className="window">
      <main className="main">
        <header className="toolbar">
          <span className="wordmark">Shipslip</span>
          {map?.screen === "map" && <ComparePicker envs={map.envs} pair={pair} onChange={setPair} />}
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
            onSelect={select}
            compare={pair ? { pair, lines } : null}
          />
        </div>
        <Footer state={state} onRetry={refresh} />
      </main>
    </div>
  );
}
