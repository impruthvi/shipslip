import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import { PANE_DEFAULT, useNarrow } from "./pane";

vi.mock("./api", () => ({
  api: {
    loadMap: async () => fixtures.map,
    compare: async () => ({ kind: "notFound" }),
    loadReceipt: async (_generation: number, id: number) => fixtures.receipts[String(id)],
    copyText: async () => {},
    onFocus: async () => () => {},
    receiptSeen: async () => {},
  },
}));

let windowWidth: number;
let queries: { list: MediaQueryList; max: number; listeners: Set<() => void> }[];

afterEach(() => vi.unstubAllGlobals());

beforeEach(() => {
  localStorage.clear();
  windowWidth = 1280;
  queries = [];
  vi.stubGlobal("matchMedia", (media: string) => {
    const max = Number(media.match(/max-width: (\d+)px/)![1]);
    const listeners = new Set<() => void>();
    const list = {
      get matches() { return windowWidth <= max; },
      media,
      addEventListener: (_type: string, callback: () => void) => listeners.add(callback),
      removeEventListener: (_type: string, callback: () => void) => listeners.delete(callback),
    } as unknown as MediaQueryList;
    queries.push({ list, max, listeners });
    return list;
  });
});

function resizeWindow(width: number) {
  act(() => {
    windowWidth = width;
    for (const { listeners } of queries) for (const listener of listeners) listener();
  });
}

test("1280px keeps the default inspector alongside the board", async () => {
  render(<App />);
  await act(async () => {});
  expect(screen.getByRole("complementary", { name: "Receipt" })).not.toHaveClass("pane-drawer");
});

test("a saved wider inspector at 1280px opens as a drawer instead of squeezing the board", async () => {
  localStorage.setItem("shipslip.paneWidth", "460");
  render(<App />);
  await act(async () => {});
  expect(screen.queryByRole("complementary", { name: "Receipt" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /Receipt/ }));
  expect(screen.getByRole("complementary", { name: "Receipt" })).toHaveClass("pane-drawer");
  expect(localStorage.getItem("shipslip.paneWidth")).toBe("460");
});

test("drawer mode updates for pane resizing and subsequent window resizing", () => {
  const { result, rerender } = renderHook(({ width }) => useNarrow(width), {
    initialProps: { width: PANE_DEFAULT },
  });
  expect(result.current).toBe(false);
  rerender({ width: 460 });
  expect(result.current).toBe(true);
  resizeWindow(1440);
  expect(result.current).toBe(false);
  resizeWindow(1280);
  expect(result.current).toBe(true);
  rerender({ width: PANE_DEFAULT });
  expect(result.current).toBe(false);
});
