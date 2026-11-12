// The only module that talks to Rust. Components call `api`; tests mock it.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { CheckoutReceipts, CompareResult, FolderView, LoadResult, QuitRun, ReceiptResult } from "./types";

export interface Backend {
  loadMap(): Promise<LoadResult>;
  receiptsChanged(): Promise<boolean>;
  compare(generation: number, row: number, a: number, b: number): Promise<CompareResult>;
  loadReceipt(generation: number, id: number): Promise<ReceiptResult>;
  /** A map row's own receipts, newest first; their ids open with `loadReceipt`. */
  checkoutReceipts(generation: number, row: number): Promise<CheckoutReceipts>;
  copyText(text: string): Promise<void>;
  /** Calls `handler` each time the window gains focus; resolves to an unsubscribe. */
  onFocus(handler: () => void): Promise<() => void>;
  /** The pane shows this run's receipt. Rust counts it as seen only while
   * the window is focused, which clears the run's Dock badge. */
  receiptSeen(runId: string): Promise<void>;
  /** Opens the macOS folder panel and reads the folder picked. */
  pickFolder(): Promise<FolderView>;
  /** Adds the picked folder's checkout to the projects list; resolves to its
   * path as the map spells it. Rejects with a `CommandError`. */
  addCheckout(): Promise<string>;
}

const tauri: Backend = {
  loadMap: () => invoke<LoadResult>("load_map"),
  receiptsChanged: () => invoke<boolean>("receipts_changed"),
  compare: (generation, row, a, b) => invoke<CompareResult>("compare", { generation, row, a, b }),
  loadReceipt: (generation, id) => invoke<ReceiptResult>("load_receipt", { generation, id }),
  checkoutReceipts: (generation, row) => invoke<CheckoutReceipts>("checkout_receipts", { generation, row }),
  copyText: (text) => writeText(text),
  onFocus: (handler) =>
    getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) handler();
    }),
  receiptSeen: (runId) => invoke("receipt_seen", { runId }),
  pickFolder: () => invoke<FolderView>("pick_folder"),
  addCheckout: () => invoke<string>("add_checkout"),
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
  receiptsChanged: async () => (await getBackend()).receiptsChanged(),
  compare: async (...args) => (await getBackend()).compare(...args),
  loadReceipt: async (...args) => (await getBackend()).loadReceipt(...args),
  checkoutReceipts: async (...args) => (await getBackend()).checkoutReceipts(...args),
  copyText: async (text) => (await getBackend()).copyText(text),
  onFocus: async (handler) => (await getBackend()).onFocus(handler),
  receiptSeen: async (runId) => (await getBackend()).receiptSeen(runId),
  pickFolder: async () => (await getBackend()).pickFolder(),
  addCheckout: async () => (await getBackend()).addCheckout(),
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
