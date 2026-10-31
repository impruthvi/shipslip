import { act, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, test, vi } from "vitest";
import App from "./App";
import type { CompareLine } from "./compare";
import * as fixtures from "./fixtures.gen";
import { markLabel } from "./Mark";
import { ReleaseMap } from "./ReleaseMap";
import { Slip } from "./Slip";
import { context, type Target } from "./slipTarget";
import type { CellView, EnvCellView, LoadResult, MapView } from "./types";
import type { Opened } from "./useReceipt";

// The app shows what this Mac recorded, never what a server is running now
// (DD13). This walks every screen, cell, comparison and receipt the UI can
// draw and checks two things: each cell's state is framed as evidence, and
// no sentence anywhere claims the server's present state.

vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    compare: async () => ({ kind: "notFound" }),
    loadReceipt: async (_generation: number, id: number) => fixtures.receipts[String(id)],
    copyText: async () => {},
    onFocus: async () => () => {},
    receiptSeen: async () => {},
  },
}));
const loadMap = vi.fn<() => Promise<LoadResult>>();

const SUBTITLE = "What this Mac recorded for each app and environment";

/** Sentences that would claim what a server is running right now. */
const LIVE_CLAIMS: RegExp[] = [
  /\blive\b/i,
  /\bcurrently\b/i,
  /\bright now\b/i,
  /\brunning now\b/i,
  /\bnow (?:on|running|serving)\b/i,
  /\b(?:is|are) (?:running|serving)\s+(?:[0-9a-f]{7}|on\b|the\b|this\b|commit)/i,
  /\b(?:is|are) on [0-9a-f]{7}/i,
  /\b(?:is|are|now) deployed\b/i,
  /(?<!\b(?:was|were|been|be) )\bdeployed (?:to|on)\b/i,
];

function liveClaims(text: string): string[] {
  const withoutSubtitle = text.split(SUBTITLE).join(" ");
  return LIVE_CLAIMS.flatMap((pattern) => {
    const match = withoutSubtitle.match(pattern);
    if (!match) return [];
    const at = match.index ?? 0;
    return [`${match[0]} … in "${withoutSubtitle.slice(Math.max(0, at - 40), at + 40)}"`];
  });
}

/** Everything a person can read or hear: text, spoken names, hover text.
 * Text nodes are joined with spaces so words in neighbouring elements stay
 * apart, as they read on screen. */
function readable(root: HTMLElement = document.body): string {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const text: string[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) text.push(node.nodeValue ?? "");
  const attributes = [...root.querySelectorAll("[aria-label], [title]")].flatMap((el) => [
    el.getAttribute("aria-label") ?? "",
    el.getAttribute("title") ?? "",
  ]);
  return [text.join(" ").replace(/\s+/g, " "), ...attributes].join("\n");
}

const map: MapView = fixtures.map.kind === "loaded" ? fixtures.map.map : (null as never);

const present = (cell: CellView): cell is { kind: "present" } & EnvCellView => cell.kind === "present";

beforeEach(() => {
  localStorage.clear();
  loadMap.mockReset();
});

async function app(result: LoadResult) {
  loadMap.mockResolvedValue(result);
  const view = render(<App />);
  await act(async () => {});
  return view;
}

describe("the checker", () => {
  test.each([
    "production is running a1b2c3d",
    "shop is live on web1",
    "staging is on a1b2c3d",
    "Currently deployed to production",
    "a1b2c3d is deployed",
    "web1 is now serving a1b2c3d",
  ])("catches %j", (claim) => {
    expect(liveClaims(claim)).not.toEqual([]);
  });

  test.each([
    SUBTITLE,
    "Recorded code a1b2c3d · 3d ago",
    "observed after run 00000009 failed at step 1",
    "Server code not known",
    "Approved config now deploys to production2:/var/www/api, not this target",
    "runs the remaining steps on the deployed commit",
    "a1b2c3d was deployed to production 3d ago",
    "? running when saved",
    "The run did not finish. If no deploy is running, resume it from /Users/you/code/shop",
    "Opening receipt for shop · production…",
  ])("lets evidence through: %j", (words) => {
    expect(liveClaims(words)).toEqual([]);
  });

  test("a claim split across two elements is still caught", () => {
    render(
      <p>
        <span>production is</span>
        <span>running a1b2c3d</span>
      </p>,
    );
    expect(liveClaims(readable())).not.toEqual([]);
  });

  test("a live claim slipped into a cell is caught on screen", () => {
    const rows = map.rows.map((row) => ({
      ...row,
      cells: row.cells.map((cell) =>
        present(cell) && cell.outcome
          ? { ...cell, outcome: { ...cell.outcome, text: "production is running this commit" } }
          : cell,
      ),
    }));
    render(<ReleaseMap map={{ ...map, rows }} />);
    expect(liveClaims(readable())).not.toEqual([]);
  });
});

