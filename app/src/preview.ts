// Dev-only stand-in for Rust when the UI runs in a plain browser, serving
// src/fixtures.gen.ts. `?screen=firstRun|noneReadable|failed` picks the
// first load; `?slow=ms` delays every answer. `?deploy=failed` ends a
// deploy at its failed step instead of succeeding; `?recovery=failed` shows
// the recovery actions a failed run leaves.

import type { Backend } from "./api";
import type { DeployBackend } from "./deployApi";
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

const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

export const deploy: DeployBackend = {
  entry: () =>
    later(
      params.get("recovery") === "failed"
        ? { ...fixtures.deployEntries[3], recovery: fixtures.recoveries[0] }
        : fixtures.deployEntries[3],
    ),
  prepare: async (_generation, _row, _env, _plan, onProgress) => {
    for (const phase of fixtures.prepareProgress) {
      onProgress(phase);
      await wait(600);
    }
    return fixtures.deployReview;
  },
  reviewTrust: () => later(fixtures.trustReview),
  openConfig: () => wait(100),
  approveTrust: () => wait(300),
  reviewUp: () =>
    later({
      handle: "up-1",
      env: fixtures.runInfo.env,
      production: true,
      operation: "up",
      sshAlias: fixtures.runInfo.sshAlias,
      path: fixtures.runInfo.path,
      runId: null,
      targetSha: null,
      phase: "Bring this app out of maintenance mode",
      remainingSteps: [{ index: null, command: "php artisan up" }],
    }),
  reviewAttach: () => Promise.reject({ code: "notFound", message: "There is no unfinished run for this checkout and environment." }),
  confirm: () => later(fixtures.runInfo),
  cancel: () => wait(400),
  follow: async (_handle, onBatch) => {
    const end = params.get("deploy") === "failed" ? "failed" : "succeeded";
    const timers = [
      setTimeout(() => onBatch(fixtures.runBatches.running), 300),
      setTimeout(() => onBatch(fixtures.runBatches.watching), 4000),
      setTimeout(() => onBatch(fixtures.runBatches[end]), 9000),
    ];
    return () => timers.forEach(clearTimeout);
  },
  stop: () => wait(200),
  liveRuns: async () => ({ runs: [], closedPreviews: [] }),
};
