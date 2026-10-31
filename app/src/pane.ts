import { useCallback, useMemo, useState, useSyncExternalStore } from "react";

export const PANE_MIN = 340;
export const PANE_MAX = 560;
export const PANE_DEFAULT = 380;
/** Sidebar (200), main padding (40), app column (150), two environment
 * columns (232 each), and their gaps (16). Keep in sync with styles.css. */
const BOARD_WIDTH = 870;

const WIDTH_KEY = "shipslip.paneWidth";

export function clampWidth(width: number): number {
  return Math.round(Math.min(PANE_MAX, Math.max(PANE_MIN, width)));
}

function storedWidth(): number {
  const value = Number(localStorage.getItem(WIDTH_KEY));
  return Number.isFinite(value) && value > 0 ? clampWidth(value) : PANE_DEFAULT;
}

export function usePaneWidth() {
  const [width, setWidth] = useState(storedWidth);
  const resize = useCallback((next: number) => {
    const clamped = clampWidth(next);
    localStorage.setItem(WIDTH_KEY, String(clamped));
    setWidth(clamped);
  }, []);
  return { width, resize };
}

function query(width: number): MediaQueryList | null {
  return typeof window.matchMedia === "function"
    ? window.matchMedia(`(max-width: ${BOARD_WIDTH + width - 1}px)`)
    : null;
}

/** True while the window is too narrow for the pane beside the map. */
export function useNarrow(width = PANE_DEFAULT): boolean {
  const list = useMemo(() => query(width), [width]);
  const subscribe = useCallback((onChange: () => void) => {
    list?.addEventListener("change", onChange);
    return () => list?.removeEventListener("change", onChange);
  }, [list]);
  return useSyncExternalStore(subscribe, () => list?.matches ?? false, () => false);
}
