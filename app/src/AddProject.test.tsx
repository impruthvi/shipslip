import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import type { CheckoutReceipts, DeployEntry, FolderView, LiveRuns, LoadResult, TrustReview } from "./types";

const loadMap = vi.fn<() => Promise<LoadResult>>();
const pickFolder = vi.fn<() => Promise<FolderView>>();
const addCheckout = vi.fn<() => Promise<string>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    receiptsChanged: async () => false,
    compare: vi.fn(),
    loadReceipt: async () => ({ kind: "notFound" }),
    checkoutReceipts: async (): Promise<CheckoutReceipts> => ({ kind: "listed", generation: 1, row: 0, receipts: [] }),
    copyText: vi.fn(),
    onFocus: async () => () => {},
    receiptSeen: async () => {},
    pickFolder: () => pickFolder(),
    addCheckout: () => addCheckout(),
  },
}));

const reviewTrust = vi.fn<(generation: number, row: number, env: string) => Promise<TrustReview>>();
const approveTrust = vi.fn<(handle: string, typedEnv: string) => Promise<void>>();
vi.mock("./deployApi", () => ({
  deploy: {
    entry: async (): Promise<DeployEntry> => ({ kind: "unavailable", reason: "Needs approval." }),
    prepare: vi.fn(),
    confirm: vi.fn(),
    cancel: async () => {},
    follow: async () => () => {},
    stop: vi.fn(),
    reviewTrust: (...args: [number, number, string]) => reviewTrust(...args),
    approveTrust: (...args: [string, string]) => approveTrust(...args),
    openConfig: async () => {},
    openFolder: async () => {},
    reviewUp: vi.fn(),
    reviewAttach: vi.fn(),
    liveRuns: async (): Promise<LiveRuns> => ({ runs: [], closedPreviews: [] }),
  },
}));

const crm = "/Users/you/code/crm";
if (fixtures.addedMap.kind !== "loaded") throw new Error("fixture");
/** The map read after the add: newer than the map shown before it. */
const added: LoadResult = { ...fixtures.addedMap, generation: 2 };
const crmRow = fixtures.addedMap.map.rows.findIndex((r) => r.repoRoot === crm);

beforeEach(() => {
  localStorage.clear();
  loadMap.mockReset().mockResolvedValue(fixtures.map);
  pickFolder.mockReset().mockResolvedValue(fixtures.folders.config);
  addCheckout.mockReset().mockResolvedValue(crm);
  reviewTrust.mockReset().mockResolvedValue(fixtures.trustReview);
  approveTrust.mockReset().mockResolvedValue(undefined);
});

async function settle() {
  await act(async () => {});
}

const sheet = () => screen.getByRole("dialog", { name: "Add project" });

async function openSheet() {
  render(<App />);
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "Add project" }));
  await settle();
}

async function chooseFolder() {
  fireEvent.click(within(sheet()).getByRole("button", { name: "Choose folder…" }));
  await settle();
}

/** Picks the crm folder and continues; the map read after it lists crm. */
async function toApproval() {
  await openSheet();
  await chooseFolder();
  loadMap.mockResolvedValue(added);
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  await settle();
}

test("the sidebar's Add project opens the sheet on the folder step with its heading focused", async () => {
  await openSheet();
  expect(sheet()).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Choose the project's folder" })).toHaveFocus();
  const rail = within(sheet()).getByRole("navigation", { name: "Add project steps" });
  expect(within(rail).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["1Folder", "2Approval"]);
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();
});

test("a folder with .shipslip.toml shows its environments read only and asks no questions", async () => {
  await openSheet();
  await chooseFolder();
  const envs = within(sheet()).getByRole("region", { name: "Environments in .shipslip.toml" });
  const lines = within(envs).getAllByRole("listitem");
  expect(lines.map((line) => line.textContent)).toEqual([
    "productionproduction1:/var/www/crm · main · productionNeeds approval",
    "stagingstaging1:/var/www/crm · mainApproved",
  ]);
  expect(within(sheet()).queryByRole("textbox")).toBeNull();
  expect(within(sheet()).getByText(crm)).toHaveClass("sheet-path");
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeEnabled();
});

