import { describe, expect, test } from "vitest";
import * as fixtures from "./fixtures.gen";
import type { LoadResult, Loaded } from "./types";
import { applyLoad, type MapState } from "./useReleaseMap";

const t1 = new Date(2026, 9, 12, 10, 42);
const t2 = new Date(2026, 9, 12, 10, 45);

function loaded(generation: number): LoadResult & Loaded {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  return { ...fixtures.map, generation };
}

const shown = (generation: number): MapState => ({
  phase: "shown",
  loaded: loaded(generation),
  readAt: t1,
  failure: null,
});

describe("applyLoad", () => {
  test("a first map is shown with its read time", () => {
    const next = applyLoad({ phase: "loading" }, loaded(1), t1);
    expect(next).toMatchObject({ phase: "shown", readAt: t1, failure: null });
  });

  test("a coalesced answer changes nothing", () => {
    const prev = shown(1);
    expect(applyLoad(prev, { kind: "coalesced" }, t2)).toBe(prev);
  });

  test("an older or same generation never replaces the shown map", () => {
    const prev = shown(3);
    expect(applyLoad(prev, loaded(2), t2)).toBe(prev);
    expect(applyLoad(prev, loaded(3), t2)).toBe(prev);
    expect(applyLoad(prev, loaded(4), t2)).toMatchObject({ readAt: t2 });
  });

  test("a failed refresh keeps the map and its read time", () => {
    const next = applyLoad(shown(1), { kind: "failed", message: "boom", current: null }, t2);
    expect(next).toMatchObject({ phase: "shown", readAt: t1, failure: { message: "boom", at: t2 } });
  });

  test("a good read clears an earlier failure", () => {
    const failed = applyLoad(shown(1), { kind: "failed", message: "boom", current: null }, t2);
    expect(applyLoad(failed, loaded(2), t2)).toMatchObject({ failure: null });
  });

  test("a first read that fails is the error screen", () => {
    expect(applyLoad({ phase: "loading" }, fixtures.failed, t1)).toEqual({
      phase: "error",
      message: fixtures.failed.kind === "failed" ? fixtures.failed.message : "",
    });
  });
});
