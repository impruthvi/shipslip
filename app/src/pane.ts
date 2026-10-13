import { useCallback, useEffect, useState } from "react";

export const PANE_MIN = 360;
export const PANE_MAX = 560;
export const PANE_DEFAULT = 460;
/** Below this window width the pane becomes a drawer. */
export const DRAWER_BELOW = 1200;

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

function query(): MediaQueryList | null {
  return typeof window.matchMedia === "function"
    ? window.matchMedia(`(max-width: ${DRAWER_BELOW - 1}px)`)
    : null;
}

/** True while the window is too narrow for the pane beside the map. */
export function useNarrow(): boolean {
  const [narrow, setNarrow] = useState(() => query()?.matches ?? false);
  useEffect(() => {
    const list = query();
    if (!list) return;
    const onChange = () => setNarrow(list.matches);
    list.addEventListener("change", onChange);
    return () => list.removeEventListener("change", onChange);
  }, []);
  return narrow;
}
