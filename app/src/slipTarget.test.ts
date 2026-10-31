import { describe, expect, test } from "vitest";
import * as fixtures from "./fixtures.gen";
import { context, target } from "./slipTarget";
import type { EnvCellView } from "./types";

const map = fixtures.map.kind === "loaded" ? fixtures.map.map : (null as never);

function cellAt(repoRoot: string, env: string): EnvCellView {
  const row = map.rows.find((r) => r.repoRoot === repoRoot);
  const cell = row?.cells[map.envs.indexOf(env)];
  if (cell?.kind !== "present") throw new Error(`no cell ${repoRoot} ${env}`);
  return cell;
}

describe("which receipt a cell opens", () => {
  test("the code run by default; the last attempt and the unreadable newer receipt on request", () => {
    const api = cellAt("/Users/you/code/api", "production");
    expect(target(api, "code")).toEqual({ id: api.receipt, role: "code" });
    expect(target(api, "last")).toEqual({ id: api.lastAttempt?.receipt, role: "last" });
    const views = (ctx: ReturnType<typeof context>) => ctx?.links.map((link) => link.view);
    expect(context(api, "code")?.text).toBe("Receipt behind the recorded code");
    expect(views(context(api, "code"))).toEqual(["last"]);
    expect(context(api, "last")?.text).toBe("Latest attempt · recorded code comes from an earlier run");
    expect(views(context(api, "last"))).toEqual(["code"]);

    const billing = cellAt("/Users/you/code/billing", "production");
    expect(target(billing, "stale")).toEqual({ id: billing.staleReceipt, role: "stale" });
    expect(views(context(billing, "code"))).toEqual(["stale"]);
    expect(views(context(billing, "stale"))).toEqual(["code"]);
  });

  test("a view the cell lacks falls back to the code run", () => {
    const shop = cellAt("/Users/you/code/shop", "staging");
    expect(shop.lastAttempt).toBeNull();
    expect(target(shop, "last")).toEqual({ id: shop.receipt, role: "code" });
    expect(target(shop, "stale")).toEqual({ id: shop.receipt, role: "code" });
  });

  test("no receipt on this Mac: nothing to open and no context line", () => {
    const blog = cellAt("/Users/you/code/blog", "staging");
    expect(target(blog, "code")).toEqual({ id: null, role: null });
    expect(context(blog, null)).toBeNull();
  });

  test("only a last attempt: it opens, with no way back to a code run", () => {
    const blog = cellAt("/Users/you/code/blog", "staging");
    const attemptOnly: EnvCellView = {
      ...blog,
      lastAttempt: { receipt: 7, outcome: { text: "Cancelled before changes", tone: "neutral", kind: "cancelled" }, flags: [], atMs: 0, age: "1m ago" },
    };
    expect(target(attemptOnly, "code")).toEqual({ id: 7, role: "last" });
    expect(context(attemptOnly, "last")).toEqual({ text: "Latest attempt", links: [] });
  });
});
