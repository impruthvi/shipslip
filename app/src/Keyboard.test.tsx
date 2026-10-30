import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import { cellName } from "./Cell";
import * as fixtures from "./fixtures.gen";
import type { LoadResult, ReceiptResult } from "./types";

const loadMap = vi.fn<() => Promise<LoadResult>>();
const loadReceipt = vi.fn<(generation: number, id: number) => Promise<ReceiptResult>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    compare: async () => ({ kind: "notFound" }),
    loadReceipt: (generation: number, id: number) => loadReceipt(generation, id),
    copyText: async () => {},
    onFocus: async () => () => {},
    receiptSeen: async () => {},
  },
}));

const map = fixtures.map.kind === "loaded" ? fixtures.map.map : (null as never);

let narrow = false;
beforeEach(() => {
  localStorage.clear();
  narrow = false;
  window.matchMedia = ((query: string) => ({
    matches: narrow,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
  loadMap.mockReset().mockResolvedValue(fixtures.map);
  loadReceipt.mockReset().mockImplementation(async (_g, id) => fixtures.receipts[String(id)]);
});

async function settle() {
  await act(async () => {});
}

function cell(repoRoot: string, env: string) {
  const row = map.rows.findIndex((r) => r.repoRoot === repoRoot);
  return within(screen.getAllByRole("row")[row + 1]).getAllByRole("gridcell")[map.envs.indexOf(env)];
}

/** What Tab visits, in order: enabled, not hidden from Tab. */
function tabStops(): HTMLElement[] {
  const focusable = "button, select, a[href], input, [tabindex]";
  return [...document.querySelectorAll<HTMLElement>(focusable)].filter(
    (el) => el.tabIndex >= 0 && !(el as HTMLButtonElement).disabled,
  );
}

const name = (el: HTMLElement) => el.getAttribute("aria-label") ?? el.textContent?.trim() ?? "";

test("Tab goes sidebar → problems → picker → grid (one stop) → receipt", async () => {
  render(<App />);
  await settle();
  const stops = tabStops();
  const gridStops = stops.filter((el) => el.getAttribute("role") === "gridcell");
  expect(gridStops).toHaveLength(1);
  expect(gridStops[0]).toHaveAttribute("aria-selected", "true");
  const order = stops.map((el) =>
    el.classList.contains("side-item")
      ? "sidebar"
      : el.id === "compare-pair"
      ? "picker"
      : el.closest("[aria-label=Problems]")
        ? "problems"
        : el.getAttribute("role") === "gridcell"
          ? "grid"
          : el.closest("footer")
            ? "footer"
            : el.closest("aside")
              ? "receipt"
              : `other: ${name(el)}`,
  );
  expect([...new Set(order)]).toEqual(["sidebar", "problems", "picker", "grid", "receipt"]);
  for (const button of within(screen.getByRole("grid")).queryAllByRole("button")) {
    expect(button).toHaveAttribute("tabindex", "-1");
  }
});

test("arrows move focus between cells and skip absent ones; the Tab stop follows", async () => {
  render(<App />);
  await settle();
  const blogProd = cell("/Users/you/code/blog", "production");
  blogProd.focus();
  fireEvent.keyDown(blogProd, { key: "ArrowDown" });
  const shopProd = cell("/Users/you/code/shop", "production");
  expect(shopProd).toHaveFocus();
  expect(shopProd).toHaveAttribute("tabindex", "0");
  expect(blogProd).toHaveAttribute("tabindex", "-1");
  fireEvent.keyDown(shopProd, { key: "ArrowRight" });
  expect(cell("/Users/you/code/shop", "staging")).toHaveFocus();
  // Focus alone does not change the selection.
  expect(cell("/Users/you/code/api", "production")).toHaveAttribute("aria-selected", "true");
});

test("Enter opens the receipt and moves focus to it; Esc comes back to the cell", async () => {
  render(<App />);
  await settle();
  const billing = cell("/Users/you/code/billing", "staging");
  billing.focus();
  fireEvent.keyDown(billing, { key: "Enter" });
  await settle();
  expect(billing).toHaveAttribute("aria-selected", "true");
  const title = screen.getByRole("heading", { name: "billing · staging" });
  expect(title).toHaveFocus();
  fireEvent.keyDown(title, { key: "Escape" });
  expect(billing).toHaveFocus();
});

test("Space opens too; in a narrow window Enter opens the drawer and focuses it", async () => {
  narrow = true;
  render(<App />);
  await settle();
  expect(screen.queryByRole("complementary", { name: "Receipt" })).not.toBeInTheDocument();
  const shop = cell("/Users/you/code/shop", "staging");
  shop.focus();
  fireEvent.keyDown(shop, { key: " " });
  await settle();
  expect(screen.getByRole("heading", { name: "shop · staging" })).toHaveFocus();
  fireEvent.keyDown(document.activeElement!, { key: "Escape" });
  expect(screen.queryByRole("complementary", { name: "Receipt" })).not.toBeInTheDocument();
  expect(shop).toHaveFocus();
});

test("a slow receipt takes focus when it arrives, not before", async () => {
  render(<App />);
  await settle();
  let resolve!: (value: ReceiptResult) => void;
  loadReceipt.mockReturnValueOnce(new Promise((r) => (resolve = r)));
  const billing = cell("/Users/you/code/billing", "staging");
  billing.focus();
  fireEvent.keyDown(billing, { key: "Enter" });
  await settle();
  expect(billing).toHaveFocus();
  const id = map.rows.find((r) => r.repoRoot === "/Users/you/code/billing")!.cells[map.envs.indexOf("staging")];
  if (id.kind !== "present") throw new Error("fixture");
  await act(async () => resolve(fixtures.receipts[String(id.receipt)]));
  expect(screen.getByRole("heading", { name: "billing · staging" })).toHaveFocus();
});

test("moving on before a slow receipt arrives keeps focus in the grid", async () => {
  render(<App />);
  await settle();
  let resolve!: (value: ReceiptResult) => void;
  loadReceipt.mockReturnValueOnce(new Promise((r) => (resolve = r)));
  const billing = cell("/Users/you/code/billing", "staging");
  billing.focus();
  fireEvent.keyDown(billing, { key: "Enter" });
  await settle();
  fireEvent.keyDown(billing, { key: "ArrowLeft" });
  const next = cell("/Users/you/code/billing", "production");
  expect(next).toHaveFocus();
  const id = map.rows.find((r) => r.repoRoot === "/Users/you/code/billing")!.cells[map.envs.indexOf("staging")];
  if (id.kind !== "present") throw new Error("fixture");
  await act(async () => resolve(fixtures.receipts[String(id.receipt)]));
  expect(next).toHaveFocus();
});

test("⌘I closing the drawer from inside it puts focus back on the cell", async () => {
  narrow = true;
  render(<App />);
  await settle();
  const shop = cell("/Users/you/code/shop", "staging");
  shop.focus();
  fireEvent.keyDown(shop, { key: "Enter" });
  await settle();
  const title = screen.getByRole("heading", { name: "shop · staging" });
  expect(title).toHaveFocus();
  fireEvent.keyDown(title, { key: "i", metaKey: true });
  expect(screen.queryByRole("complementary", { name: "Receipt" })).not.toBeInTheDocument();
  expect(shop).toHaveFocus();
});

test("Enter on a link inside a cell is the link's, not the cell's", async () => {
  render(<App />);
  await settle();
  const last = screen.getByRole("button", { name: /^Last: stopped in maintenance/ });
  last.focus();
  fireEvent.keyDown(last, { key: "Enter" });
  await settle();
  expect(screen.queryByRole("heading", { name: "api · production" })).not.toHaveFocus();
});

test("Select cell in the problems strip selects and focuses that cell", async () => {
  render(<App />);
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "Select cell" }));
  const billingProd = cell("/Users/you/code/billing", "production");
  expect(billingProd).toHaveAttribute("aria-selected", "true");
  expect(billingProd).toHaveFocus();
});

