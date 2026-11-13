import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import type {
  CheckView,
  CheckoutReceipts,
  DeployEntry,
  DeployReview,
  LiveRuns,
  LoadResult,
  ReceiptResult,
} from "./types";

const loadMap = vi.fn<() => Promise<LoadResult>>();
const loadReceipt = vi.fn<(generation: number, id: number) => Promise<ReceiptResult>>();
const checkoutReceipts = vi.fn<(generation: number, row: number) => Promise<CheckoutReceipts>>();
const checkConnection =
  vi.fn<(generation: number, row: number, env: string, onLines: (view: CheckView) => void) => Promise<CheckView>>();
const removeCheckout = vi.fn<(generation: number, row: number) => Promise<void>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    receiptsChanged: async () => false,
    compare: vi.fn(),
    loadReceipt: (generation: number, id: number) => loadReceipt(generation, id),
    checkoutReceipts: (generation: number, row: number) => checkoutReceipts(generation, row),
    copyText: vi.fn(),
    onFocus: async () => () => {},
    receiptSeen: async () => {},
    checkConnection: (...args: [number, number, string, (view: CheckView) => void]) => checkConnection(...args),
    removeCheckout: (...args: [number, number]) => removeCheckout(...args),
  },
}));

const entry = vi.fn<(generation: number, row: number, env: string) => Promise<DeployEntry>>();
const prepare = vi.fn<(generation: number, row: number, env: string, plan: unknown) => Promise<DeployReview>>();
const openConfig = vi.fn<(generation: number, row: number, env: string) => Promise<void>>();
const openFolder = vi.fn<(generation: number, row: number) => Promise<void>>();
const reviewTrust = vi.fn();
const liveRuns = vi.fn<() => Promise<LiveRuns>>();
vi.mock("./deployApi", () => ({
  deploy: {
    entry: (...args: [number, number, string]) => entry(...args),
    prepare: (...args: [number, number, string, unknown]) => prepare(...args),
    confirm: vi.fn(),
    cancel: async () => {},
    follow: async () => () => {},
    stop: vi.fn(),
    reviewTrust: (...args: unknown[]) => reviewTrust(...args),
    approveTrust: vi.fn(),
    openConfig: (...args: [number, number, string]) => openConfig(...args),
    openFolder: (...args: [number, number]) => openFolder(...args),
    reviewUp: vi.fn(),
    reviewAttach: vi.fn(),
    liveRuns: () => liveRuns(),
  },
}));

function ready(over: Partial<Extract<DeployEntry, { kind: "ready" }>> = {}): DeployEntry {
  return { kind: "ready", production: false, needsTrust: false, steps: [], recovery: null, ...over };
}

beforeEach(() => {
  localStorage.clear();
  loadMap.mockReset().mockResolvedValue(fixtures.map);
  loadReceipt.mockReset().mockImplementation(async (_generation, id) => fixtures.receipts[String(id)]);
  checkoutReceipts.mockReset().mockImplementation(async (_generation, row) => fixtures.checkoutReceipts[row]);
  entry.mockReset().mockResolvedValue(ready());
  prepare.mockReset().mockReturnValue(new Promise(() => {}));
  openConfig.mockReset().mockResolvedValue(undefined);
  openFolder.mockReset().mockResolvedValue(undefined);
  reviewTrust.mockReset().mockReturnValue(new Promise(() => {}));
  liveRuns.mockReset().mockResolvedValue({ runs: [], closedPreviews: [] });
  checkConnection.mockReset().mockResolvedValue(fixtures.connectionChecks.pass);
  removeCheckout.mockReset().mockResolvedValue(undefined);
});

async function settle() {
  await act(async () => {});
}

const main = () => screen.getByRole("main");

async function openCheckout(name: RegExp) {
  render(<App />);
  await settle();
  const projects = screen.getByRole("region", { name: "Projects" });
  fireEvent.click(within(projects).getByRole("button", { name }));
  await settle();
}

function envSection(env: string) {
  return within(main()).getByRole("region", { name: env });
}

test("selecting a checkout shows its environments side by side with targets, and its heading takes focus", async () => {
  await openCheckout(/^api/);
  const heading = screen.getByRole("heading", { level: 1, name: "api" });
  expect(heading).toHaveFocus();
  expect(screen.queryByRole("grid", { name: "Release map" })).toBeNull();
  const projects = screen.getByRole("region", { name: "Projects" });
  expect(within(projects).getByRole("button", { name: /^api/ })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("button", { name: "All projects" })).not.toHaveAttribute("aria-current");

  const production = envSection("production");
  expect(within(production).getByText("Maintenance?")).toBeInTheDocument();
  // Expanded: the cell says why it reads as it does.
  expect(within(production).getByText(/^Last: stopped in maintenance mode/)).toBeInTheDocument();
  expect(within(production).getByText("production1:/var/www/api")).toBeInTheDocument();
  expect(within(envSection("staging")).getByText("staging1:/var/www/api")).toBeInTheDocument();
  expect(checkoutReceipts).toHaveBeenCalledWith(1, 0);
});

