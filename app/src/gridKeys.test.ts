import { describe, expect, test } from "vitest";
import * as fixtures from "./fixtures.gen";
import { first, move } from "./gridKeys";

const map = fixtures.map.kind === "loaded" ? fixtures.map.map : (null as never);
const row = (repoRoot: string) => map.rows.findIndex((r) => r.repoRoot === repoRoot);
const col = (env: string) => map.envs.indexOf(env);

const API = row("/Users/you/code/api");
const BLOG = row("/Users/you/code/blog");
const COPY = row("/Users/you/tmp/shop-copy");
const SHOP = row("/Users/you/code/shop");
const PROD = col("production");
const STAGING = col("staging");

describe("arrow keys in the grid", () => {
  test("the fixture has an absent cell to skip", () => {
    expect(map.rows[COPY].cells[PROD].kind).toBe("absent");
    expect(PROD).toBeLessThan(STAGING);
    expect(COPY).toBe(BLOG + 1);
  });

  test("left and right stay in the row and skip absent cells", () => {
    expect(move(map, { row: API, col: PROD }, "ArrowRight")).toEqual({ row: API, col: STAGING });
    expect(move(map, { row: API, col: STAGING }, "ArrowLeft")).toEqual({ row: API, col: PROD });
    expect(move(map, { row: COPY, col: STAGING }, "ArrowLeft")).toBeNull();
    expect(move(map, { row: API, col: STAGING }, "ArrowRight")).toBeNull();
  });

  test("up and down stay in the column and skip absent cells", () => {
    expect(move(map, { row: BLOG, col: PROD }, "ArrowDown")).toEqual({ row: SHOP, col: PROD });
    expect(move(map, { row: SHOP, col: PROD }, "ArrowUp")).toEqual({ row: BLOG, col: PROD });
    expect(move(map, { row: BLOG, col: STAGING }, "ArrowDown")).toEqual({ row: COPY, col: STAGING });
    expect(move(map, { row: API, col: PROD }, "ArrowUp")).toBeNull();
  });

  test("Home and End go to the ends of the row; with Ctrl or ⌘, of the grid", () => {
    expect(move(map, { row: API, col: STAGING }, "Home")).toEqual({ row: API, col: PROD });
    expect(move(map, { row: API, col: PROD }, "End")).toEqual({ row: API, col: STAGING });
    expect(move(map, { row: COPY, col: STAGING }, "Home")).toEqual({ row: COPY, col: STAGING });
    expect(move(map, { row: SHOP, col: STAGING }, "Home", true)).toEqual(first(map));
    expect(move(map, { row: API, col: PROD }, "End", true)).toEqual({ row: map.rows.length - 1, col: STAGING });
  });

  test("other keys are not grid keys", () => {
    expect(move(map, { row: API, col: PROD }, "a")).toBeNull();
    expect(move(map, { row: API, col: PROD }, "Tab")).toBeNull();
  });
});
