// The only module that talks to Rust. Components call `api`; tests mock it.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { CompareResult, LoadResult, QuitRun, ReceiptResult } from "./types";

export interface Backend {
  loadMap(): Promise<LoadResult>;
  compare(generation: number, row: number, a: number, b: number): Promise<CompareResult>;
  loadReceipt(generation: number, id: number): Promise<ReceiptResult>;
  copyText(text: string): Promise<void>;
  /** Calls `handler` each time the window gains focus; resolves to an unsubscribe. */
  onFocus(handler: () => void): Promise<() => void>;
  /** The pane shows this run's receipt. Rust counts it as seen only while
   * the window is focused, which clears the run's Dock badge. */
  receiptSeen(runId: string): Promise<void>;
}

const tauri: Backend = {
  loadMap: () => invoke<LoadResult>("load_map"),
  compare: (generation, row, a, b) => invoke<CompareResult>("compare", { generation, row, a, b }),
  loadReceipt: (generation, id) => invoke<ReceiptResult>("load_receipt", { generation, id }),
  copyText: (text) => writeText(text),
  onFocus: (handler) =>
    getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) handler();
    }),
  receiptSeen: (runId) => invoke("receipt_seen", { runId }),
};

let backend: Promise<Backend> | undefined;

// A plain browser (vite dev without Tauri) gets fixture data instead.
const inBrowser = () => import.meta.env.DEV && !("__TAURI_INTERNALS__" in window);

function getBackend(): Promise<Backend> {
  if (inBrowser()) {
    backend ??= import("./preview").then((preview) => preview.backend);
  }
  backend ??= Promise.resolve(tauri);
  return backend;
}

export const api: Backend = {
  loadMap: async () => (await getBackend()).loadMap(),
  compare: async (...args) => (await getBackend()).compare(...args),
  loadReceipt: async (...args) => (await getBackend()).loadReceipt(...args),
  copyText: async (text) => (await getBackend()).copyText(text),
  onFocus: async (handler) => (await getBackend()).onFocus(handler),
  receiptSeen: async (runId) => (await getBackend()).receiptSeen(runId),
};

/** Quitting while a run is live. Separate from `Backend` so screens need not mock it. */
export interface QuitBackend {
  /** Calls `handler` when Quit, Cmd-Q or the Dock asks to quit during a run. */
  onRequest(handler: () => void): Promise<() => void>;
  status(): Promise<QuitRun[]>;
  /** Brings each run to a safe point, then exits the app. */
  detachAndQuit(): Promise<void>;
  quitNow(): Promise<void>;
}

export const quit: QuitBackend = {
  onRequest: async (handler) =>
    inBrowser() ? () => {} : listen("quit-requested", () => handler()),
  status: () => invoke<QuitRun[]>("quit_status"),
  detachAndQuit: () => invoke("detach_and_quit"),
  quitNow: () => invoke("quit_now"),
};