test("the receipts table is newest first with age, environment, outcome and commit", async () => {
  await openCheckout(/^api/);
  const table = within(main()).getByRole("region", { name: "Receipts" });
  const rows = within(table).getAllByRole("button");
  expect(rows.map((row) => row.textContent)).toEqual([
    "20m agoproductionStopped in maintenance mode, no steps ranfffffff",
    "4h agostagingFailed at step 17777777",
    "25h agoproductionSucceededfffffff",
  ]);
  // Red for the failed step only; the maintenance stop is amber.
  expect(within(rows[1]).getByText("Failed at step 1")).toHaveClass("tone-bad");
  expect(within(rows[0]).getByText(/^Stopped in maintenance/)).toHaveClass("tone-attention");
  expect(within(rows[2]).getByText("Succeeded")).not.toHaveClass("tone-good");
});

test("selecting a receipt opens it in the receipt pane", async () => {
  await openCheckout(/^api/);
  expect(screen.queryByRole("complementary", { name: "Receipt" })).toBeNull();
  const table = within(main()).getByRole("region", { name: "Receipts" });
  const older = within(table).getAllByRole("button")[2];
  fireEvent.click(older);
  await settle();
  expect(loadReceipt).toHaveBeenLastCalledWith(1, 0);
  const pane = screen.getByRole("complementary", { name: "Receipt" });
  expect(within(pane).getByText("api → production")).toBeInTheDocument();
  expect(older).toHaveAttribute("aria-current", "true");
  expect(within(table).getAllByRole("button")[0]).not.toHaveAttribute("aria-current");
});

test("Deploy enters the deploy flow for its own environment", async () => {
  await openCheckout(/^api/);
  const button = within(envSection("staging")).getByRole("button", { name: "Deploy to staging…" });
  expect(button).not.toHaveClass("button-primary");
  fireEvent.click(button);
  await settle();
  expect(prepare).toHaveBeenCalledWith(1, 0, "staging", "Deploy", expect.any(Function));
  expect(screen.getByRole("heading", { level: 1, name: "Deploy api → staging" })).toHaveFocus();
});

test("Deploy for a config that needs approval opens the review first", async () => {
  entry.mockResolvedValue(ready({ needsTrust: true }));
  await openCheckout(/^api/);
  fireEvent.click(within(envSection("production")).getByRole("button", { name: "Deploy to production…" }));
  await settle();
  expect(reviewTrust).toHaveBeenCalledWith(1, 0, "production");
  expect(prepare).not.toHaveBeenCalled();
  expect(screen.getByRole("heading", { level: 1, name: "Review configuration · api · production" })).toBeInTheDocument();
});

test("Deploy is disabled as Running… while its environment has a run going", async () => {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  const api = fixtures.map.map.rows[0];
  liveRuns.mockResolvedValue({
    runs: [{ ...fixtures.runInfo, checkout: api.repoRoot, project: "api", env: "staging" }],
    closedPreviews: [],
  });
  await openCheckout(/^api/);
  const running = within(envSection("staging")).getByRole("button", { name: "Running…" });
  expect(running).toBeDisabled();
  fireEvent.click(running);
  expect(prepare).not.toHaveBeenCalled();
  expect(within(envSection("production")).getByRole("button", { name: "Deploy to production…" })).toBeEnabled();
});

test("a run the library reports as going disables Deploy too", async () => {
  entry.mockImplementation(async (_generation, _row, env) =>
    env === "staging" ? { kind: "running", handle: "h" } : ready(),
  );
  await openCheckout(/^api/);
  expect(within(envSection("staging")).getByRole("button", { name: "Running…" })).toBeDisabled();
});

test("Deploy that is not available is disabled and says why", async () => {
  entry.mockResolvedValue({ kind: "unavailable", reason: "This checkout has no .shipslip.toml." });
  await openCheckout(/^api/);
  const button = within(envSection("staging")).getByRole("button", { name: "Deploy to staging…" });
  expect(button).toBeDisabled();
  expect(button).toHaveAccessibleDescription("This checkout has no .shipslip.toml.");
});

test("an approved environment with nothing deployed from this Mac says so plainly", async () => {
  await openCheckout(/^blog/);
  const staging = envSection("staging");
  expect(within(staging).getByText("Approved. Nothing deployed from this Mac yet.")).toBeInTheDocument();
  expect(within(staging).getByText("staging1:/var/www/blog")).toBeInTheDocument();
  expect(within(staging).queryByText("approved before")).toBeNull();
});

