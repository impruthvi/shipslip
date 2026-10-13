import { render, screen, within } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import * as fixtures from "./fixtures.gen";
import { ReleaseMap } from "./ReleaseMap";
import type { MapView } from "./types";

const map: MapView = (() => {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  return fixtures.map.map;
})();

function cell(repoRoot: string, env: string) {
  render(<ReleaseMap map={map} />);
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
    expect(within(c).getByText("Succeeded")).toHaveClass("tone-good");
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

  test("not known: hatched, no commit, the reason in red", () => {
    const c = cell("/Users/you/code/blog", "production");
    expect(c.querySelector(".cell")).toHaveClass("cell-hatched");
    expect(c).toHaveTextContent("server code not known");
    expect(c.querySelector(".cell-sha")).toBeNull();
    expect(within(c).getByText(/fast-forward failed/)).toHaveClass("tone-bad");
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
