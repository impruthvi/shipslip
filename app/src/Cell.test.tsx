import { render, screen, within } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import * as fixtures from "./fixtures.gen";
import { Cell, cellName, Flap, reason, status } from "./Cell";
import { ReleaseMap } from "./ReleaseMap";
import type { FlagView, MapView, OutcomeKind } from "./types";

const map: MapView = (() => {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  return fixtures.map.map;
})();

/** A cell in the selected row, which shows every line under its board line. */
function cell(repoRoot: string, env: string) {
  const row = map.rows.findIndex((r) => r.repoRoot === repoRoot);
  const col = map.envs.indexOf(env);
  render(<ReleaseMap map={map} selected={{ repoRoot, env, row, col }} />);
  return find(repoRoot, env);
}

function find(repoRoot: string, env: string) {
  const row = screen
    .getAllByRole("row")
    .find((r) => within(r).queryByTitle(repoRoot) !== null);
  if (!row) throw new Error(`no row ${repoRoot}`);
  return within(row).getAllByRole("gridcell")[map.envs.indexOf(env)];
}

describe("cells", () => {
  test("a run from step says which steps it ran, so it does not read as a full deploy", () => {
    const cell = fixtures.fromStepCell;
    if (cell.kind !== "present") throw new Error("fixture");
    expect(status(cell).word).toBe("Succeeded");
    expect(reason(cell)?.text).toBe("Steps 2–3 only · checked out at start");
    expect(cellName("shop", "staging", cell)).toContain("Succeeded · steps 2–3 only");
    render(<Cell cell={cell} expanded />);
    expect(screen.getByText("Steps 2–3 only · checked out at start")).toHaveClass("cell-reason");
    const lastFailed = {
      ...cell,
      lastAttempt: {
        receipt: 1,
        outcome: { text: "Failed at step 3", tone: "bad" as const, kind: "failed" as const },
        stepsRun: "step 3 only",
        flags: [],
        atMs: cell.atMs ?? 0,
        age: "1m ago",
      },
    };
    expect(reason(lastFailed)?.text).toBe("Last: failed at step 3 · step 3 only · 1m ago");
    expect(cellName("shop", "staging", lastFailed)).toContain("last attempt: failed at step 3, step 3 only, 1m ago");
  });

  test("a terminal-driven run shows Running and where to follow it", () => {
    if (fixtures.terminalMap.kind !== "loaded") throw new Error("fixture");
    const map = fixtures.terminalMap.map;
    const row = map.rows[0];
    render(<ReleaseMap map={map} selected={{ repoRoot: row.repoRoot, env: map.envs[0], row: 0, col: 0 }} />);
    expect(screen.getByText("Running")).toHaveClass("flap-running");
    expect(screen.getByText("Running from the terminal")).toBeInTheDocument();
    expect(document.querySelector(".mark-spin")).not.toBeNull();
  });

  test("a board line: mark, status word, 7-char commit, age", () => {
    const c = cell("/Users/you/code/shop", "production");
    expect(within(c).getByRole("img", { name: "recorded code" })).toBeInTheDocument();
    expect(within(c).getByText("Succeeded")).toHaveClass("flap");
    expect(within(c.querySelector(".cell-head")!).getByText("bbbbbbb")).toHaveClass("cell-sha");
    expect(c).toHaveTextContent("2d ago");
  });

  test("the selected row says why each cell reads as it does, in one line", () => {
    expect(cell("/Users/you/code/api", "production").querySelector(".cell-reason")).toHaveTextContent(
      "Last: stopped in maintenance mode, no steps ran · 20m ago",
    );
    expect(find("/Users/you/code/api", "staging").querySelector(".cell-reason")).toHaveTextContent(
      "observed after run 00000009 failed at step 1 · expected 777777777777 · tree dirty",
    );
  });

  test("the reason names the newest warning before the older outcome", () => {
    const c = cell("/Users/you/code/billing", "production");
    expect(c.querySelector(".cell-reason")).toHaveTextContent(
      "a newer receipt could not be read; this may be out of date",
    );
    expect(c.querySelector(".cell-reason")).toHaveClass("tone-attention");
  });

  test("incomplete deploy: half mark and a failure, never a success", () => {
    const c = cell("/Users/you/code/billing", "staging");
    expect(within(c).getByRole("img", { name: "deploy incomplete" })).toBeInTheDocument();
    expect(within(c).getByText("Failed at step 1 · deploy incomplete")).toHaveClass("tone-bad");
  });

  test("not known: hatched, no commit, the reason in attention amber, not failure red", () => {
    const c = cell("/Users/you/code/blog", "production");
    expect(c.querySelector(".cell")).toHaveClass("cell-hatched");
    expect(c.querySelector(".cell-sha")).toHaveTextContent("—");
    expect(within(c).getByText(/fast-forward failed/)).toHaveClass("tone-attention");
  });

  test("approved but never deployed here: dashed, muted, says approved before", () => {
    const c = cell("/Users/you/code/blog", "staging");
    expect(c.querySelector(".cell")).toHaveClass("cell-empty");
    expect(c).toHaveTextContent("No code");
    expect(c).toHaveTextContent("approved before");
  });

  test("an app without an environment gets a quiet dash, no cell", () => {
    const c = cell("/Users/you/tmp/shop-copy", "production");
    expect(c).toHaveTextContent("—");
    expect(c.querySelector(".cell")).toBeNull();
    expect(c).not.toHaveAttribute("tabindex");
  });

  test("a checkout that is gone says so under its path", () => {
    render(<ReleaseMap map={map} selected={{ repoRoot: "/Users/you/tmp/shop-copy", env: "staging", row: 3, col: 1 }} />);
    const header = screen.getByTitle("/Users/you/tmp/shop-copy").closest('[role="rowheader"]');
    expect(header).toHaveTextContent("checkout not found");
  });

  test("same-named apps show the folder that tells them apart", () => {
    render(<ReleaseMap map={map} />);
    expect(screen.getByTitle("/Users/you/tmp/shop-copy")).toHaveTextContent("in ~/tmp");
    expect(screen.getByTitle("/Users/you/code/shop")).toHaveTextContent("in ~/code");
    expect(screen.getByTitle("/Users/you/code/api")).toHaveTextContent("~/code/api");
  });

  test("the key names every mark", () => {
    render(<ReleaseMap map={map} />);
    const key = screen.getByRole("list", { name: "Key" });
    for (const name of ["recorded code", "deploy incomplete", "observed after a failed step", "not known", "no code recorded"]) {
      expect(within(key).getByRole("img", { name })).toBeInTheDocument();
    }
  });
});

