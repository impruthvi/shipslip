// Dev-only stand-in for Rust when the UI runs in a plain browser, serving
// src/fixtures.gen.ts. `?screen=firstRun|noneReadable|failed` picks the
// first load; `?slow=ms` delays every answer. `?deploy=failed` ends a
// deploy at its failed step instead of succeeding; `?recovery=failed` shows
// the recovery actions a failed run leaves; `?folder=` picks what Add
// project's folder panel returns.

import type { Backend } from "./api";
import type { DeployBackend } from "./deployApi";
import * as fixtures from "./fixtures.gen";
import type { CheckView, LoadResult } from "./types";

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
  receiptsChanged: async () => false,
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
  checkoutReceipts: (generation, row) =>
    later(
      generation === 1 ? (fixtures.checkoutReceipts[row] ?? { kind: "notFound" }) : { kind: "stale" },
    ),
  copyText: (text) => navigator.clipboard.writeText(text),
  onFocus: async (handler) => {
    window.addEventListener("focus", handler);
    return () => window.removeEventListener("focus", handler);
  },
  receiptSeen: async () => {},
  // `?folder=notGit|newConfig|unreadable` picks what the folder panel returns.
  pickFolder: () => later(fixtures.folders[params.get("folder") ?? "config"] ?? fixtures.folders.config),
  addCheckout: () => later("/Users/you/code/crm"),
  reopenSetup: () => later(fixtures.folders.newConfig),
  removeCheckout: async () => {},
  renderConfig: (answers) =>
    later(answers.env.trim() === "" ? fixtures.configTexts.invalid : fixtures.configTexts.rendered),
  addWithConfig: () => later({ checkout: "/Users/you/code/api", approvalError: null }),
  sshAliases: () => later(fixtures.sshAliases),
  hostBlock: (draft) =>
    later(
      draft.hostName.trim() === ""
        ? fixtures.hostBlocks.invalid
        : {
            kind: "block",
            lines: [`Host ${draft.alias}`, `  HostName ${draft.hostName}`, ...(draft.user ? [`  User ${draft.user}`] : [])],
          },
    ),
  // `?alias=matches|differs|failed` picks what Check again finds.
  checkAlias: () => later(fixtures.aliasChecks[params.get("alias") ?? "differs"] ?? fixtures.aliasChecks.differs),
  openSshConfig: async () => {},
  // `?check=pass|unknownKey|changedKey|wrongBranch` picks what the connection check finds.
  checkConnection: (_generation, _row, _env, onLines) => previewCheck(onLines),
  checkRendered: (onLines) => previewCheck(onLines),
};

async function previewCheck(onLines: (view: CheckView) => void): Promise<CheckView> {
  onLines(fixtures.connectionChecks.waiting);
  await wait(slow || 300);
  onLines(fixtures.connectionChecks.running);
  await wait(slow || 300);
  return fixtures.connectionChecks[params.get("check") ?? "pass"] ?? fixtures.connectionChecks.pass;
}

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
  openFolder: () => wait(100),
  approveTrust: () => wait(300),
  reviewUp: () => later(fixtures.upReview),
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
