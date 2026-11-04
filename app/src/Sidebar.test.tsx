import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import * as fixtures from "./fixtures.gen";
import { Sidebar } from "./Sidebar";
import type { MapState } from "./useReleaseMap";

function props(busy: boolean) {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture not loaded");
  const state: MapState = { phase: "shown", loaded: fixtures.map, readAt: new Date(0), failure: null };
  return {
    state, busy, onRetry: vi.fn(), runs: [fixtures.runInfo], now: fixtures.runInfo.startedAtMs,
    onFollow: vi.fn(), runsButton: { current: null }, onProblem: vi.fn(), live: {},
  };
}

test("a deploy keeps the running and attention lists visible without navigation away from its review", () => {
  const current = props(true);
  render(<Sidebar {...current} />);
  const runs = screen.getByRole("heading", { name: "Deploys running" }).closest("section")!;
  expect(within(runs).getByRole("listitem")).toHaveTextContent(
    new RegExp(`^${fixtures.runInfo.project} ${fixtures.runInfo.env} · `),
  );
  const attention = screen.getByRole("heading", { name: /^Needs attention/ }).closest("section")!;
  expect(within(attention).getAllByRole("listitem").length).toBeGreaterThan(0);
  expect(within(runs).queryByRole("button")).toBeNull();
  expect(within(attention).queryByRole("button")).toBeNull();
  expect(current.onFollow).not.toHaveBeenCalled();
  expect(current.onProblem).not.toHaveBeenCalled();
});

test("the running list can return to a run when no review owns navigation", () => {
  const current = props(false);
  render(<Sidebar {...current} />);
  const runs = screen.getByRole("heading", { name: "Deploys running" }).closest("section")!;
  fireEvent.click(within(runs).getByRole("button"));
  expect(current.onFollow).toHaveBeenCalledWith(fixtures.runInfo);
});

test("the running row replaces its animation and run timer with the recorded connection gap", () => {
  const current = props(false);
  const run = fixtures.gapRunInfo;
  const { rerender } = render(<Sidebar {...current} runs={[run]} now={run.connectionGap!.lostAtMs + 12_000} />);
  const runs = screen.getByRole("region", { name: "Deploys running" });
  expect(within(runs).getByText(/Connection lost during step 1 at/)).toHaveClass("tone-attention");
  expect(runs).toHaveTextContent("Connection gap · 12s");
  expect(within(runs).getByRole("img", { hidden: true, name: "not known" })).toBeInTheDocument();
  expect(runs.querySelector(".mark-spin")).toBeNull();
  rerender(<Sidebar {...current} runs={[fixtures.runInfo]} />);
  expect(within(runs).queryByText(/Connection lost/)).toBeNull();
  expect(runs.querySelector(".mark-spin")).not.toBeNull();
});

test("a terminal run is listed only under Deploys running and opens its environment", () => {
  if (fixtures.terminalMap.kind !== "loaded") throw new Error("fixture");
  const current = props(false);
  const state: MapState = { phase: "shown", loaded: fixtures.terminalMap, readAt: new Date(0), failure: null };
  render(<Sidebar {...current} state={state} runs={[]} />);
  const runs = screen.getByRole("region", { name: "Deploys running" });
  expect(within(runs).getAllByRole("listitem")).toHaveLength(1);
  expect(runs).toHaveTextContent("shop");
  expect(runs).toHaveTextContent("staging · Running from the terminal");
  expect(within(screen.getByRole("region", { name: /^Needs attention/ })).queryByRole("listitem")).toBeNull();
  fireEvent.click(within(runs).getByRole("button"));
  expect(current.onProblem).toHaveBeenCalledWith(0, 0);
  expect(current.onFollow).not.toHaveBeenCalled();
});

test.each([false, true])("an app-owned running environment is not duplicated (terminal marker: %s)", (terminalMarker) => {
  if (fixtures.terminalMap.kind !== "loaded") throw new Error("fixture");
  const loaded = structuredClone(fixtures.terminalMap);
  const { map } = loaded;
  const row = map.rows[0];
  const env = map.envs[0];
  const cell = row.cells[0];
  if (cell.kind !== "present") throw new Error("fixture");
  if (!terminalMarker) cell.terminalRun = null;
  const run = { ...fixtures.runInfo, checkout: row.repoRoot, env, project: row.label };
  const state: MapState = { phase: "shown", loaded, readAt: new Date(0), failure: null };
  render(<Sidebar {...props(false)} state={state} runs={[run]} live={{ [`${row.repoRoot}\n${env}`]: "Step 1" }} />);
  const runs = screen.getByRole("region", { name: "Deploys running" });
  expect(within(runs).getAllByRole("listitem")).toHaveLength(1);
  expect(runs).not.toHaveTextContent("Running from the terminal");
  expect(within(screen.getByRole("region", { name: /^Needs attention/ })).queryByRole("listitem")).toBeNull();
});