test.each([
  ["passed", "Succeeded", "ink"],
  ["failed", "Smoke failed", "bad"],
  ["upOnlyFailed", "Smoke failed", "bad"],
])("the newest Up smoke result %s earns its status colour", (fixture, word, tone) => {
  const cell = fixtures.smokeCells[fixture];
  if (cell.kind !== "present") throw new Error("fixture");
  render(<Flap status={status(cell)} />);
  expect(screen.getByText(word)).toHaveClass(`flap-${tone}`);
});

describe("board status word", () => {
  const at = (repoRoot: string, env: string) => {
    const found = map.rows.find((r) => r.repoRoot === repoRoot)?.cells[map.envs.indexOf(env)];
    if (found?.kind !== "present") throw new Error("fixture");
    return found;
  };

  test.each([
    ["/Users/you/code/api", "production", "Maintenance?", "attention"],
    ["/Users/you/code/api", "staging", "Failed", "bad"],
    ["/Users/you/code/billing", "production", "Out of date?", "attention"],
    ["/Users/you/code/billing", "staging", "Incomplete", "bad"],
    ["/Users/you/code/blog", "production", "Not known", "attention"],
    ["/Users/you/code/blog", "staging", "No code", "muted"],
    ["/Users/you/code/shop", "production", "Succeeded", "ink"],
    ["/Users/you/code/shop", "staging", "New errors", "bad"],
  ])("%s %s reads %s", (repoRoot, env, word, tone) => {
    expect(status(at(repoRoot, env))).toEqual({ word, tone });
  });

  test("a newer risky attempt outranks the older success it sits on", () => {
    const api = at("/Users/you/code/api", "production");
    expect(api.outcome?.tone).toBe("good");
    expect(status(api).word).toBe("Maintenance?");
  });

  test("red is kept for a step that failed; stops and lost results are amber", () => {
    const api = at("/Users/you/code/api", "production");
    const last = (kind: OutcomeKind, flags: FlagView[] = []) =>
      status({ ...api, lastAttempt: { ...api.lastAttempt!, outcome: { text: "x", tone: "bad", kind }, flags } });
    expect(last("failed")).toEqual({ word: "Last failed", tone: "bad" });
    expect(last("failed", api.lastAttempt!.flags)).toEqual({ word: "Last failed", tone: "bad" });
    expect(last("stopped")).toEqual({ word: "Last stopped", tone: "attention" });
    expect(last("unknown")).toEqual({ word: "Last unknown", tone: "attention" });
    expect(last("stoppedInMaintenance")).toEqual({ word: "Maintenance?", tone: "attention" });
  });

  test("a saved unknown result is not described as an unfinished or running deploy", () => {
    const api = at("/Users/you/code/api", "production");
    expect(status({
      ...api, lastAttempt: null, flags: [],
      outcome: { text: "Unknown result", tone: "bad", kind: "unknown" },
    })).toEqual({ word: "Not known", tone: "attention" });
  });

  test("a run going now reads Running whatever came before", () => {
    expect(status(at("/Users/you/code/blog", "production"), "Deploying… 4s")).toEqual({
      word: "Running",
      tone: "running",
    });
  });

  test("rows other than the selected one keep to the board line", () => {
    render(<ReleaseMap map={map} />);
    const c = find("/Users/you/code/shop", "staging");
    expect(c).toHaveTextContent("New errors");
    expect(c).toHaveTextContent("aaaaaaa");
    expect(c.querySelector(".cell-reason")).toBeNull();
    expect(c.getAttribute("aria-label")).toContain("2 new error groups after this run");
  });
});

describe("flap", () => {
  test("turns only when its word changes", () => {
    const view = render(<Flap status={{ word: "Succeeded", tone: "ink" }} />);
    expect(screen.getByText("Succeeded")).not.toHaveClass("flap-turn");
    view.rerender(<Flap status={{ word: "Succeeded", tone: "ink" }} />);
    expect(screen.getByText("Succeeded")).not.toHaveClass("flap-turn");
    view.rerender(<Flap status={{ word: "Running", tone: "running" }} />);
    expect(screen.getByText("Running")).toHaveClass("flap", "flap-running", "flap-turn");
  });
});
