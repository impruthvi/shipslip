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
    onFollow: vi.fn(), runsButton: { current: null }, onProblem: vi.fn(), live: {}, onAllProjects: vi.fn(),
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
  expect(within(within(attention).getByRole("list")).queryByRole("button")).toBeNull();
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

test("only the run timer is set in mono; the connection gap sentence is interface prose", () => {
  const current = props(false);
  const run = fixtures.gapRunInfo;
  const { rerender } = render(<Sidebar {...current} runs={[run]} now={run.connectionGap!.lostAtMs + 6_000} />);
  const runs = screen.getByRole("region", { name: "Deploys running" });
  expect(runs.querySelector(".side-meta")).toBeNull();
  rerender(<Sidebar {...current} runs={[fixtures.runInfo]} now={fixtures.runInfo.startedAtMs + 6_000} />);
  expect(runs.querySelector(".side-meta")).toHaveTextContent(/^6s$/);
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

function shown(loaded: typeof fixtures.map): MapState {
  if (loaded.kind !== "loaded") throw new Error("fixture not loaded");
  return { phase: "shown", loaded, readAt: new Date(0), failure: null };
}

test("the sidebar runs Add project, All projects, Deploys running, Needs attention, problems, then Projects", () => {
  render(<Sidebar {...props(false)} />);
  const nav = screen.getByRole("navigation", { name: "Sidebar" });
  const order = [
    screen.getByRole("button", { name: "Add project" }),
    screen.getByRole("button", { name: "All projects" }),
    screen.getByRole("region", { name: "Deploys running" }),
    screen.getByRole("region", { name: /^Needs attention/ }),
    screen.getByText("1 receipt could not be read"),
    screen.getByRole("region", { name: "Projects" }),
  ];
  for (const [before, after] of order.slice(0, -1).map((el, i) => [el, order[i + 1]])) {
    expect(nav).toContainElement(before);
    expect(before.compareDocumentPosition(after) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  }
  expect(screen.getByRole("button", { name: "All projects" })).toHaveAttribute("aria-current", "page");
});

test("All projects asks for its screen; a deploy screen keeps it from navigating", () => {
  const current = props(false);
  const onAllProjects = vi.fn();
  const { rerender } = render(<Sidebar {...current} onAllProjects={onAllProjects} />);
  fireEvent.click(screen.getByRole("button", { name: "All projects" }));
  expect(onAllProjects).toHaveBeenCalledOnce();
  rerender(<Sidebar {...current} busy onAllProjects={onAllProjects} />);
  expect(screen.queryByRole("button", { name: "All projects" })).toBeNull();
  expect(screen.getByText("All projects")).not.toHaveAttribute("aria-current");
});

test("Projects has one line per checkout, with the folder only on repeated names", () => {
  render(<Sidebar {...props(false)} />);
  const projects = screen.getByRole("region", { name: "Projects" });
  const lines = within(projects).getAllByRole("listitem");
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  expect(lines).toHaveLength(fixtures.map.map.rows.length);
  expect(lines.map((line) => line.querySelector(".side-name")?.textContent)).toEqual([
    "api",
    "billing",
    "blog",
    "shop · in ~/tmp",
    "shop · in ~/code",
  ]);
});

test("a checkout's marks keep their evidence tone; hovering one names its environment and status word", () => {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  const api = fixtures.map.map.rows[0];
  render(<Sidebar {...props(false)} live={{ [`${api.repoRoot}\nstaging`]: "Step 1" }} />);
  const projects = screen.getByRole("region", { name: "Projects" });
  const [apiLine, billingLine] = within(projects).getAllByRole("listitem");
  const marks = within(billingLine).getAllByRole("img");
  expect(marks.map((mark) => mark.getAttribute("title"))).toEqual([
    "production: Out of date?",
    "staging: Incomplete",
  ]);
  // The word is red on the map; the mark keeps the incomplete evidence tone.
  expect(within(billingLine).getByTitle("staging: Incomplete").querySelector("svg")).toHaveClass("mark-attention");
  // A run going now changes the word, never the mark.
  const running = within(apiLine).getByTitle("staging: Running");
  expect(running.querySelector("svg")).not.toHaveClass("mark-running");
  expect(projects.querySelector(".flap, .tone-bad, .mark-bad, .mark-running")).toBeNull();
});

test("a checkout whose config has an environment without a current approval reads Needs approval", () => {
  render(<Sidebar {...props(false)} state={shown(fixtures.approvalMap)} />);
  const projects = screen.getByRole("region", { name: "Projects" });
  const [docs, shop] = within(projects).getAllByRole("listitem");
  expect(docs).toHaveTextContent(/^docs\s*Needs approval$/);
  expect(within(docs).getByText("Needs approval")).toHaveClass("tone-attention");
  expect(within(docs).queryByRole("img")).toBeNull();
  expect(shop).not.toHaveTextContent("Needs approval");
  expect(within(shop).getAllByRole("img")).toHaveLength(2);
});

/** The fixture map with only its first `count` attention cells still needing attention. */
function attentionMap(count: number): MapState {
  const loaded = structuredClone(fixtures.map);
  if (loaded.kind !== "loaded") throw new Error("fixture not loaded");
  let kept = 0;
  for (const row of loaded.map.rows) {
    for (const cell of row.cells) {
      if (cell.kind !== "present" || !cell.needsAttention) continue;
      cell.needsAttention = kept < count;
      kept += 1;
    }
  }
  if (kept < count) throw new Error("fixture has too few attention cells");
  return shown(loaded);
}

function attentionNames(section: HTMLElement) {
  return within(within(section).getByRole("list")).getAllByRole("listitem").map((item) => item.textContent);
}

test("with 4 or fewer, Needs attention shows every item and no toggle", () => {
  render(<Sidebar {...props(false)} state={attentionMap(4)} />);
  const section = screen.getByRole("region", { name: /^Needs attention/ });
  expect(attentionNames(section)).toHaveLength(4);
  expect(within(section).queryByRole("button", { name: /^Show / })).toBeNull();
});

test("with 5 or more, Needs attention shows the 3 worst and toggles between Show all n and Show less", () => {
  render(<Sidebar {...props(false)} state={attentionMap(5)} />);
  const section = screen.getByRole("region", { name: /^Needs attention/ });
  const heading = within(section).getByRole("heading");
  expect(heading).toHaveTextContent(/^Needs attention5$/);
  const collapsed = attentionNames(section);
  expect(collapsed).toHaveLength(3);

  fireEvent.click(within(section).getByRole("button", { name: "Show all 5" }));
  const all = attentionNames(section);
  expect(all).toHaveLength(5);
  expect(all.slice(0, 3)).toEqual(collapsed);
  expect(heading).toHaveTextContent(/^Needs attention5$/);

  fireEvent.click(within(section).getByRole("button", { name: "Show less" }));
  expect(attentionNames(section)).toEqual(collapsed);
  expect(within(section).getByRole("button", { name: "Show all 5" })).toBeInTheDocument();
});

test("the Needs attention toggle is not kept across a reload", () => {
  const { unmount } = render(<Sidebar {...props(false)} state={attentionMap(5)} />);
  fireEvent.click(screen.getByRole("button", { name: "Show all 5" }));
  unmount();
  render(<Sidebar {...props(false)} state={attentionMap(5)} />);
  expect(attentionNames(screen.getByRole("region", { name: /^Needs attention/ }))).toHaveLength(3);
});

test("a deploy screen keeps the Needs attention toggle, which only changes what the list shows", () => {
  const current = props(true);
  render(<Sidebar {...current} state={attentionMap(5)} />);
  const section = screen.getByRole("region", { name: /^Needs attention/ });
  fireEvent.click(within(section).getByRole("button", { name: "Show all 5" }));
  expect(attentionNames(section)).toHaveLength(5);
  expect(current.onProblem).not.toHaveBeenCalled();
});
