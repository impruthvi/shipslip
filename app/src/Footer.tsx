import type { MapState } from "./useReleaseMap";
import { clock } from "./time";

export function Footer({ state, onRetry }: { state: MapState; onRetry: () => void }) {
  if (state.phase === "shown" && state.failure) {
    return (
      <footer className="footer footer-failed">
        <span className="footer-failed-text">
          Could not re-read at {clock(state.failure.at)} · showing {clock(state.readAt)}
        </span>
        <button type="button" className="link" onClick={onRetry}>
          Retry ⌘R
        </button>
      </footer>
    );
  }
  return (
    <footer className="footer">
      {state.phase === "shown" ? (
        <>
          <span className="muted">Read from local receipts</span>
          <span className="mono">{clock(state.readAt)} · on window focus</span>
        </>
      ) : (
        <>
          <span className="muted">Not read yet</span>
          <span className="mono">· on window focus</span>
        </>
      )}
    </footer>
  );
}
