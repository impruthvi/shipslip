// The only module that talks to Rust. Components call `api`; tests mock it.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { CompareResult, LoadResult, ReceiptResult } from "./types";

export interface Backend {
  loadMap(): Promise<LoadResult>;
  compare(generation: number, row: number, a: number, b: number): Promise<CompareResult>;
  loadReceipt(generation: number, id: number): Promise<ReceiptResult>;
  copyText(text: string): Promise<void>;
  /** Calls `handler` each time the window gains focus; resolves to an unsubscribe. */
  onFocus(handler: () => void): Promise<() => void>;
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
};

let backend: Promise<Backend> | undefined;

function getBackend(): Promise<Backend> {
  // A plain browser (vite dev without Tauri) gets fixture data instead.
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
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
};