test("a folder outside git is refused with why, and cannot continue", async () => {
  pickFolder.mockResolvedValue(fixtures.folders.notGit);
  await openSheet();
  await chooseFolder();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent(
    "/Users/you/Downloads is not inside a git repository. Shipslip deploys from a git checkout",
  );
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();
  expect(addCheckout).not.toHaveBeenCalled();
});

test("a checkout without a config points to slip init; an unreadable one shows the error", async () => {
  pickFolder.mockResolvedValue(fixtures.folders.noConfig);
  await openSheet();
  await chooseFolder();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("This checkout has no .shipslip.toml yet. Run slip init");
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();

  pickFolder.mockResolvedValue(fixtures.folders.unreadable);
  fireEvent.click(within(sheet()).getByRole("button", { name: "Choose another folder…" }));
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("missing field `stack`");
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();
});

test("closing the folder panel keeps the folder picked before", async () => {
  await openSheet();
  await chooseFolder();
  pickFolder.mockResolvedValue(fixtures.folders.cancelled);
  fireEvent.click(within(sheet()).getByRole("button", { name: "Choose another folder…" }));
  await settle();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeEnabled();
});

test("Continue adds the checkout, then reviews only the environments without approval", async () => {
  await toApproval();
  expect(addCheckout).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("heading", { name: "Approve production" })).toHaveFocus();
  expect(reviewTrust).toHaveBeenCalledTimes(1);
  expect(reviewTrust).toHaveBeenCalledWith(2, crmRow, "production");
  const rail = within(sheet()).getByRole("navigation", { name: "Add project steps" });
  expect(within(rail).getByRole("button", { name: /Folder/ })).toBeInTheDocument();
  expect(within(sheet()).getByRole("region", { name: "Settings" })).toBeInTheDocument();
  expect(within(sheet()).getByRole("button", { name: "Approve and add" })).toBeDisabled();
});

test("Approve and add approves with the typed name and lands on the new checkout selected", async () => {
  await toApproval();
  fireEvent.change(within(sheet()).getByRole("textbox", { name: "Type production to approve" }), {
    target: { value: "production" },
  });
  fireEvent.click(within(sheet()).getByRole("button", { name: "Approve and add" }));
  await settle();
  await settle();
  expect(approveTrust).toHaveBeenCalledWith(fixtures.trustReview.handle, "production");
  expect(screen.queryByRole("dialog")).toBeNull();
  const projects = screen.getByRole("region", { name: "Projects" });
  expect(within(projects).getByRole("button", { name: /^crm/ })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("heading", { level: 1, name: "crm" })).toHaveFocus();
});

test("a failed approval stays on the step and says why", async () => {
  approveTrust.mockRejectedValue({ code: "local", message: "could not write trust.json" });
  await toApproval();
  fireEvent.change(within(sheet()).getByRole("textbox", { name: "Type production to approve" }), {
    target: { value: "production" },
  });
  fireEvent.click(within(sheet()).getByRole("button", { name: "Approve and add" }));
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("could not write trust.json");
  expect(screen.getByRole("dialog")).toBeInTheDocument();
});

test("Finish later after the folder step leaves the checkout listed as Needs approval", async () => {
  await toApproval();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Finish later" }));
  await settle();
  expect(screen.queryByRole("dialog")).toBeNull();
  const projects = screen.getByRole("region", { name: "Projects" });
  expect(within(projects).getByRole("button", { name: /^crm/ })).toHaveTextContent("Needs approval");
  expect(approveTrust).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Add project" })).toHaveFocus();
});

test("Escape closes the sheet like Finish later", async () => {
  await openSheet();
  fireEvent.keyDown(window, { key: "Escape" });
  await settle();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(addCheckout).not.toHaveBeenCalled();
});

