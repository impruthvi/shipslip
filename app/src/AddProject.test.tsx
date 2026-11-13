import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import type {
  Answers,
  CheckoutReceipts,
  ConfigAdded,
  ConfigText,
  DeployEntry,
  FolderView,
  LiveRuns,
  LoadResult,
  TrustReview,
} from "./types";

const loadMap = vi.fn<() => Promise<LoadResult>>();
const pickFolder = vi.fn<() => Promise<FolderView>>();
const addCheckout = vi.fn<() => Promise<string>>();
const reopenSetup = vi.fn<(generation: number, row: number) => Promise<FolderView>>();
const renderConfig = vi.fn<(answers: Answers) => Promise<ConfigText>>();
const addWithConfig = vi.fn<() => Promise<ConfigAdded>>();
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
    reopenSetup: (...args: [number, number]) => reopenSetup(...args),
    renderConfig: (answers: Answers) => renderConfig(answers),
    addWithConfig: () => addWithConfig(),
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
const api = "/Users/you/code/api";
if (fixtures.setupMap.kind !== "loaded") throw new Error("fixture");
/** The map read after the config is written: api is set up. */
const apiAdded: LoadResult = {
  ...fixtures.setupMap,
  generation: 2,
  map: {
    ...fixtures.setupMap.map,
    rows: fixtures.setupMap.map.rows.map((row) => ({ ...row, setupNotFinished: false, needsApproval: true })),
  },
};

