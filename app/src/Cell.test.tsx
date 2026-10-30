import { render, screen, within } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import * as fixtures from "./fixtures.gen";
import { Flap, status } from "./Cell";
import { ReleaseMap } from "./ReleaseMap";
import type { MapView } from "./types";

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
  test("recorded code: solid mark, 7-char commit, age, outcome, target", () => {
    const c = cell("/Users/you/code/shop", "production");
    expect(within(c).getByRole("img", { name: "recorded code" })).toBeInTheDocument();
    expect(c).toHaveTextContent("bbbbbbb");
    expect(c).toHaveTextContent("2d ago");
    expect(within(c).getByText("Succeeded")).toHaveClass("flap");
    expect(c).toHaveTextContent("production1:/var/www/shop");
  });

  test("the newest attempt sits below the tear line only when it is another run", () => {
    expect(cell("/Users/you/code/shop", "production")).toHaveTextContent(
      "Last: cancelled before changes · 45m ago",
    );
  });

  test("NEW errors get the chip and the library words", () => {
    const c = cell("/Users/you/code/shop", "staging");
    expect(within(c).getByText("NEW")).toHaveClass("chip");
    expect(c).toHaveTextContent("2 new error groups after this run");
  });

  test("incomplete deploy: half mark and a failure, never a success", () => {
    const c = cell("/Users/you/code/billing", "staging");
    expect(within(c).getByRole("img", { name: "deploy incomplete" })).toBeInTheDocument();
    expect(within(c).getByText("Failed at step 1 · deploy incomplete")).toHaveClass("tone-bad");
  });

  test("an unreadable newer receipt warns right under the commit", () => {
    const c = cell("/Users/you/code/billing", "production");
    const lines = Array.from(c.querySelectorAll(".cell-line")).map((l) => l.textContent);
    expect(lines[0]).toBe("a newer receipt could not be read; this may be out of date");
    expect(lines[1]).toBe("Succeeded");
  });

  test("observed: ring mark and what was seen after which run", () => {
    const c = cell("/Users/you/code/api", "staging");
    expect(within(c).getByRole("img", { name: "observed after a failed step" })).toBeInTheDocument();
    expect(c).toHaveTextContent("observed after run 00000009 failed at step 1 · expected 777777777777 · tree dirty");
  });

  test("moved target and a maintenance-mode last attempt are both warned", () => {
    const c = cell("/Users/you/code/api", "production");
    expect(c).toHaveTextContent("approved config now deploys to production2:/var/www/api, not this target");
    expect(c).toHaveTextContent("Last: stopped in maintenance mode, no steps ran · 20m ago");
    expect(within(c).getByText("app may be in maintenance mode")).toHaveClass("sentence");
  });

  test("not known: hatched, no commit, the reason in attention amber, not failure red", () => {
    const c = cell("/Users/you/code/blog", "production");
    expect(c.querySelector(".cell")).toHaveClass("cell-hatched");
    expect(c).toHaveTextContent("server code not known");
    expect(c.querySelector(".cell-sha")).toBeNull();
    expect(within(c).getByText(/fast-forward failed/)).toHaveClass("tone-attention");
  });

  test("approved but never deployed here: dashed, muted, says approved before", () => {
    const c = cell("/Users/you/code/blog", "staging");
    expect(c.querySelector(".cell")).toHaveClass("cell-empty");
    expect(c).toHaveTextContent("no deploy receipt on this Mac");
    expect(c).toHaveTextContent("approved before");
  });

  test("an app without an environment gets an empty, unoutlined cell", () => {
    const c = cell("/Users/you/tmp/shop-copy", "production");
    expect(c).toBeEmptyDOMElement();
  });

  test("a checkout that is gone says so under its path", () => {
    render(<ReleaseMap map={map} />);
    const header = screen.getByTitle("/Users/you/tmp/shop-copy").closest('[role="rowheader"]');
    expect(header).toHaveTextContent("~/tmp/shop-copy");
    expect(header).toHaveTextContent("checkout not found");
  });

  test("the key names every mark", () => {
    render(<ReleaseMap map={map} />);
    const key = screen.getByRole("list", { name: "Key" });
    for (const name of ["recorded code", "deploy incomplete", "observed after a failed step", "not known", "no code recorded"]) {
      expect(within(key).getByRole("img", { name })).toBeInTheDocument();
    }
  });
});

describe("board status word", () => {
  const at = (repoRoot: string, env: string) => {
    const found = map.rows.find((r) => r.repoRoot === repoRoot)?.cells[map.envs.indexOf(env)];
    if (found?.kind !== "present") throw new Error("fixture");
    return found;
  };

  test.each([
    ["/Users/you/code/api", "production", "Last failed", "bad"],
    ["/Users/you/code/api", "staging", "Failed", "bad"],
    ["/Users/you/code/billing", "production", "Check", "attention"],
    ["/Users/you/code/billing", "staging", "Incomplete", "bad"],
    ["/Users/you/code/blog", "production", "Not known", "attention"],
    ["/Users/you/code/blog", "staging", "No code", "muted"],
    ["/Users/you/code/shop", "production", "Succeeded", "ink"],
    ["/Users/you/code/shop", "staging", "New errors", "bad"],
  ])("%s %s reads %s", (repoRoot, env, word, tone) => {
    expect(status(at(repoRoot, env))).toEqual({ word, tone });
  });

  test("a newer failed attempt outranks the older success it sits on", () => {
    const api = at("/Users/you/code/api", "production");
    expect(api.outcome?.tone).toBe("good");
    expect(status(api).word).toBe("Last failed");
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
    expect(c).not.toHaveTextContent("2 new error groups after this run");
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