describe("every screen", () => {
  test.each([
    ["the release map", fixtures.map],
    ["first run", fixtures.firstRun],
    ["no readable receipts", fixtures.noneReadable],
    ["an unreadable receipts folder", fixtures.failed],
  ] as const)("%s keeps to evidence", async (_name, result) => {
    await app(result);
    expect(screen.getByText(SUBTITLE)).toBeInTheDocument();
    expect(liveClaims(readable())).toEqual([]);
  });

  test("the receipt open beside the map keeps to evidence", async () => {
    await app(fixtures.map);
    const pane = screen.getByRole("complementary", { name: "Receipt" });
    expect(within(pane).getByRole("heading", { level: 2 })).toBeInTheDocument();
    expect(liveClaims(readable())).toEqual([]);
  });
});

describe("every cell", () => {
  const evidence = new Set(Object.values(markLabel));

  test("each state is told as evidence: a mark from the key, then recorded words", () => {
    render(<ReleaseMap map={map} />);
    const cells = map.rows.flatMap((row) => row.cells.filter(present));
    const marks = screen.getAllByRole("img").filter((el) => el.closest('[role="gridcell"]'));
    expect(marks).toHaveLength(cells.length);
    for (const mark of marks) {
      expect(evidence).toContain(mark.getAttribute("aria-label"));
    }
    for (const cell of cells) {
      if (!cell.shortSha) {
        expect(cell.words).toMatch(/^(server code not known|no deploy receipt on this Mac|no code recorded)/);
      }
    }
  });

  test("with every comparison drawn, the grid still keeps to evidence", () => {
    const pairs = new Map<string, Record<number, CompareLine>>();
    for (const [key, result] of Object.entries(fixtures.compares)) {
      if (result.kind !== "compared") continue;
      const [, a, b] = key.split(":");
      const pairKey = `${a}:${b}`;
      const lines = pairs.get(pairKey) ?? {};
      lines[result.row] = { status: "done", ...result.comparison };
      pairs.set(pairKey, lines);
    }
    expect(pairs.size).toBeGreaterThan(0);
    for (const [pairKey, lines] of pairs) {
      const [a, b] = pairKey.split(":").map(Number);
      const { unmount } = render(
        <ReleaseMap map={map} compare={{ pair: { a: map.envs[a], b: map.envs[b] }, lines }} />,
      );
      expect(liveClaims(readable())).toEqual([]);
      unmount();
    }
    const waiting: Record<number, CompareLine> = { 0: { status: "pending" }, 1: { status: "slow" } };
    render(<ReleaseMap map={map} compare={{ pair: { a: map.envs[0], b: map.envs[1] }, lines: waiting }} />);
    expect(liveClaims(readable())).toEqual([]);
  });
});

describe("every receipt", () => {
  const cells = map.rows.flatMap((row) =>
    row.cells.flatMap((cell, col) => (present(cell) ? [{ row, cell, env: map.envs[col] }] : [])),
  );
  const roles: Target["role"][] = ["code", "last", "stale", null];

  function slips(): [string, Opened][] {
    const opened = Object.entries(fixtures.receipts).flatMap(([id, result]) =>
      result.kind === "opened" ? [[`receipt ${id}`, result.slip] as [string, Opened]] : [],
    );
    return [
      ...opened,
      ...Object.entries(fixtures.slips).map(([name, slip]) => [name, slip] as [string, Opened]),
      ["no receipt", { kind: "none" }],
      ["not found", { kind: "notFound" }],
      ["failed", { kind: "failed", message: "invoke failed" }],
    ];
  }

  test.each(slips())("%s keeps to evidence in every context", (_name, opened) => {
    for (const { row, cell, env } of cells) {
      for (const role of roles) {
        const { unmount } = render(
          <Slip
            opened={opened}
            cell={cell}
            label={row.label}
            env={env}
            context={context(cell, role)}
            onView={() => {}}
          />,
        );
        expect(liveClaims(readable())).toEqual([]);
        unmount();
      }
    }
  });
});
