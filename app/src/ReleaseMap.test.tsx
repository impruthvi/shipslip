import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import * as fixtures from "./fixtures.gen";
import { ReleaseMap, revealScroll } from "./ReleaseMap";
import type { MapView } from "./types";

const wide = fixtures.wideMap.kind === "loaded" ? fixtures.wideMap.map : (null as never);

// jsdom has no layout: a 150px sticky App column, then 200px env columns
// with an 8px gap, in a scroll box `viewWidth` wide.
const APP = 150;
const COL = 200;
const GAP = 8;
let viewWidth = 1200;
const scrolls = new WeakMap<Element, number>();
const original = {
  rect: Element.prototype.getBoundingClientRect,
  scrollLeft: Object.getOwnPropertyDescriptor(Element.prototype, "scrollLeft")!,
  clientWidth: Object.getOwnPropertyDescriptor(Element.prototype, "clientWidth")!,
};

const box = (left: number, width: number) => ({ left, width, right: left + width }) as DOMRect;

beforeEach(() => {
  viewWidth = 1200;
  Object.defineProperty(Element.prototype, "scrollLeft", {
    configurable: true,
    get(this: Element) {
      return scrolls.get(this) ?? 0;
    },
    set(this: Element, value: number) {
      scrolls.set(this, value);
    },
  });
  Object.defineProperty(Element.prototype, "clientWidth", {
    configurable: true,
    get(this: Element) {
      return this.classList.contains("grid-scroll") ? viewWidth : 0;
    },
  });
  Element.prototype.getBoundingClientRect = function (this: Element) {
    const scroller = this.closest(".grid-scroll");
    if (this === scroller) return box(0, viewWidth);
    if (this.classList.contains("grid-app")) return box(0, APP);
    const col = (this as HTMLElement).dataset.envCol;
    if (col === undefined) return box(0, 0);
    const scroll = scroller ? scroller.scrollLeft : 0;
    return box(APP + GAP + Number(col) * (COL + GAP) - scroll, COL);
  };
});

afterEach(() => {
  Element.prototype.getBoundingClientRect = original.rect;
  Object.defineProperty(Element.prototype, "scrollLeft", original.scrollLeft);
  Object.defineProperty(Element.prototype, "clientWidth", original.clientWidth);
});

const scroller = () => document.querySelector(".grid-scroll")!;
const left = (col: number) => APP + GAP + col * (COL + GAP);
const inView = (col: number) => {
  const s = scroller().scrollLeft;
  return left(col) >= s + APP && left(col) + COL <= s + viewWidth;
};
const compare = (a: string, b: string) => ({ pair: { a, b }, lines: {} });

test("columns run production, staging, then the rest by name", () => {
  render(<ReleaseMap map={wide} />);
  const heads = screen.getAllByRole("columnheader").map((head) => head.querySelector(".grid-head-env")?.textContent);
  expect(heads).toEqual([undefined, "production", "staging", "demo", "qa", "sandbox", "uat"]);
});

test("a compare scrolls both compared columns into view", () => {
  render(<ReleaseMap map={wide} compare={compare("staging", "uat")} />);
  const [a, b] = [wide.envs.indexOf("staging"), wide.envs.indexOf("uat")];
  expect(inView(a) && inView(b)).toBe(true);
});

test("a compare of columns left of the view scrolls back to them", () => {
  const { rerender } = render(<ReleaseMap map={wide} compare={compare("qa", "uat")} />);
  expect(scroller().scrollLeft).toBeGreaterThan(0);
  rerender(<ReleaseMap map={wide} compare={compare("production", "staging")} />);
  expect(inView(0) && inView(1)).toBe(true);
});

test("a new environment never hides the compare target", () => {
  const { rerender } = render(<ReleaseMap map={wide} compare={compare("staging", "sandbox")} />);
  // "beta" sorts in before sandbox and pushes it right.
  const at = wide.envs.indexOf("demo");
  const grown: MapView = {
    ...wide,
    envs: [...wide.envs.slice(0, at), "beta", ...wide.envs.slice(at)],
    rows: wide.rows.map((row) => ({
      ...row,
      cells: [...row.cells.slice(0, at), { kind: "absent" }, ...row.cells.slice(at)],
    })),
  };
  rerender(<ReleaseMap map={grown} compare={compare("staging", "sandbox")} />);
  expect(inView(grown.envs.indexOf("staging")) && inView(grown.envs.indexOf("sandbox"))).toBe(true);
});

test("a pair too far apart to fit shows the compare target", () => {
  viewWidth = 600;
  render(<ReleaseMap map={wide} compare={compare("uat", "production")} />);
  expect(inView(wide.envs.indexOf("production"))).toBe(true);
  render(<ReleaseMap map={wide} compare={compare("production", "uat")} />);
  const second = document.querySelectorAll(".grid-scroll")[1];
  const s = second.scrollLeft;
  const at = left(wide.envs.indexOf("uat"));
  expect(at >= s + APP && at + COL <= s + viewWidth).toBe(true);
});

test("a compare that already shows both columns does not scroll", () => {
  render(<ReleaseMap map={wide} compare={compare("production", "demo")} />);
  expect(scroller().scrollLeft).toBe(0);
});

test("revealScroll moves as little as it can, and shows the left edge when both cannot fit", () => {
  // scroll, view width, covered inset, span left, span right
  expect(revealScroll(0, 1000, 150, 200, 900)).toBe(0);
  expect(revealScroll(0, 1000, 150, 300, 1100)).toBe(100);
  expect(revealScroll(500, 1000, 150, 300, 900)).toBe(150);
  expect(revealScroll(0, 500, 150, 400, 1400)).toBe(250);
});
