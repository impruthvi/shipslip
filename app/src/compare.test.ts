import { beforeEach, expect, test } from "vitest";
import { loadPair, pairs, storePair } from "./compare";

beforeEach(() => localStorage.clear());

test("every ordered pair of envs", () => {
  expect(pairs(["production", "staging", "demo"]).map((p) => `${p.a}>${p.b}`)).toEqual([
    "production>staging",
    "production>demo",
    "staging>production",
    "staging>demo",
    "demo>production",
    "demo>staging",
  ]);
  expect(pairs(["staging"])).toEqual([]);
});

test("a remembered pair comes back only when both envs exist", () => {
  storePair({ a: "staging", b: "production" });
  expect(loadPair(["production", "staging"])).toEqual({ a: "staging", b: "production" });
  expect(loadPair(["staging", "demo"])).toBeNull();
  storePair(null);
  expect(loadPair(["production", "staging"])).toBeNull();
  localStorage.setItem("shipslip.compare", '{"a":"x","b":"x"}');
  expect(loadPair(["x"])).toBeNull();
  localStorage.setItem("shipslip.compare", "nope");
  expect(loadPair(["x"])).toBeNull();
});
