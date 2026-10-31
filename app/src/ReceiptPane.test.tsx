import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import { PANE_DEFAULT, PANE_MAX } from "./pane";
import type { LoadResult, ReceiptResult } from "./types";
import { SLOW_OPEN_MS } from "./useReceipt";

const loadMap = vi.fn<() => Promise<LoadResult>>();
const loadReceipt = vi.fn<(generation: number, id: number) => Promise<ReceiptResult>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    compare: async () => ({ kind: "notFound" }),
    loadReceipt: (generation: number, id: number) => loadReceipt(generation, id),
    copyText: async () => {},
    onFocus: async () => () => {},
    receiptSeen: async () => {},
  },
}));

const loaded = fixtures.map.kind === "loaded" ? fixtures.map : (null as never);
const map = loaded.map;

function at(repoRoot: string, env: string) {
  const row = map.rows.findIndex((r) => r.repoRoot === repoRoot);
  const col = map.envs.indexOf(env);
  const cell = map.rows[row].cells[col];
  if (cell.kind !== "present") throw new Error("absent");
  return { row, col, cell };
}

function gridcell(repoRoot: string, env: string) {
  const { row, col } = at(repoRoot, env);
  return within(screen.getAllByRole("row")[row + 1]).getAllByRole("gridcell")[col];
}

const fromFixtures = async (_generation: number, id: number) => fixtures.receipts[String(id)];

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
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
  loadReceipt.mockReset().mockImplementation(fromFixtures);
});
afterEach(() => vi.useRealTimers());

async function settle() {
  await act(async () => {});
}

const pane = () => screen.getByRole("complementary", { name: "Receipt" });

test("a failed latest attempt opens before the older success; other cells open their code run", async () => {
  render(<App />);
  await settle();
  const first = at("/Users/you/code/api", "production");
  expect(loadReceipt).toHaveBeenLastCalledWith(loaded.generation, first.cell.lastAttempt!.receipt);
  expect(within(pane()).getByText(/^Latest attempt · /)).toBeInTheDocument();
  expect(within(pane()).getByRole("tab", { name: "Latest attempt" })).toHaveAttribute("aria-selected", "true");

  fireEvent.click(gridcell("/Users/you/code/billing", "staging"));
  await settle();
  const billing = at("/Users/you/code/billing", "staging");
  expect(loadReceipt).toHaveBeenLastCalledWith(loaded.generation, billing.cell.receipt);
  expect(within(pane()).getByRole("heading", { name: "billing · staging" })).toBeInTheDocument();
  expect(within(pane()).queryByRole("tablist")).not.toBeInTheDocument();
});