test("the recorded code and the unreadable receipt are reachable from the receipt", async () => {
  render(<App />);
  await settle();
  const pane = screen.getByRole("complementary", { name: "Receipt" });
  fireEvent.click(within(pane).getByRole("tab", { name: "Recorded code" }));
  await settle();
  expect(within(pane).getByText("Receipt behind the recorded code")).toBeInTheDocument();

  fireEvent.click(cell("/Users/you/code/billing", "production"));
  await settle();
  fireEvent.click(within(pane).getByRole("tab", { name: "Unreadable newer" }));
  await settle();
  expect(within(pane).getByText("Can't read this receipt.")).toBeInTheDocument();
});

test("cells have spoken names: where, what, then the cell's lines", async () => {
  render(<App />);
  await settle();
  expect(cell("/Users/you/code/api", "production")).toHaveAccessibleName(
    "api, production, recorded code fffffff, 24h ago, Succeeded, " +
      "approved config now deploys to production2:/var/www/api, not this target, " +
      "last attempt: stopped in maintenance mode, no steps ran, 20m ago, app may be in maintenance mode",
  );
  expect(cell("/Users/you/code/blog", "staging")).toHaveAccessibleName(
    "blog, staging, no deploy receipt on this Mac, approved before",
  );
  expect(cell("/Users/you/tmp/shop-copy", "production")).toHaveAccessibleName("shop, production, none");
  expect(cellName("shop", "staging", { kind: "absent" })).toBe("shop, staging, none");
});
