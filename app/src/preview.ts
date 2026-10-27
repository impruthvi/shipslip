// Dev-only stand-in for Rust when the UI runs in a plain browser, serving
// src/fixtures.gen.ts. `?screen=firstRun|noneReadable|failed` picks the
// first load; `?slow=ms` delays every answer.

import type { Backend } from "./api";
import * as fixtures from "./fixtures.gen";
import type { LoadResult } from "./types";

const params = new URLSearchParams(window.location.search);
const slow = Number(params.get("slow") ?? 0);
const screens: Record<string, LoadResult> = {
  firstRun: fixtures.firstRun,
  noneReadable: fixtures.noneReadable,
  failed: fixtures.failed,
};

const later = <T>(value: T) =>
  new Promise<T>((resolve) => setTimeout(() => resolve(value), slow));

export const backend: Backend = {
  loadMap: () => later(screens[params.get("screen") ?? ""] ?? fixtures.map),
  compare: (generation, row, a, b) =>
    later(
      generation === 1
        ? (fixtures.compares[`${row}:${a}:${b}`] ?? { kind: "notFound" })
        : { kind: "stale" },
    ),
  loadReceipt: (generation, id) =>
    later(
      generation === 1 ? (fixtures.receipts[String(id)] ?? { kind: "notFound" }) : { kind: "stale" },
    ),
  copyText: (text) => navigator.clipboard.writeText(text),
  onFocus: async (handler) => {
    window.addEventListener("focus", handler);
    return () => window.removeEventListener("focus", handler);
  },
  receiptSeen: async () => {},
};
