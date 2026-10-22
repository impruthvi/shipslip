import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import QuitDialog from "./QuitDialog";
import { quitRuns } from "./fixtures.gen";
import type { QuitRun } from "./types";

let request: (() => void) | undefined;
const status = vi.fn<() => Promise<QuitRun[]>>();
const detachAndQuit = vi.fn<() => Promise<void>>();
const quitNow = vi.fn<() => Promise<void>>();
vi.mock("./api", () => ({
  quit: {
    onRequest: async (handler: () => void) => {
      request = handler;
      return () => {};
    },
    status: () => status(),
    detachAndQuit: () => detachAndQuit(),
    quitNow: () => quitNow(),
  },
}));

const deploying = quitRuns[0];

beforeEach(() => {
  request = undefined;
  status.mockReset().mockResolvedValue([deploying]);
  detachAndQuit.mockReset().mockResolvedValue();
  quitNow.mockReset().mockResolvedValue();
});

async function ask() {
  await act(async () => {});
  await act(async () => request!());
}

test("asks only when quit is requested, defaults to keep running, and Escape keeps running", async () => {
  render(<QuitDialog />);
  expect(screen.queryByRole("alertdialog")).toBeNull();
  await ask();
  expect(screen.getByRole("alertdialog", { name: "Quit while shop → production is deploying?" }));
  expect(screen.getByText(deploying.detach));
  expect(screen.getByText(deploying.quitNow, { exact: false }));
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Keep running" }));
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(detachAndQuit).not.toHaveBeenCalled();
  expect(quitNow).not.toHaveBeenCalled();
});

test("detach and quit waits safely and still offers quit now", async () => {
  render(<QuitDialog />);
  await ask();
  fireEvent.click(screen.getByRole("button", { name: "Detach and quit" }));
  expect(detachAndQuit).toHaveBeenCalledOnce();
  expect(screen.getByRole("alertdialog", { name: "Finishing safely…" }));
  expect(screen.queryByRole("button", { name: "Keep running" })).toBeNull();
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.getByRole("alertdialog"));
  fireEvent.click(screen.getByRole("button", { name: "Quit now" }));
  expect(quitNow).toHaveBeenCalledOnce();
});

test("a run that already finished just quits", async () => {
  status.mockResolvedValue([]);
  render(<QuitDialog />);
  await ask();
  expect(quitNow).toHaveBeenCalledOnce();
  expect(screen.queryByRole("alertdialog")).toBeNull();
});

test("names every live run when more than one would be cut short", async () => {
  status.mockResolvedValue(quitRuns);
  render(<QuitDialog />);
  await ask();
  expect(screen.getByRole("alertdialog", { name: "Quit while 2 runs are live?" }));
  for (const run of quitRuns) expect(screen.getByText(run.detach));
});
