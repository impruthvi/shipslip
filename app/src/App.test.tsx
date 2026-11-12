import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { within } from "@testing-library/react";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import type { LoadResult } from "./types";

const loadMap = vi.fn<() => Promise<LoadResult>>();
const receiptsChanged = vi.fn<() => Promise<boolean>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    receiptsChanged: () => receiptsChanged(),
    compare: vi.fn(),
    loadReceipt: async () => ({ kind: "notFound" }),
    copyText: vi.fn(),
    onFocus: async (handler: () => void) => {
      window.addEventListener("focus", handler);
      return () => window.removeEventListener("focus", handler);
    },
    receiptSeen: async () => {},
  },
}));

beforeEach(() => {
  loadMap.mockReset();
  receiptsChanged.mockReset().mockResolvedValue(false);
  localStorage.clear();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function settle() {
  await act(async () => {});
}

test("first load says it is reading, then shows the map", async () => {
  let resolve!: (value: LoadResult) => void;
  loadMap.mockReturnValueOnce(new Promise((r) => (resolve = r)));
  render(<App />);
  expect(screen.getByText("Reading local receipts…")).toBeInTheDocument();
  expect(screen.getByText("Not read yet")).toBeInTheDocument();
  await act(async () => resolve(fixtures.map));
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(screen.getByText("Read from local receipts")).toBeInTheDocument();
  expect(screen.getByText("1 receipt could not be read")).toBeInTheDocument();
});

test("window focus and ⌘R read again; a failed refresh keeps the map", async () => {
  loadMap.mockResolvedValue(fixtures.map);
  render(<App />);
  await settle();
  expect(loadMap).toHaveBeenCalledTimes(1);

  loadMap.mockResolvedValueOnce(fixtures.coalesced);
  fireEvent.focus(window);
  await settle();
  expect(loadMap).toHaveBeenCalledTimes(2);

  loadMap.mockResolvedValueOnce({ kind: "failed", message: "disk gone", current: null });
  const event = new KeyboardEvent("keydown", { key: "r", metaKey: true, cancelable: true });
  await act(async () => window.dispatchEvent(event));
  expect(event.defaultPrevented).toBe(true);
  expect(loadMap).toHaveBeenCalledTimes(3);
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(screen.getByText(/^Could not re-read at .* · showing /)).toBeInTheDocument();

  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  loadMap.mockResolvedValueOnce({ ...fixtures.map, generation: 2 });
  fireEvent.click(screen.getByRole("button", { name: "Retry ⌘R" }));
  await settle();
  expect(loadMap).toHaveBeenCalledTimes(4);
  expect(screen.queryByText(/Could not re-read/)).not.toBeInTheDocument();
});

test("a receipt change re-reads the map without changing window focus", async () => {
  vi.useFakeTimers();
  loadMap.mockResolvedValue(fixtures.firstRun);
  render(<App />);
  await settle();
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(receiptsChanged).toHaveBeenCalledTimes(1);
  expect(loadMap).toHaveBeenCalledTimes(1);

  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  loadMap.mockResolvedValueOnce({ ...fixtures.map, generation: 2 });
  receiptsChanged.mockResolvedValueOnce(true);
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(loadMap).toHaveBeenCalledTimes(2);
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
});

test("the footer names the last successful receipt read, not the last check", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 10, 4, 10, 42));
  loadMap.mockResolvedValue(fixtures.map);
  render(<App />);
  await settle();
  expect(screen.getByRole("contentinfo")).toHaveTextContent("Last read at 10:42");
  expect(screen.getByRole("contentinfo")).not.toHaveTextContent(/live|on window focus/i);

  vi.setSystemTime(new Date(2026, 10, 4, 10, 45));
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(screen.getByRole("contentinfo")).toHaveTextContent("Last read at 10:42");

  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  loadMap.mockResolvedValueOnce({ ...fixtures.map, generation: 2 });
  receiptsChanged.mockResolvedValueOnce(true);
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(screen.getByRole("contentinfo")).toHaveTextContent("Last read at 10:45");
});

test.each([false, true])("receipt checks pause while hidden (starts hidden: %s)", async (startsHidden) => {
  vi.useFakeTimers();
  let hidden = startsHidden;
  vi.spyOn(document, "hidden", "get").mockImplementation(() => hidden);
  loadMap.mockResolvedValue(fixtures.map);
  render(<App />);
  await settle();

  hidden = true;
  fireEvent(document, new Event("visibilitychange"));
  await act(async () => vi.advanceTimersByTimeAsync(9000));
  expect(receiptsChanged).not.toHaveBeenCalled();
  expect(loadMap).toHaveBeenCalledTimes(1);

  hidden = false;
  fireEvent(document, new Event("visibilitychange"));
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(receiptsChanged).toHaveBeenCalledTimes(1);
});