test("first launch says there are no projects and offers Add project and slip init", async () => {
  loadMap.mockResolvedValue(fixtures.firstRun);
  render(<App />);
  await settle();
  const main = screen.getByRole("main");
  expect(within(main).getByRole("heading", { name: "No projects on this Mac yet." })).toBeInTheDocument();
  expect(within(main).getByText(/Shipslip reads its .shipslip.toml/)).toBeInTheDocument();
  expect(within(main).getByText("cd your-app && slip init")).toBeInTheDocument();
  expect(screen.queryByRole("grid")).toBeNull();
  fireEvent.click(within(main).getByRole("button", { name: "Add project" }));
  await settle();
  expect(sheet()).toBeInTheDocument();
});

test("with two environments to approve, each is typed in turn and the last one adds", async () => {
  const config = fixtures.folders.config;
  if (config.kind !== "config") throw new Error("fixture");
  pickFolder.mockResolvedValue({ ...config, envs: config.envs.map((env) => ({ ...env, approved: false })) });
  reviewTrust.mockImplementation(async (_generation, _row, env) => ({
    ...fixtures.trustReview,
    env,
    handle: `trust-${env}`,
  }));
  await toApproval();
  const approve = async (env: string, action: string) => {
    fireEvent.change(within(sheet()).getByRole("textbox", { name: `Type ${env} to approve` }), {
      target: { value: env },
    });
    fireEvent.click(within(sheet()).getByRole("button", { name: action }));
    await settle();
    await settle();
  };
  await approve("production", "Approve production");
  expect(approveTrust).toHaveBeenLastCalledWith("trust-production", "production");
  expect(screen.getByRole("heading", { name: "Approve staging" })).toHaveFocus();
  await approve("staging", "Approve and add");
  expect(approveTrust).toHaveBeenLastCalledWith("trust-staging", "staging");
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("a folder panel that fails says why and keeps the sheet open", async () => {
  pickFolder.mockRejectedValue({ code: "local", message: "App storage is unavailable." });
  await openSheet();
  await chooseFolder();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("App storage is unavailable.");
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();
});

test("an add that fails stays on the folder step and says why", async () => {
  addCheckout.mockRejectedValue({ code: "local", message: "could not write projects.json" });
  await openSheet();
  await chooseFolder();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("could not write projects.json");
  expect(screen.getByRole("heading", { name: "Choose the project's folder" })).toBeInTheDocument();
  expect(reviewTrust).not.toHaveBeenCalled();
});

test("a review that fails says why and Try again asks once more", async () => {
  reviewTrust.mockRejectedValueOnce({ code: "notFound", message: "This checkout or environment is no longer on the map." });
  await toApproval();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("no longer on the map");
  expect(within(sheet()).getByRole("button", { name: "Approve and add" })).toBeDisabled();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Try again" }));
  await settle();
  expect(reviewTrust).toHaveBeenCalledTimes(2);
  expect(within(sheet()).getByRole("region", { name: "Settings" })).toBeInTheDocument();
});

test("until the map lists the added checkout, approval says it is waiting", async () => {
  await openSheet();
  await chooseFolder();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(within(sheet()).getByRole("status")).toHaveTextContent("Waiting for the map to list this checkout…");
  expect(reviewTrust).not.toHaveBeenCalled();
  expect(within(sheet()).getByRole("button", { name: "Approve and add" })).toBeDisabled();
});

test("Escape waits while an approval is in flight", async () => {
  approveTrust.mockReturnValue(new Promise(() => {}));
  await toApproval();
  fireEvent.change(within(sheet()).getByRole("textbox", { name: "Type production to approve" }), {
    target: { value: "production" },
  });
  fireEvent.click(within(sheet()).getByRole("button", { name: "Approve and add" }));
  await settle();
  fireEvent.keyDown(window, { key: "Escape" });
  await settle();
  expect(screen.getByRole("dialog")).toBeInTheDocument();
});
