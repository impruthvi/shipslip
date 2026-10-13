import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { within } from "@testing-library/react";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import type { LoadResult } from "./types";

const loadMap = vi.fn<() => Promise<LoadResult>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    compare: vi.fn(),
    loadReceipt: async () => ({ kind: "notFound" }),
    copyText: vi.fn(),
    onFocus: async (handler: () => void) => {
      window.addEventListener("focus", handler);
      return () => window.removeEventListener("focus", handler);
    },
  },
}));

beforeEach(() => {
  loadMap.mockReset();
  localStorage.clear();
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

  const blogStaging = screen.getByText(/^no deploy receipt on this Mac$/i).closest('[role="gridcell"]')!;
  fireEvent.click(blogStaging);
  expect(selected()).toEqual([blogStaging]);
  expect(JSON.parse(localStorage.getItem("shipslip.selection")!)).toEqual({
    repoRoot: "/Users/you/code/blog",
    env: "staging",
  });

  fireEvent.click(within(screen.getByLabelText("Problems")).getByRole("button", { name: "Select cell" }));
  expect(selected()[0]).toHaveTextContent("ddddddd");

  const absent = screen.getAllByRole("gridcell").find((c) => c.childElementCount === 0)!;
  fireEvent.click(absent);
  expect(selected()[0]).toHaveTextContent("ddddddd");
  expect(absent).not.toHaveAttribute("aria-selected");
});