test("receipt checks do not overlap, and stop after unmount", async () => {
  vi.useFakeTimers();
  let resolve!: (changed: boolean) => void;
  receiptsChanged.mockReturnValueOnce(new Promise((done) => (resolve = done)));
  loadMap.mockResolvedValue(fixtures.map);
  const { unmount } = render(<App />);
  await settle();
  await act(async () => vi.advanceTimersByTimeAsync(9000));
  expect(receiptsChanged).toHaveBeenCalledTimes(1);

  unmount();
  await act(async () => resolve(true));
  await act(async () => vi.advanceTimersByTimeAsync(9000));
  expect(loadMap).toHaveBeenCalledTimes(1);
  expect(receiptsChanged).toHaveBeenCalledTimes(1);
});

test("a receipt check failure keeps the recorded map and read time, then recovers", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 10, 4, 10, 42));
  loadMap.mockResolvedValue(fixtures.map);
  render(<App />);
  await settle();

  vi.setSystemTime(new Date(2026, 10, 4, 10, 45));
  receiptsChanged.mockRejectedValueOnce(new Error("disk unavailable"));
  loadMap.mockResolvedValueOnce({ kind: "failed", message: "disk unavailable", current: null });
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(screen.getByRole("contentinfo")).toHaveTextContent("Could not re-read at 10:45 · showing 10:42");

  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  receiptsChanged.mockResolvedValueOnce(true);
  loadMap.mockResolvedValueOnce({ ...fixtures.map, generation: 2 });
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(screen.getByRole("contentinfo")).toHaveTextContent("Last read at 10:45");
  expect(screen.queryByText(/Could not re-read/)).not.toBeInTheDocument();
});

test("first run shows the two setup steps and no problems", async () => {
  loadMap.mockResolvedValue(fixtures.firstRun);
  render(<App />);
  await settle();
  expect(screen.getByText("No deploys or approvals recorded yet.")).toBeInTheDocument();
  expect(screen.queryByRole("grid")).not.toBeInTheDocument();
  expect(screen.queryByLabelText("Problems")).not.toBeInTheDocument();
});

test("unreadable receipts root is a banner with retry", async () => {
  loadMap.mockResolvedValue(fixtures.failed);
  render(<App />);
  await settle();
  expect(screen.getByRole("alert")).toHaveTextContent("Can't read the receipts folder");
  expect(screen.getByText("Not read yet")).toBeInTheDocument();
});

test("a rejected command is shown like a failed read", async () => {
  loadMap.mockRejectedValue("the request failed inside the app");
  render(<App />);
  await settle();
  expect(screen.getByRole("alert")).toHaveTextContent("the request failed inside the app");
});

test("receipts that exist but cannot be read list as problems over an empty grid", async () => {
  loadMap.mockResolvedValue(fixtures.noneReadable);
  render(<App />);
  await settle();
  expect(screen.getByLabelText("Problems")).toHaveTextContent("1 receipt could not be read");
  expect(screen.getByText("No readable receipts")).toBeInTheDocument();
});

test("launch selects the first cell that needs attention; clicks and problems select", async () => {
  loadMap.mockResolvedValue(fixtures.map);
  render(<App />);
  await settle();
  const selected = () => screen.getAllByRole("gridcell").filter((c) => c.getAttribute("aria-selected") === "true");
  expect(selected()).toHaveLength(1);
  expect(selected()[0]).toHaveTextContent("fffffff");

  const blogStaging = screen.getByRole("gridcell", { name: /^blog, staging, no deploy receipt on this Mac/ });
  fireEvent.click(blogStaging);
  expect(selected()).toEqual([blogStaging]);
  expect(JSON.parse(localStorage.getItem("shipslip.selection")!)).toEqual({
    repoRoot: "/Users/you/code/blog",
    env: "staging",
  });

  fireEvent.click(within(screen.getByLabelText("Problems")).getByRole("button", { name: "Select billing · production" }));
  expect(selected()[0]).toHaveTextContent("ddddddd");

  const absent = screen.getAllByRole("gridcell").find((c) => c.getAttribute("aria-label")?.endsWith(", none"))!;
  fireEvent.click(within(screen.getByRole("navigation")).getByRole("button", { name: /^blog Not known production$/ }));
  expect(selected()[0]).toHaveAccessibleName(/^blog, production/);
  expect(selected()[0]).toHaveFocus();

  fireEvent.click(absent);
  expect(selected()[0]).toHaveAccessibleName(/^blog, production/);
  expect(absent).not.toHaveAttribute("aria-selected");
});

test("the app opens on All projects, and choosing it again moves focus to its heading", async () => {
  loadMap.mockResolvedValue(fixtures.map);
  render(<App />);
  await settle();
  const all = screen.getByRole("button", { name: "All projects" });
  expect(all).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  const heading = screen.getByRole("heading", { level: 1, name: "All projects" });
  expect(heading).not.toHaveFocus();
  fireEvent.click(all);
  expect(heading).toHaveFocus();
});