test("tabs switch between the latest attempt and the recorded code; arrows move along them", async () => {
  render(<App />);
  await settle();
  const { cell } = at("/Users/you/code/api", "production");
  const tabs = within(within(pane()).getByRole("tablist", { name: "Receipts for this cell" }));
  expect(tabs.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Latest attempt", "Recorded code"]);
  expect(within(pane()).queryByRole("button", { name: "Open the receipt behind the recorded code" })).toBeNull();

  fireEvent.click(tabs.getByRole("tab", { name: "Recorded code" }));
  await settle();
  expect(loadReceipt).toHaveBeenLastCalledWith(loaded.generation, cell.receipt);
  expect(within(pane()).getByText("Receipt behind the recorded code")).toBeInTheDocument();
  expect(within(pane()).getByRole("tabpanel")).toHaveAccessibleName("Recorded code");

  fireEvent.keyDown(tabs.getByRole("tab", { name: "Recorded code" }), { key: "ArrowLeft" });
  await settle();
  expect(loadReceipt).toHaveBeenLastCalledWith(loaded.generation, cell.lastAttempt!.receipt);
  expect(tabs.getByRole("tab", { name: "Latest attempt" })).toHaveFocus();
});

test("an expanded error on one receipt does not stay expanded on the next", async () => {
  render(<App />);
  await settle();
  fireEvent.click(gridcell("/Users/you/code/shop", "staging"));
  await settle();
  const first = within(pane()).getAllByRole("button", { expanded: false })[0];
  fireEvent.click(first);
  expect(first).toHaveAttribute("aria-expanded", "true");
  const other = deferred<ReceiptResult>();
  loadReceipt.mockReturnValueOnce(other.promise);
  fireEvent.click(gridcell("/Users/you/code/billing", "staging"));
  const shop = await fromFixtures(1, at("/Users/you/code/shop", "staging").cell.receipt!);
  if (shop.kind !== "opened") throw new Error("fixture");
  const slip = shop.slip;
  if (slip.kind !== "receipt") throw new Error("fixture");
  await act(async () => other.resolve({ ...shop, slip: { ...slip, project: "billing" } }));
  expect(within(pane()).getByRole("heading", { name: /^billing/ })).toBeInTheDocument();
  expect(within(pane()).getAllByRole("button", { expanded: false }).length).toBeGreaterThan(0);
  expect(within(pane()).queryByRole("button", { expanded: true })).not.toBeInTheDocument();
});

test("a late answer for an earlier cell is dropped", async () => {
  render(<App />);
  await settle();
  const billing = deferred<ReceiptResult>();
  const shop = deferred<ReceiptResult>();
  loadReceipt.mockReturnValueOnce(billing.promise).mockReturnValueOnce(shop.promise);
  fireEvent.click(gridcell("/Users/you/code/billing", "staging"));
  fireEvent.click(gridcell("/Users/you/code/shop", "staging"));
  await act(async () => shop.resolve(await fromFixtures(1, at("/Users/you/code/shop", "staging").cell.receipt!)));
  await act(async () =>
    billing.resolve(await fromFixtures(1, at("/Users/you/code/billing", "staging").cell.receipt!)),
  );
  expect(within(pane()).getByRole("heading", { name: "shop · staging" })).toBeInTheDocument();
});

test("a slow read dims the old slip after 150 ms and names the new one; no flash before", async () => {
  render(<App />);
  await settle();
  vi.useFakeTimers();
  const slow = deferred<ReceiptResult>();
  loadReceipt.mockReturnValueOnce(slow.promise);
  fireEvent.click(gridcell("/Users/you/code/billing", "staging"));
  act(() => vi.advanceTimersByTime(SLOW_OPEN_MS - 1));
  expect(screen.queryByText(/^Opening receipt/)).not.toBeInTheDocument();
  act(() => vi.advanceTimersByTime(1));
  expect(screen.getByText("Opening receipt for billing · staging…")).toBeInTheDocument();
  expect(within(pane()).getByRole("heading", { name: "api · production" }).closest(".pane-slip")).toHaveClass(
    "pane-slip-dim",
  );
  await act(async () => slow.resolve(await fromFixtures(1, at("/Users/you/code/billing", "staging").cell.receipt!)));
  expect(screen.queryByText(/^Opening receipt/)).not.toBeInTheDocument();
  expect(within(pane()).getByRole("heading", { name: "billing · staging" }).closest(".pane-slip")).not.toHaveClass(
    "pane-slip-dim",
  );
});

test("an answer inside 150 ms never shows the opening line", async () => {
  render(<App />);
  await settle();
  vi.useFakeTimers();
  const quick = deferred<ReceiptResult>();
  loadReceipt.mockReturnValueOnce(quick.promise);
  fireEvent.click(gridcell("/Users/you/code/billing", "staging"));
  act(() => vi.advanceTimersByTime(SLOW_OPEN_MS / 2));
  await act(async () => quick.resolve(await fromFixtures(1, at("/Users/you/code/billing", "staging").cell.receipt!)));
  act(() => vi.advanceTimersByTime(SLOW_OPEN_MS * 2));
  expect(screen.queryByText(/^Opening receipt/)).not.toBeInTheDocument();
});

test("a refresh re-reads the open receipt without dimming or blanking it", async () => {
  const second = { ...loaded, generation: loaded.generation + 1 };
  render(<App />);
  await settle();
  vi.useFakeTimers();
  const again = deferred<ReceiptResult>();
  loadReceipt.mockReturnValueOnce(again.promise);
  loadMap.mockResolvedValueOnce(second);
  const event = new KeyboardEvent("keydown", { key: "r", metaKey: true, cancelable: true });
  await act(async () => window.dispatchEvent(event));
  expect(loadReceipt).toHaveBeenLastCalledWith(second.generation, expect.any(Number));
  act(() => vi.advanceTimersByTime(SLOW_OPEN_MS * 2));
  expect(screen.queryByText(/^Opening receipt/)).not.toBeInTheDocument();
  expect(within(pane()).getByRole("heading", { name: "api · production" })).toBeInTheDocument();
});

test("a stale answer reads the map again, then opens from the new map", async () => {
  const second = { ...loaded, generation: loaded.generation + 1 };
  loadReceipt.mockResolvedValueOnce({ kind: "stale" });
  loadMap.mockResolvedValueOnce(fixtures.map).mockResolvedValueOnce(second);
  render(<App />);
  await settle();
  await settle();
  expect(loadMap).toHaveBeenCalledTimes(2);
  expect(loadReceipt).toHaveBeenLastCalledWith(second.generation, expect.any(Number));
});

test("below 1200 px the pane is a drawer: opens on selection, ⌘I and the toolbar toggle it", async () => {
  narrow = true;
  render(<App />);
  await settle();
  expect(screen.queryByRole("complementary", { name: "Receipt" })).not.toBeInTheDocument();
  const toggle = screen.getByRole("button", { name: /Receipt/ });
  expect(toggle).toHaveAttribute("aria-expanded", "false");

  fireEvent.click(gridcell("/Users/you/code/billing", "staging"));
  await settle();
  expect(pane()).toHaveClass("pane-drawer");
  expect(toggle).toHaveAttribute("aria-expanded", "true");

  const cmdI = new KeyboardEvent("keydown", { key: "i", metaKey: true, cancelable: true });
  act(() => window.dispatchEvent(cmdI));
  expect(cmdI.defaultPrevented).toBe(true);
  expect(screen.queryByRole("complementary", { name: "Receipt" })).not.toBeInTheDocument();
  fireEvent.click(toggle);
  expect(pane()).toBeInTheDocument();
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("complementary", { name: "Receipt" })).not.toBeInTheDocument();
});

test("the pane resizes within 360–560 px and remembers its width", async () => {
  render(<App />);
  await settle();
  const handle = screen.getByRole("separator", { name: "Resize receipt" });
  expect(handle).toHaveAttribute("aria-valuenow", String(PANE_DEFAULT));
  for (let i = 0; i < 20; i++) fireEvent.keyDown(handle, { key: "ArrowLeft" });
  expect(handle).toHaveAttribute("aria-valuenow", String(PANE_MAX));
  expect(pane()).toHaveStyle({ width: `${PANE_MAX}px` });
  expect(localStorage.getItem("shipslip.paneWidth")).toBe(String(PANE_MAX));

  fireEvent.pointerDown(handle, { clientX: 900 });
  fireEvent.pointerMove(handle, { clientX: 1000 });
  fireEvent.pointerUp(handle);
  expect(handle).toHaveAttribute("aria-valuenow", String(PANE_MAX - 100));
});