test("a checkout with no receipts says so", async () => {
  checkoutReceipts.mockResolvedValue({ kind: "listed", generation: 1, row: 2, receipts: [] });
  await openCheckout(/^blog/);
  const table = within(main()).getByRole("region", { name: "Receipts" });
  expect(within(table).getByText("No receipts for this checkout on this Mac yet.")).toBeInTheDocument();
});

test("a receipts list that cannot be read says why", async () => {
  checkoutReceipts.mockRejectedValue("receipts root is unavailable");
  await openCheckout(/^blog/);
  const table = within(main()).getByRole("region", { name: "Receipts" });
  expect(within(table).getByText("Couldn't list this checkout's receipts: receipts root is unavailable")).toBeInTheDocument();
});

test("an unreadable receipts folder is said, never shown as no receipts", async () => {
  checkoutReceipts.mockResolvedValue({ kind: "unreadable", message: "Permission denied (os error 13)" });
  await openCheckout(/^blog/);
  const table = within(main()).getByRole("region", { name: "Receipts" });
  expect(within(table).getByText("Couldn't list this checkout's receipts: Permission denied (os error 13)")).toBeInTheDocument();
  expect(within(table).queryByText("No receipts for this checkout on this Mac yet.")).toBeNull();
});

test("a receipts answer for an older map lists nothing, and does not claim there are no receipts", async () => {
  checkoutReceipts.mockResolvedValue({ kind: "stale" });
  await openCheckout(/^blog/);
  const table = within(main()).getByRole("region", { name: "Receipts" });
  expect(within(table).queryByRole("list")).toBeNull();
  expect(within(table).queryByText("No receipts for this checkout on this Mac yet.")).toBeNull();
});

test("Deploy stays off when its entry could not be asked for", async () => {
  entry.mockRejectedValue({ code: "stale", message: "The map was read again since this was selected." });
  await openCheckout(/^api/);
  const button = within(envSection("staging")).getByRole("button", { name: "Deploy to staging…" });
  expect(button).toBeDisabled();
  fireEvent.click(button);
  expect(prepare).not.toHaveBeenCalled();
});

test("a never-deployed environment without an approval does not say it is approved", async () => {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  const unapproved = structuredClone(fixtures.map);
  if (unapproved.kind !== "loaded") throw new Error("fixture");
  const staging = unapproved.map.rows[2].cells[1];
  if (staging.kind !== "present") throw new Error("fixture");
  staging.approved = false;
  loadMap.mockResolvedValue(unapproved);
  await openCheckout(/^blog/);
  const section = envSection("staging");
  expect(within(section).getByText("Nothing deployed from this Mac yet.")).toBeInTheDocument();
  expect(within(section).queryByText(/^Approved\./)).toBeNull();
});

test("at most one primary button shows, even with a receipt open", async () => {
  await openCheckout(/^api/);
  expect(document.querySelectorAll(".button-primary")).toHaveLength(0);
  fireEvent.click(within(within(main()).getByRole("region", { name: "Receipts" })).getAllByRole("button")[0]);
  await settle();
  expect(screen.getByRole("complementary", { name: "Receipt" })).toBeInTheDocument();
  expect(document.querySelectorAll(".button-primary").length).toBeLessThanOrEqual(1);
});

test("the checkout menu opens the folder and the config, and says why when it can't", async () => {
  await openCheckout(/^api/);
  const menu = () => within(main()).getByRole("button", { name: "api actions" });
  fireEvent.click(menu());
  fireEvent.click(screen.getByRole("menuitem", { name: "Open folder in Finder" }));
  await settle();
  expect(openFolder).toHaveBeenCalledWith(1, 0);

  openConfig.mockRejectedValue({ code: "notFound", message: "This checkout has no .shipslip.toml." });
  fireEvent.click(menu());
  fireEvent.click(screen.getByRole("menuitem", { name: "Open .shipslip.toml" }));
  await settle();
  expect(openConfig).toHaveBeenCalledWith(1, 0, "production");
  expect(within(main()).getByRole("status")).toHaveTextContent("This checkout has no .shipslip.toml.");
});

test("Check connection from the checkout menu runs the read-only check for the environment picked", async () => {
  await openCheckout(/^api/);
  fireEvent.click(within(main()).getByRole("button", { name: "api actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Check connection" }));
  await settle();
  const dialog = screen.getByRole("dialog", { name: "Check connection" });
  expect(within(dialog).getByRole("heading", { name: "Check connection" })).toHaveFocus();
  expect(within(dialog).getByText(/nothing on the server changes\.$/)).toBeInTheDocument();
  expect(checkConnection).not.toHaveBeenCalled();

  const picker = within(dialog).getByRole("combobox", { name: "Environment" });
  expect(picker).toHaveValue("production");
  fireEvent.change(picker, { target: { value: "staging" } });
  checkConnection.mockResolvedValue(fixtures.connectionChecks.changedKey);
  fireEvent.click(within(dialog).getByRole("button", { name: "Check connection" }));
  await settle();
  expect(checkConnection).toHaveBeenCalledWith(1, 0, "staging", expect.any(Function));
  expect(within(dialog).getByRole("alert")).toHaveTextContent("Host key changed for staging. Shipslip stops here.");
  expect(within(dialog).getByRole("list", { name: "Connection check" })).toBeInTheDocument();
  expect(within(dialog).getByRole("button", { name: "Check again" })).toBeEnabled();

  fireEvent.keyDown(window, { key: "Escape" });
  await settle();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.getByRole("heading", { level: 1, name: "api" })).toHaveFocus();
});

