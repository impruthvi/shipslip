import { beforeEach, expect, test, vi } from "vitest";

const invoke = vi.fn();
const writeText = vi.fn();
const focusHandlers: Array<(event: { payload: boolean }) => void> = [];
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onFocusChanged: async (handler: (event: { payload: boolean }) => void) => {
      focusHandlers.push(handler);
      return () => {};
    },
  }),
}));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText }));

beforeEach(() => {
  vi.resetModules();
  invoke.mockReset().mockResolvedValue({ kind: "stale" });
  writeText.mockReset().mockResolvedValue(undefined);
  Object.assign(window, { __TAURI_INTERNALS__: {} });
});

test("commands get the argument names Rust expects", async () => {
  const { api } = await import("./api");
  await api.loadMap();
  await api.compare(3, 1, 0, 2);
  await api.loadReceipt(3, 7);
  expect(invoke.mock.calls).toEqual([
    ["load_map"],
    ["compare", { generation: 3, row: 1, a: 0, b: 2 }],
    ["load_receipt", { generation: 3, id: 7 }],
  ]);
});

test("copy goes through the clipboard plugin's write-text", async () => {
  const { api } = await import("./api");
  await api.copyText("cd '/work/app' && slip up staging");
  expect(writeText).toHaveBeenCalledWith("cd '/work/app' && slip up staging");
});

test("window focus comes from Tauri and only gaining focus counts", async () => {
  const { api } = await import("./api");
  const handler = vi.fn();
  await api.onFocus(handler);
  const listen = focusHandlers.at(-1)!;
  listen({ payload: false });
  expect(handler).not.toHaveBeenCalled();
  listen({ payload: true });
  expect(handler).toHaveBeenCalledTimes(1);
});
