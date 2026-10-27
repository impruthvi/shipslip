import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import type { LoadResult } from "./types";

// A run's Dock badge clears only once its receipt is seen focused. The view
// says which receipt the pane shows, again on focus; Rust checks the focus.

const loadMap = vi.fn<() => Promise<LoadResult>>();
const receiptSeen = vi.fn<(runId: string) => Promise<void>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    compare: async () => ({ kind: "notFound" }),
    loadReceipt: async (_generation: number, id: number) => fixtures.receipts[String(id)],
    copyText: async () => {},
    onFocus: async (handler: () => void) => {
      window.addEventListener("focus", handler);
      return () => window.removeEventListener("focus", handler);
    },
    receiptSeen: (runId: string) => receiptSeen(runId),
  },
}));

const map = fixtures.map.kind === "loaded" ? fixtures.map.map : (null as never);

function runId(repoRoot: string, env: string): string {
  const cell = map.rows.find((r) => r.repoRoot === repoRoot)!.cells[map.envs.indexOf(env)];
  if (cell.kind !== "present") throw new Error("absent");
  const opened = fixtures.receipts[String(cell.receipt)];
  if (opened.kind !== "opened" || opened.slip.kind !== "receipt") throw new Error("no receipt");
  return opened.slip.runId;
}

function gridcell(repoRoot: string, env: string) {
  const row = map.rows.findIndex((r) => r.repoRoot === repoRoot);
  return within(screen.getAllByRole("row")[row + 1]).getAllByRole("gridcell")[map.envs.indexOf(env)];
}

let narrow = false;
beforeEach(() => {
  localStorage.clear();
  narrow = false;
  window.matchMedia = ((query: string) => ({
    matches: narrow,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
  loadMap.mockReset().mockResolvedValue(fixtures.map);
  receiptSeen.mockReset().mockResolvedValue();
});

const settle = () => act(async () => {});

test("the pane reports the receipt it shows, again on focus and for the next one", async () => {
  render(<App />);
  await settle();
  const first = runId("/Users/you/code/api", "production");
  expect(receiptSeen).toHaveBeenLastCalledWith(first);

  receiptSeen.mockClear();
  await act(async () => {
    window.dispatchEvent(new Event("focus"));
  });
  expect(receiptSeen).toHaveBeenCalledWith(first);

  fireEvent.click(gridcell("/Users/you/code/billing", "staging"));
  await settle();
  expect(receiptSeen).toHaveBeenLastCalledWith(runId("/Users/you/code/billing", "staging"));
});

test("a closed drawer shows no receipt, so nothing is seen until it opens", async () => {
  narrow = true;
  render(<App />);
  await settle();
  await act(async () => {
    window.dispatchEvent(new Event("focus"));
  });
  expect(receiptSeen).not.toHaveBeenCalled();

  fireEvent.keyDown(window, { key: "i", metaKey: true });
  await settle();
  expect(receiptSeen).toHaveBeenLastCalledWith(runId("/Users/you/code/api", "production"));
});
