import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import type { CompareResult, LoadResult } from "./types";

const loadMap = vi.fn<() => Promise<LoadResult>>();
const compare = vi.fn<(g: number, row: number, a: number, b: number) => Promise<CompareResult>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    compare: (g: number, row: number, a: number, b: number) => compare(g, row, a, b),
    loadReceipt: async () => ({ kind: "notFound" }),
    copyText: vi.fn(),
    onFocus: async (handler: () => void) => {
      window.addEventListener("focus", handler);
      return () => window.removeEventListener("focus", handler);
    },
  },
}));

const map = fixtures.map.kind === "loaded" ? fixtures.map : (null as never);
const PROD = map.map.envs.indexOf("production");
const STAGING = map.map.envs.indexOf("staging");
const shopRow = map.map.rows.findIndex((r) => r.repoRoot === "/Users/you/code/shop");

beforeEach(() => {
  localStorage.clear();
  loadMap.mockReset().mockResolvedValue(fixtures.map);
  compare.mockReset();
  vi.useFakeTimers({ shouldAdvanceTime: true });
});

afterEach(() => vi.useRealTimers());

async function settle() {
  await act(async () => {});
}

function pick(value: string) {
  fireEvent.change(screen.getByLabelText("Compare"), { target: { value } });
}

const shopStaging = () =>
  screen
    .getAllByRole("row")
    .find((r) => within(r).queryByTitle("/Users/you/code/shop"))!
    .querySelectorAll('[role="gridcell"]')[STAGING];

test("the picker starts empty and shows no compare lines or markers", async () => {
  render(<App />);
  await settle();
  expect(screen.getByLabelText("Compare")).toHaveValue("");
  expect(document.querySelector(".cell-compare")).toBeNull();
  expect(screen.queryByRole("img", { name: "compared from" })).toBeNull();
  expect(compare).not.toHaveBeenCalled();
});

test("picking a pair compares every row; pending, slow, then the answer", async () => {
  const answers: Record<number, (r: CompareResult) => void> = {};
  compare.mockImplementation((_g, row) => new Promise((resolve) => (answers[row] = resolve)));
  render(<App />);
  await settle();
  pick("staging → production");
  await settle();
  expect(compare).toHaveBeenCalledTimes(map.map.rows.length);
  expect(compare).toHaveBeenCalledWith(map.generation, shopRow, STAGING, PROD);
  expect(shopStaging()).toHaveTextContent("comparing…");
  expect(screen.getAllByRole("img", { name: "compared from" })).toHaveLength(2);

  await act(async () => vi.advanceTimersByTime(5000));
  expect(shopStaging()).toHaveTextContent("still comparing (git is slow)");

  await act(async () => answers[shopRow](fixtures.compares[`${shopRow}:${STAGING}:${PROD}`]));
  expect(shopStaging()).toHaveTextContent("staging is 4 commits ahead of production");
  expect(shopStaging()).not.toHaveTextContent("still comparing");
  expect(JSON.parse(localStorage.getItem("shipslip.compare")!)).toEqual({ a: "staging", b: "production" });
});

test("answers for an old pair or a stale map are dropped", async () => {
  const answers: Array<{ row: number; a: number; resolve: (r: CompareResult) => void }> = [];
  compare.mockImplementation((_g, row, a) => new Promise((resolve) => answers.push({ row, a, resolve })));
  render(<App />);
  await settle();
  pick("staging → production");
  await settle();
  pick("production → staging");
  await settle();
  const old = answers.find((x) => x.row === shopRow && x.a === STAGING)!;
  await act(async () => old.resolve(fixtures.compares[`${shopRow}:${STAGING}:${PROD}`]));
  expect(document.body).not.toHaveTextContent("staging is 4 commits ahead of production");

  const current = answers.find((x) => x.row === shopRow && x.a === PROD)!;
  await act(async () => current.resolve({ kind: "stale" }));
  expect(document.body).not.toHaveTextContent("staging is 4 commits");
  expect(screen.getAllByText("comparing…").length).toBeGreaterThan(0);
});

test("clear removes the lines and forgets the pair", async () => {
  compare.mockImplementation((_g, row, a, b) => Promise.resolve(fixtures.compares[`${row}:${a}:${b}`]));
  render(<App />);
  await settle();
  pick("staging → production");
  await settle();
  expect(document.querySelectorAll(".cell-compare").length).toBeGreaterThan(0);
  fireEvent.click(screen.getByRole("button", { name: "Clear comparison" }));
  await settle();
  expect(document.querySelector(".cell-compare")).toBeNull();
  expect(screen.getByLabelText("Compare")).toHaveValue("");
  expect(localStorage.getItem("shipslip.compare")).toBeNull();
});

test("a remembered pair comes back on launch", async () => {
  localStorage.setItem("shipslip.compare", JSON.stringify({ a: "production", b: "staging" }));
  compare.mockImplementation((_g, row, a, b) => Promise.resolve(fixtures.compares[`${row}:${a}:${b}`]));
  render(<App />);
  await settle();
  await settle();
  expect(screen.getByLabelText("Compare")).toHaveValue("production → staging");
  expect(compare).toHaveBeenCalledWith(map.generation, shopRow, PROD, STAGING);
});

test("cannot compare is shown muted", async () => {
  compare.mockImplementation((_g, row, a, b) => Promise.resolve(fixtures.compares[`${row}:${a}:${b}`]));
  render(<App />);
  await settle();
  pick("staging → production");
  await settle();
  expect(screen.getByText("cannot compare: no recorded code on staging")).toHaveClass("muted");
});

test("a refresh keeps the answers it has while it asks again", async () => {
  compare.mockImplementation((_g, row, a, b) => Promise.resolve(fixtures.compares[`${row}:${a}:${b}`]));
  render(<App />);
  await settle();
  pick("staging → production");
  await settle();
  expect(shopStaging()).toHaveTextContent("staging is 4 commits ahead of production");

  compare.mockImplementation(() => new Promise(() => {}));
  loadMap.mockResolvedValueOnce({ ...map, generation: map.generation + 1 });
  fireEvent.focus(window);
  await settle();
  expect(compare).toHaveBeenCalledWith(map.generation + 1, shopRow, STAGING, PROD);
  expect(shopStaging()).toHaveTextContent("staging is 4 commits ahead of production");
  expect(shopStaging()).not.toHaveTextContent("comparing…");
});