test("Close leaves the check dialog without running anything", async () => {
  await openCheckout(/^api/);
  fireEvent.click(within(main()).getByRole("button", { name: "api actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Check connection" }));
  await settle();
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(checkConnection).not.toHaveBeenCalled();
});

test("Remove from sidebar is not offered for a checkout with anything recorded", async () => {
  await openCheckout(/^api/);
  fireEvent.click(within(main()).getByRole("button", { name: "api actions" }));
  expect(screen.getByRole("menuitem", { name: "Open folder in Finder" })).toBeInTheDocument();
  expect(screen.queryByRole("menuitem", { name: "Remove from sidebar" })).toBeNull();
});

test("Remove from sidebar drops a checkout with nothing recorded and returns to All projects", async () => {
  if (fixtures.addedMap.kind !== "loaded") throw new Error("fixture");
  loadMap.mockResolvedValueOnce(fixtures.removableMap).mockResolvedValue({ ...fixtures.addedMap, generation: 2 });
  await openCheckout(/^shop/);
  fireEvent.click(within(main()).getByRole("button", { name: "shop actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Remove from sidebar" }));
  await settle();
  expect(removeCheckout).toHaveBeenCalledWith(1, 0);
  expect(loadMap).toHaveBeenCalledTimes(2);
  expect(screen.getByRole("heading", { level: 1, name: "All projects" })).toHaveFocus();
  const projects = screen.getByRole("region", { name: "Projects" });
  expect(within(projects).queryByRole("button", { name: /^shop/ })).toBeNull();
});

test("a refused removal says why and keeps the checkout", async () => {
  loadMap.mockResolvedValue(fixtures.removableMap);
  removeCheckout.mockRejectedValue({
    code: "local",
    message: "This Mac has receipts or approvals for this checkout, so it stays in the sidebar.",
  });
  await openCheckout(/^shop/);
  fireEvent.click(within(main()).getByRole("button", { name: "shop actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Remove from sidebar" }));
  await settle();
  expect(within(main()).getByRole("status")).toHaveTextContent(
    "This Mac has receipts or approvals for this checkout, so it stays in the sidebar.",
  );
  expect(screen.getByRole("heading", { level: 1, name: "shop" })).toBeInTheDocument();
});

test("All projects goes back to the map with its heading focused", async () => {
  await openCheckout(/^api/);
  fireEvent.click(screen.getByRole("button", { name: "All projects" }));
  await settle();
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1, name: "All projects" })).toHaveFocus();
});

test("a Needs attention item from a checkout shows its cell on the map", async () => {
  await openCheckout(/^api/);
  const attention = screen.getByRole("region", { name: /^Needs attention/ });
  fireEvent.click(within(attention).getAllByRole("button")[0]);
  await settle();
  expect(screen.getByRole("heading", { level: 1, name: "All projects" })).toBeInTheDocument();
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(document.activeElement).toHaveAttribute("role", "gridcell");
});

test("leaving the deploy flow returns to the checkout, focus on that environment's Deploy", async () => {
  prepare.mockRejectedValue({ code: "local", message: "ssh failed" });
  await openCheckout(/^api/);
  fireEvent.click(within(envSection("staging")).getByRole("button", { name: "Deploy to staging…" }));
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "Back" }));
  await settle();
  expect(screen.getByRole("heading", { level: 1, name: "api" })).toBeInTheDocument();
  expect(within(envSection("staging")).getByRole("button", { name: "Deploy to staging…" })).toHaveFocus();
});

test("back from the deploy flow to a Deploy that is now off, focus goes to the checkout's heading", async () => {
  prepare.mockRejectedValue({ code: "local", message: "ssh failed" });
  await openCheckout(/^api/);
  fireEvent.click(within(envSection("staging")).getByRole("button", { name: "Deploy to staging…" }));
  await settle();
  entry.mockResolvedValue({ kind: "unavailable", reason: "This checkout has no .shipslip.toml." });
  fireEvent.click(screen.getByRole("button", { name: "Back" }));
  await settle();
  expect(screen.getByRole("heading", { level: 1, name: "api" })).toHaveFocus();
});