beforeEach(() => {
  localStorage.clear();
  loadMap.mockReset().mockResolvedValue(fixtures.map);
  pickFolder.mockReset().mockResolvedValue(fixtures.folders.config);
  addCheckout.mockReset().mockResolvedValue(crm);
  reviewTrust.mockReset().mockResolvedValue(fixtures.trustReview);
  approveTrust.mockReset().mockResolvedValue(undefined);
  reopenSetup.mockReset().mockResolvedValue(fixtures.folders.newConfig);
  renderConfig.mockReset().mockResolvedValue(fixtures.configTexts.rendered);
  addWithConfig.mockReset().mockResolvedValue({ checkout: api, approvalError: null });
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

test("a checkout whose config can't be read shows the error and cannot continue", async () => {
  pickFolder.mockResolvedValue(fixtures.folders.unreadable);
  await openSheet();
  await chooseFolder();
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

// A new config: the slip init questions, then the rendered config.

const railSteps = () =>
  within(within(sheet()).getByRole("navigation", { name: "Add project steps" }))
    .getAllByRole("listitem")
    .map((li) => li.textContent);

/** Picks the api folder (no config) and continues to the questions. */
async function toQuestions(folder: FolderView = { ...fixtures.folders.newConfig, branch: "main" } as FolderView) {
  pickFolder.mockResolvedValue(folder);
  addCheckout.mockResolvedValue(api);
  await openSheet();
  await chooseFolder();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
}

async function toConfig() {
  await toQuestions();
  fireEvent.change(within(sheet()).getByRole("textbox", { name: "SSH host alias" }), {
    target: { value: "api-staging" },
  });
  fireEvent.change(within(sheet()).getByRole("textbox", { name: "Remote path" }), {
    target: { value: "/var/www/api" },
  });
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
}

const typeEnv = (env: string) =>
  fireEvent.change(within(sheet()).getByRole("textbox", { name: `Type ${env} to approve` }), {
    target: { value: env },
  });

test("a checkout without a config is added and asks the seven slip init questions", async () => {
  await toQuestions();
  expect(addCheckout).toHaveBeenCalledTimes(1);
  expect(railSteps()).toEqual(["Folder", "2Environment", "3Approval"]);
  expect(screen.getByRole("heading", { name: "Describe one environment" })).toHaveFocus();
  const field = (name: string) => within(sheet()).getByRole("textbox", { name });
  expect(field("Environment name")).toHaveValue("staging");
  expect(field("SSH host alias")).toHaveValue("");
  expect(field("Remote path")).toHaveValue("");
  expect(field("Branch")).toHaveValue("main");
  expect(within(sheet()).getByRole("checkbox", { name: /^Production/ })).not.toBeChecked();
  expect(within(sheet()).getByRole("checkbox", { name: /^Maintenance mode/ })).toBeChecked();
  expect(field("Smoke URL")).toHaveValue("");
  expect(renderConfig).not.toHaveBeenCalled();
});

test("answers that break the library's rule are named under their field and keep the step", async () => {
  renderConfig.mockResolvedValue(fixtures.configTexts.invalid);
  await toQuestions();
  const env = within(sheet()).getByRole("textbox", { name: "Environment name" });
  fireEvent.change(env, { target: { value: "env.staging" } });
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(renderConfig).toHaveBeenCalledWith(expect.objectContaining({ env: "env.staging", branch: "main" }));
  expect(env).toHaveAttribute("aria-invalid", "true");
  expect(env).toHaveAccessibleDescription("Use only letters, digits, `-` and `_`.");
  expect(env).toHaveFocus();
  const path = within(sheet()).getByRole("textbox", { name: "Remote path" });
  expect(path).toHaveAccessibleDescription("Use an absolute path, such as /var/www/app.");
  expect(screen.getByRole("heading", { name: "Describe one environment" })).toBeInTheDocument();

  fireEvent.change(env, { target: { value: "staging" } });
  expect(env).not.toHaveAttribute("aria-invalid");
  expect(env).toHaveAccessibleDescription("Letters, digits, - and _");
  expect(path).toHaveAttribute("aria-invalid", "true");
});

test("approval shows the rendered config and its warnings box blocks approval until ticked", async () => {
  await toConfig();
  const rendered = fixtures.configTexts.rendered;
  if (rendered.kind !== "rendered") throw new Error("fixture");
  expect(screen.getByRole("heading", { name: "Approve staging" })).toHaveFocus();
  expect(within(sheet()).getByLabelText(".shipslip.toml").textContent).toBe(rendered.lines.join("\n"));
  const warnings = within(sheet()).getByRole("region", { name: "Read before approving" });
  expect(rendered.warnings).toHaveLength(1);
  expect(warnings).toHaveTextContent(rendered.warnings[0]);

  typeEnv("staging");
  const approve = within(sheet()).getByRole("button", { name: "Approve and add" });
  expect(approve).toBeDisabled();
  fireEvent.click(within(warnings).getByRole("checkbox", { name: /I read this/ }));
  expect(approve).toBeEnabled();
});

test("a config without warnings needs only the typed name", async () => {
  const rendered = fixtures.configTexts.rendered;
  if (rendered.kind !== "rendered") throw new Error("fixture");
  renderConfig.mockResolvedValue({ ...rendered, warnings: [] });
  await toConfig();
  expect(within(sheet()).queryByRole("region", { name: "Read before approving" })).toBeNull();
  typeEnv("staging");
  expect(within(sheet()).getByRole("button", { name: "Approve and add" })).toBeEnabled();
});

test("Approve and add writes, approves and closes on the new checkout", async () => {
  await toConfig();
  fireEvent.click(within(sheet()).getByRole("checkbox", { name: /I read this/ }));
  typeEnv("staging");
  loadMap.mockResolvedValue(apiAdded);
  fireEvent.click(within(sheet()).getByRole("button", { name: "Approve and add" }));
  await settle();
  await settle();
  expect(addWithConfig).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("dialog")).toBeNull();
  const projects = screen.getByRole("region", { name: "Projects" });
  expect(within(projects).getByRole("button", { name: /^api/ })).toHaveAttribute("aria-current", "page");
  expect(screen.getByRole("heading", { level: 1, name: "api" })).toHaveFocus();
});

test("a failed approval after the write says so and leaves the checkout needing approval", async () => {
  addWithConfig.mockResolvedValue({ checkout: api, approvalError: "invalid trust store `trust.json`" });
  await toConfig();
  fireEvent.click(within(sheet()).getByRole("checkbox", { name: /I read this/ }));
  typeEnv("staging");
  const reads = loadMap.mock.calls.length;
  loadMap.mockResolvedValue(apiAdded);
  fireEvent.click(within(sheet()).getByRole("button", { name: "Approve and add" }));
  await settle();
  const alert = within(sheet()).getByRole("alert");
  expect(alert).toHaveTextContent("Shipslip wrote .shipslip.toml but could not approve it.");
  expect(alert).toHaveTextContent("“Needs approval”");
  expect(alert).toHaveTextContent("invalid trust store `trust.json`");
  expect(loadMap.mock.calls.length).toBeGreaterThan(reads);
  expect(within(sheet()).queryByRole("button", { name: "Approve and add" })).toBeNull();
  // The file is written: the answers can't be changed from here.
  const rail = within(sheet()).getByRole("navigation", { name: "Add project steps" });
  expect(within(rail).queryByRole("button")).toBeNull();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Open checkout" }));
  await settle();
  await settle();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.getByRole("heading", { level: 1, name: "api" })).toHaveFocus();
  const projects = screen.getByRole("region", { name: "Projects" });
  expect(within(projects).getByRole("button", { name: /^api/ })).toHaveTextContent("Needs approval");
});

test("a render that fails says why and stays on the questions", async () => {
  renderConfig.mockRejectedValue({ code: "local", message: "This checkout has a .shipslip.toml now." });
  await toQuestions();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("This checkout has a .shipslip.toml now.");
  expect(screen.getByRole("heading", { name: "Describe one environment" })).toBeInTheDocument();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeEnabled();
});

test("a reopen that fails says why on the folder step", async () => {
  loadMap.mockResolvedValue(fixtures.setupMap);
  reopenSetup.mockRejectedValue({ code: "stale", message: "The map was read again since this was selected." });
  render(<App />);
  await settle();
  fireEvent.click(within(screen.getByRole("region", { name: "Projects" })).getByRole("button", { name: /^api/ }));
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("The map was read again");
  expect(screen.getByRole("heading", { name: "Choose the project's folder" })).toBeInTheDocument();
  expect(within(sheet()).getByRole("button", { name: "Choose folder…" })).toBeEnabled();
});

test("a write that fails says why and can be tried again", async () => {
  addWithConfig.mockRejectedValueOnce({ code: "local", message: "This checkout has a .shipslip.toml now" });
  await toConfig();
  fireEvent.click(within(sheet()).getByRole("checkbox", { name: /I read this/ }));
  typeEnv("staging");
  fireEvent.click(within(sheet()).getByRole("button", { name: "Approve and add" }));
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("This checkout has a .shipslip.toml now");
  expect(within(sheet()).getByRole("button", { name: "Approve and add" })).toBeEnabled();
});

test("going back to the environment step keeps the answers", async () => {
  await toConfig();
  const rail = within(sheet()).getByRole("navigation", { name: "Add project steps" });
  fireEvent.click(within(rail).getByRole("button", { name: /Environment/ }));
  await settle();
  expect(screen.getByRole("heading", { name: "Describe one environment" })).toHaveFocus();
  expect(within(sheet()).getByRole("textbox", { name: "Remote path" })).toHaveValue("/var/www/api");
});

test("Finish later after the folder step leaves the checkout listed", async () => {
  await toQuestions();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Finish later" }));
  await settle();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(renderConfig).not.toHaveBeenCalled();
  expect(addWithConfig).not.toHaveBeenCalled();
});

test("a Setup not finished checkout says so and reopens the sheet at the environment step", async () => {
  loadMap.mockResolvedValue(fixtures.setupMap);
  render(<App />);
  await settle();
  const projects = screen.getByRole("region", { name: "Projects" });
  const line = within(projects).getByRole("button", { name: /^api/ });
  expect(line).toHaveTextContent("Setup not finished");
  expect(within(line).getByText("Setup not finished")).not.toHaveClass("tone-attention");

  fireEvent.click(line);
  await settle();
  expect(reopenSetup).toHaveBeenCalledWith(1, 0);
  expect(pickFolder).not.toHaveBeenCalled();
  expect(screen.getByRole("heading", { name: "Describe one environment" })).toHaveFocus();
  expect(within(sheet()).getByText(api)).toHaveClass("sheet-path");
  // Answers are not kept between sessions.
  expect(within(sheet()).getByRole("textbox", { name: "SSH host alias" })).toHaveValue("");
  expect(screen.queryByRole("heading", { level: 1, name: "api" })).toBeNull();
});

test("a reopened checkout that has a config now shows it on the folder step", async () => {
  loadMap.mockResolvedValue(fixtures.setupMap);
  reopenSetup.mockResolvedValue(fixtures.folders.config);
  render(<App />);
  await settle();
  fireEvent.click(within(screen.getByRole("region", { name: "Projects" })).getByRole("button", { name: /^api/ }));
  await settle();
  expect(screen.getByRole("heading", { name: "Choose the project's folder" })).toBeInTheDocument();
  expect(within(sheet()).getByRole("region", { name: "Environments in .shipslip.toml" })).toBeInTheDocument();
});
