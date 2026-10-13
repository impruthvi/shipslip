import { ProblemsStrip } from "./ProblemsStrip";
import { ReleaseMap } from "./ReleaseMap";
import type { PaneView } from "./slipTarget";
import type { Selection } from "./selection";
import type { Compare } from "./ReleaseMap";
import type { MapState } from "./useReleaseMap";
import { WarningIcon } from "./Warning";

interface Props {
  state: MapState;
  onRetry: () => void;
  selected: Selection | null;
  onSelect: (row: number, col: number, view?: PaneView) => void;
  compare: Compare | null;
}

export function MapArea({ state, onRetry, selected, onSelect, compare }: Props) {
  switch (state.phase) {
    case "loading":
      return <p className="placeholder">Reading local receipts…</p>;
    case "error":
      return (
        <section className="banner" role="alert">
          <WarningIcon />
          <div className="banner-body">
            <div className="banner-title">Can't read the receipts folder</div>
            <div className="mono banner-detail">{state.message}</div>
          </div>
          <button type="button" className="link" onClick={onRetry}>
            Retry ⌘R
          </button>
        </section>
      );
    case "shown": {
      const { map } = state.loaded;
      if (map.screen === "firstRun") {
        return (
          <section className="first-run">
            <p className="first-run-title">No deploys or approvals recorded yet.</p>
            <ol className="first-run-steps mono">
              <li>
                <span className="muted">1</span> cd your-laravel-app && slip trust staging
              </li>
              <li>
                <span className="muted">2</span> slip deploy staging
              </li>
            </ol>
            <p className="muted">This window refreshes when you come back to it.</p>
          </section>
        );
      }
      return (
        <>
          <ProblemsStrip problems={map.problems} onSelect={onSelect} />
          {map.screen === "noneReadable" ? (
            <p className="placeholder">No readable receipts</p>
          ) : (
            <ReleaseMap map={map} selected={selected} onSelect={onSelect} compare={compare} />
          )}
        </>
      );
    }
  }
}
