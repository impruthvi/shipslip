import { beforeEach, describe, expect, test } from "vitest";
import * as fixtures from "./fixtures.gen";
import { loadStored, resolveSelection, store, type Selection } from "./selection";
import type { MapView, RowView } from "./types";

const map: MapView = (() => {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  return fixtures.map.map;
})();

const key = (s: Selection | null) => s && `${s.repoRoot} ${s.env}`;
const rowIndex = (m: MapView, repoRoot: string) => m.rows.findIndex((r) => r.repoRoot === repoRoot);
const sel = (m: MapView, repoRoot: string, env: string): Selection => ({
  repoRoot,
  env,
  row: rowIndex(m, repoRoot),
  col: m.envs.indexOf(env),
});

beforeEach(() => localStorage.clear());

describe("on launch", () => {
  test("the remembered cell wins", () => {
    const stored = { repoRoot: "/Users/you/code/blog", env: "staging" };
    expect(key(resolveSelection(map, null, stored))).toBe("/Users/you/code/blog staging");
  });

  test("a remembered cell that is gone falls back to the first attention cell", () => {
    const stored = { repoRoot: "/nowhere", env: "staging" };
    expect(key(resolveSelection(map, null, stored))).toBe("/Users/you/code/api production");
  });

  test("rows whose checkout is gone never win the attention pick", () => {
    const gone = map.rows.find((r) => r.pathMissing)!;
    const onlyGone: MapView = {
      ...map,
      rows: [gone, ...map.rows.filter((r) => !r.pathMissing && r.repoRoot.endsWith("/shop"))],
    };
    // The gone row's cell needs attention; the live shop staging cell (NEW errors) is picked.
    expect(key(resolveSelection(onlyGone, null, null))).toBe("/Users/you/code/shop staging");
  });

  test("with nothing needing attention, the top-left cell", () => {
    const calm: MapView = {
      ...map,
      rows: map.rows.map((row) => ({
        ...row,
        cells: row.cells.map((c) => (c.kind === "present" ? { ...c, needsAttention: false } : c)),
      })),
    };
    expect(key(resolveSelection(calm, null, null))).toBe("/Users/you/code/api production");
  });

  test("an empty map selects nothing", () => {
    expect(resolveSelection({ ...map, rows: [] }, null, null)).toBeNull();
  });
});

describe("after a refresh", () => {
  test("the same cell stays selected even when rows move", () => {
    const prev = sel(map, "/Users/you/code/shop", "staging");
    const reordered = { ...map, rows: [...map.rows].reverse() };
    const next = resolveSelection(reordered, prev, null)!;
    expect(key(next)).toBe("/Users/you/code/shop staging");
    expect(next.row).toBe(rowIndex(reordered, "/Users/you/code/shop"));
  });

  test("a vanished cell gives way to the nearest cell in its row", () => {
    const prev = sel(map, "/Users/you/code/blog", "staging");
    const rows: RowView[] = map.rows.map((row) =>
      row.repoRoot === "/Users/you/code/blog"
        ? { ...row, cells: row.cells.map((c, i) => (i === prev.col ? { kind: "absent" } : c)) }
        : row,
    );
    expect(key(resolveSelection({ ...map, rows }, prev, null))).toBe("/Users/you/code/blog production");
  });

  test("a vanished row gives way to the nearest cell in its column, upward on a tie", () => {
    const prev = sel(map, "/Users/you/code/blog", "production");
    const rows = map.rows.filter((row) => row.repoRoot !== "/Users/you/code/blog");
    // Its old index now holds an absent cell; billing (above) and shop (below) tie.
    expect(key(resolveSelection({ ...map, rows }, prev, null))).toBe("/Users/you/code/billing production");
  });
});

test("the selection is remembered by checkout and env only", () => {
  store(sel(map, "/Users/you/code/blog", "staging"));
  expect(loadStored()).toEqual({ repoRoot: "/Users/you/code/blog", env: "staging" });
  localStorage.setItem("shipslip.selection", "{not json");
  expect(loadStored()).toBeNull();
});
