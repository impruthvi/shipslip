import { render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import * as fixtures from "./fixtures.gen";
import { InspectorHead } from "./Inspector";
import { Sidebar } from "./Sidebar";
import { Slip } from "./Slip";
import type { EnvCellView, FlagKind, SlipView } from "./types";

vi.mock("./api", () => ({ api: { copyText: async () => {} } }));

const loaded = fixtures.watchResults;
if (loaded.kind !== "loaded") throw new Error("watch results not loaded");

const results: [string, FlagKind, string, boolean][] = [
  ["watchInterrupted", "logWatchInterrupted", "log watch interrupted · entries may be missing", true],
  ["watchUnreadable", "logUnreadable", "log could not be read", true],
  ["watchNoFile", "noLogFileYet", "no log file yet", false],
  ["watchStopped", "logWatchStoppedEarly", "log watch stopped early", false],
  ["watchSkipped", "logWatchSkippedInMaintenance", "log watch skipped · app left in maintenance", false],
];

function result(name: string): { receipt: Extract<SlipView, { kind: "receipt" }>; cell: EnvCellView } {
  const receipt = fixtures.slips[name];
  const cell = loaded.kind === "loaded"
    ? loaded.map.rows.find((row) => row.label === name)?.cells[0]
    : null;
  if (receipt.kind !== "receipt" || cell?.kind !== "present") throw new Error(`missing ${name}`);
  return { receipt, cell };
}

test.each(results)("%s renders the result with its earned tone", (name, kind, wording, attention) => {
  const { receipt, cell } = result(name);
  expect(receipt.flags.find((flag) => flag.kind === kind)?.bad).toBe(attention);
  expect(receipt.watch?.kind).toBe(kind);
  render(<Slip opened={receipt} cell={cell} label={name} env="staging" context={null} onView={vi.fn()} />);
  const flag = screen.getByText(wording, { selector: ".slip-flag .sentence" });
  expect(flag).toHaveClass(attention ? "tone-attention" : "muted");
  expect(flag).not.toHaveClass("tone-bad");
  const watch = document.querySelector(".slip-watch-status")!;
  expect(watch).toHaveTextContent(wording);
  expect(watch).toHaveClass(attention ? "tone-attention" : "muted");
});

test.each(results)("%s is amber or quiet in the inspector without parsing its wording", (name, kind, wording, attention) => {
  const { cell } = result(name);
  const flag = cell.flags.find((entry) => entry.kind === kind)!;
  render(<InspectorHead cell={{ ...cell, flags: [{ ...flag, text: "different wording" }] }} label={name} env="staging" production={false} />);
  const line = screen.getByText("different wording").closest("li")!;
  expect(line).toHaveClass(attention ? "tone-attention" : "muted");
  expect(line).not.toHaveClass("tone-bad");
  expect(line.querySelector("svg") !== null).toBe(attention);
  expect(flag.text).toBe(wording);
});

test("only interrupted and unreadable watches enter Needs attention", () => {
  render(<Sidebar
    state={{ phase: "shown", loaded, readAt: new Date(0), failure: null }}
    busy={false} onRetry={vi.fn()} runs={[]} now={0} onFollow={vi.fn()}
    runsButton={{ current: null }} onProblem={vi.fn()} live={{}}
  />);
  const section = screen.getByRole("heading", { name: /^Needs attention/ }).closest("section")!;
  const items = within(section).getAllByRole("listitem");
  expect(items).toHaveLength(2);
  expect(items[0]).toHaveTextContent("watchInterrupted");
  expect(items[1]).toHaveTextContent("watchUnreadable");
  expect(within(section).queryByText("watchNoFile")).not.toBeInTheDocument();
  expect(within(section).queryByText("watchStopped")).not.toBeInTheDocument();
  expect(within(section).queryByText("watchSkipped")).not.toBeInTheDocument();
});
