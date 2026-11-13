import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import type {
  AliasCheck,
  Answers,
  CheckView,
  CheckoutReceipts,
  ConfigAdded,
  ConfigText,
  DeployEntry,
  FolderView,
  HostBlock,
  HostDraft,
  LiveRuns,
  LoadResult,
  SshAliases,
  TrustReview,
} from "./types";

const loadMap = vi.fn<() => Promise<LoadResult>>();
const pickFolder = vi.fn<() => Promise<FolderView>>();
const addCheckout = vi.fn<() => Promise<string>>();
const reopenSetup = vi.fn<(generation: number, row: number) => Promise<FolderView>>();
const renderConfig = vi.fn<(answers: Answers) => Promise<ConfigText>>();
const addWithConfig = vi.fn<() => Promise<ConfigAdded>>();
const sshAliases = vi.fn<() => Promise<SshAliases>>();
const hostBlock = vi.fn<(draft: HostDraft) => Promise<HostBlock>>();
const checkAlias = vi.fn<(draft: HostDraft) => Promise<AliasCheck>>();
const openSshConfig = vi.fn<() => Promise<void>>();
const copyText = vi.fn<(text: string) => Promise<void>>();
const checkConnection =
  vi.fn<(generation: number, row: number, env: string, onLines: (view: CheckView) => void) => Promise<CheckView>>();
const checkRendered = vi.fn<(onLines: (view: CheckView) => void) => Promise<CheckView>>();
const removeCheckout = vi.fn<(generation: number, row: number) => Promise<void>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    receiptsChanged: async () => false,
    compare: vi.fn(),
    loadReceipt: async () => ({ kind: "notFound" }),
    checkoutReceipts: async (): Promise<CheckoutReceipts> => ({ kind: "listed", generation: 1, row: 0, receipts: [], older: false }),
    copyText: (text: string) => copyText(text),
    onFocus: async () => () => {},
    receiptSeen: async () => {},
    pickFolder: () => pickFolder(),
    addCheckout: () => addCheckout(),
    reopenSetup: (...args: [number, number]) => reopenSetup(...args),
    renderConfig: (answers: Answers) => renderConfig(answers),
    addWithConfig: () => addWithConfig(),
    sshAliases: () => sshAliases(),
    hostBlock: (draft: HostDraft) => hostBlock(draft),
    checkAlias: (draft: HostDraft) => checkAlias(draft),
    openSshConfig: () => openSshConfig(),
    checkConnection: (...args: [number, number, string, (view: CheckView) => void]) => checkConnection(...args),
    checkRendered: (onLines: (view: CheckView) => void) => checkRendered(onLines),
    removeCheckout: (...args: [number, number]) => removeCheckout(...args),
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
if (fixtures.removableMap.kind !== "loaded") throw new Error("fixture");
/** Maps read after a removal: only shop, or only api, is left. */
const shopOnly: LoadResult = { ...fixtures.removableMap, generation: 2 };
const apiOnly: LoadResult = { ...fixtures.setupMap, generation: 2 };
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
  sshAliases.mockReset().mockResolvedValue(fixtures.sshAliases);
  hostBlock
    .mockReset()
    .mockImplementation(async (draft) =>
      draft.hostName.trim() === "" ? fixtures.hostBlocks.invalid : fixtures.hostBlocks.block,
    );
  checkAlias.mockReset().mockResolvedValue(fixtures.aliasChecks.differs);
  openSshConfig.mockReset().mockResolvedValue(undefined);
  copyText.mockReset().mockResolvedValue(undefined);
  checkConnection.mockReset().mockResolvedValue(fixtures.connectionChecks.pass);
  checkRendered.mockReset().mockResolvedValue(fixtures.connectionChecks.pass);
  removeCheckout.mockReset().mockResolvedValue(undefined);
});

async function settle() {
  await act(async () => {});
}

const sheet = () => screen.getByRole("dialog", { name: "Add project" });
const nothingAdded = "Nothing is added until you continue.";

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

/** Picks the crm folder and continues to the check; the map read after it lists crm. */
async function toCheck() {
  await openSheet();
  await chooseFolder();
  loadMap.mockResolvedValue(added);
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  await settle();
}

/** Past the check without running it. */
async function toApproval() {
  await toCheck();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
}

test("the sidebar's Add project opens the sheet on the folder step with its heading focused", async () => {
  await openSheet();
  expect(sheet()).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Choose the project's folder" })).toHaveFocus();
  const rail = within(sheet()).getByRole("navigation", { name: "Add project steps" });
  expect(within(rail).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["1Folder", "2Connection check", "3Approval"]);
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

test("Finish later on the folder step adds nothing, and the step says so", async () => {
  await openSheet();
  await chooseFolder();
  expect(within(sheet()).getByText(nothingAdded)).toBeInTheDocument();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Finish later" }));
  await settle();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(addCheckout).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Add project" })).toHaveFocus();
});

test("only the folder step says nothing is added", async () => {
  await toCheck();
  expect(within(sheet()).queryByText(nothingAdded)).toBeNull();
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

test("until the map lists the added checkout, the check and approval say they are waiting", async () => {
  await openSheet();
  await chooseFolder();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(within(sheet()).getByRole("status")).toHaveTextContent("Waiting for the map to list this checkout…");
  expect(within(sheet()).getByRole("button", { name: "Check connection" })).toBeDisabled();
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

/** Answers the questions and continues to the ssh alias step. */
async function toAlias() {
  await toQuestions();
  fireEvent.change(within(sheet()).getByRole("textbox", { name: "Remote path" }), {
    target: { value: "/var/www/api" },
  });
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
}

/** Picks staging1 and continues to the connection check. */
async function toNewCheck() {
  await toAlias();
  fireEvent.click(within(sheet()).getByRole("radio", { name: /^staging1/ }));
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
}

async function toConfig() {
  await toNewCheck();
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
  expect(railSteps()).toEqual(["Folder", "2Environment", "3SSH alias", "4Connection check", "5Approval"]);
  expect(screen.getByRole("heading", { name: "Describe one environment" })).toHaveFocus();
  const field = (name: string) => within(sheet()).getByRole("textbox", { name });
  expect(field("Environment name")).toHaveValue("staging");
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
  // The checkout is already listed, so the folder step's note would be wrong.
  expect(within(sheet()).queryByText(nothingAdded)).toBeNull();
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
  expect(within(sheet()).queryByText(nothingAdded)).toBeNull();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Finish later" }));
  await settle();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(renderConfig).not.toHaveBeenCalled();
  expect(addWithConfig).not.toHaveBeenCalled();
});

test("Finish later on the ssh alias step closes the sheet and the checkout stays listed", async () => {
  loadMap.mockResolvedValueOnce(fixtures.map).mockResolvedValue(apiOnly);
  await toAlias();
  expect(screen.getByRole("heading", { name: "Pick the ssh alias" })).toBeInTheDocument();
  expect(within(sheet()).queryByText(nothingAdded)).toBeNull();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Finish later" }));
  await settle();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(addWithConfig).not.toHaveBeenCalled();
  const projects = screen.getByRole("region", { name: "Projects" });
  expect(within(projects).getByRole("button", { name: /^api/ })).toHaveTextContent("Setup not finished");
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
  expect(within(sheet()).getByRole("textbox", { name: "Remote path" })).toHaveValue("");
  expect(screen.queryByRole("heading", { level: 1, name: "api" })).toBeNull();
});

test("a reopened checkout that has a config now goes on to its connection check", async () => {
  loadMap.mockResolvedValue(fixtures.setupMap);
  reopenSetup.mockResolvedValue(fixtures.folders.config);
  render(<App />);
  await settle();
  fireEvent.click(within(screen.getByRole("region", { name: "Projects" })).getByRole("button", { name: /^api/ }));
  await settle();
  expect(screen.getByRole("heading", { name: "Check the connection" })).toHaveFocus();
  expect(addCheckout).not.toHaveBeenCalled();
});

/** Clicks api's "Setup not finished" line; api has nothing recorded. */
async function reopenSetupNotFinished() {
  loadMap.mockResolvedValueOnce(fixtures.setupMap).mockResolvedValue(shopOnly);
  render(<App />);
  await settle();
  fireEvent.click(within(screen.getByRole("region", { name: "Projects" })).getByRole("button", { name: /^api/ }));
  await settle();
}

test("a reopened sheet for a checkout with nothing recorded removes it from the sidebar", async () => {
  await reopenSetupNotFinished();
  const remove = within(sheet()).getByRole("button", { name: "Remove from sidebar" });
  expect(remove).not.toHaveClass("button-primary");
  fireEvent.click(remove);
  await settle();
  expect(removeCheckout).toHaveBeenCalledWith(1, 0);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(loadMap).toHaveBeenCalledTimes(2);
  expect(screen.getByRole("heading", { level: 1, name: "All projects" })).toHaveFocus();
  const projects = screen.getByRole("region", { name: "Projects" });
  expect(within(projects).queryByRole("button", { name: /^api/ })).toBeNull();
});

test("a reopened Needs approval sheet over its pane removes it and returns to All projects", async () => {
  loadMap.mockResolvedValueOnce(fixtures.removableMap).mockResolvedValue(apiOnly);
  reopenSetup.mockResolvedValue(fixtures.folders.config);
  render(<App />);
  await settle();
  fireEvent.click(within(screen.getByRole("region", { name: "Projects" })).getByRole("button", { name: /^shop/ }));
  await settle();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Remove from sidebar" }));
  await settle();
  expect(removeCheckout).toHaveBeenCalledWith(1, 0);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.getByRole("heading", { level: 1, name: "All projects" })).toHaveFocus();
  const projects = screen.getByRole("region", { name: "Projects" });
  expect(within(projects).queryByRole("button", { name: /^shop/ })).toBeNull();
});

/** Reopens api's sheet and writes its config; approving fails, so the
 * sheet stays open and reads `after` as the map. */
async function writeFromReopened(after: LoadResult) {
  addWithConfig.mockResolvedValue({ checkout: api, approvalError: "invalid trust store `trust.json`" });
  loadMap.mockResolvedValue(fixtures.setupMap);
  render(<App />);
  await settle();
  fireEvent.click(within(screen.getByRole("region", { name: "Projects" })).getByRole("button", { name: /^api/ }));
  await settle();
  fireEvent.change(within(sheet()).getByRole("textbox", { name: "Remote path" }), {
    target: { value: "/var/www/api" },
  });
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  fireEvent.click(within(sheet()).getByRole("radio", { name: /^staging1/ }));
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  fireEvent.click(within(sheet()).getByRole("checkbox", { name: /I read this/ }));
  typeEnv("staging");
  loadMap.mockResolvedValue(after);
  fireEvent.click(within(sheet()).getByRole("button", { name: "Approve and add" }));
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("could not approve it");
}

test("Remove from a reopened sheet uses the map the sheet refreshed", async () => {
  await writeFromReopened(apiAdded);
  fireEvent.click(within(sheet()).getByRole("button", { name: "Remove from sidebar" }));
  await settle();
  expect(removeCheckout).toHaveBeenCalledWith(2, 0);
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("Remove leaves a reopened sheet once the refreshed map has something recorded", async () => {
  if (apiAdded.kind !== "loaded") throw new Error("fixture");
  const recorded: LoadResult = {
    ...apiAdded,
    map: { ...apiAdded.map, rows: apiAdded.map.rows.map((row) => ({ ...row, removable: false })) },
  };
  await writeFromReopened(recorded);
  expect(within(sheet()).queryByRole("button", { name: "Remove from sidebar" })).toBeNull();
});

test("a refused removal from the sheet says why and keeps the sheet open", async () => {
  removeCheckout.mockRejectedValue({
    code: "local",
    message: "This Mac has receipts or approvals for this checkout, so it stays in the sidebar.",
  });
  await reopenSetupNotFinished();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Remove from sidebar" }));
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent(
    "This Mac has receipts or approvals for this checkout, so it stays in the sidebar.",
  );
  expect(within(sheet()).getByRole("button", { name: "Remove from sidebar" })).toBeEnabled();
  expect(loadMap).toHaveBeenCalledTimes(1);
});

test("a fresh Add project sheet offers no Remove from sidebar", async () => {
  loadMap.mockResolvedValue(fixtures.setupMap);
  render(<App />);
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "Add project" }));
  await settle();
  expect(within(sheet()).queryByRole("button", { name: "Remove from sidebar" })).toBeNull();
});

test("a reopened checkout with something recorded offers no Remove from sidebar", async () => {
  await reopenNeedsApproval();
  expect(within(sheet()).queryByRole("button", { name: "Remove from sidebar" })).toBeNull();
});

/** Clicks crm's "Needs approval" line on the map read after it was added. */
async function reopenNeedsApproval(folder: FolderView = fixtures.folders.config) {
  loadMap.mockResolvedValue(fixtures.addedMap);
  reopenSetup.mockResolvedValue(folder);
  render(<App />);
  await settle();
  const line = within(screen.getByRole("region", { name: "Projects" })).getByRole("button", { name: /^crm/ });
  expect(line).toHaveTextContent("Needs approval");
  fireEvent.click(line);
  await settle();
}

test("a Needs approval checkout opens the sheet at the connection check over its pane", async () => {
  await reopenNeedsApproval();
  expect(reopenSetup).toHaveBeenCalledWith(1, crmRow);
  expect(pickFolder).not.toHaveBeenCalled();
  expect(addCheckout).not.toHaveBeenCalled();
  expect(screen.getByRole("heading", { name: "Check the connection" })).toHaveFocus();
  expect(within(sheet()).getByRole("combobox", { name: "Environment" })).toHaveValue("production");
  expect(railSteps()).toEqual(["Folder", "2Connection check", "3Approval"]);
  const rail = within(sheet()).getByRole("navigation", { name: "Add project steps" });
  const folder = within(rail).getByRole("button", { name: /Folder/ });
  expect(within(folder).getByRole("img", { name: "done" })).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1, name: "crm" })).toBeInTheDocument();

  await runCheck();
  expect(checkConnection).toHaveBeenCalledWith(1, crmRow, "production", expect.any(Function));
});

test("a reopened config checks the first environment that needs approval, not the first listed", async () => {
  const config = fixtures.folders.config;
  if (config.kind !== "config") throw new Error("fixture");
  await reopenNeedsApproval({ ...config, envs: [...config.envs].reverse() });
  expect(within(sheet()).getByRole("combobox", { name: "Environment" })).toHaveValue("production");
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(screen.getByRole("heading", { name: "Approve production" })).toHaveFocus();
});

test("a Needs approval reopen that fails says why in the sheet and takes focus from the pane", async () => {
  reopenSetup.mockRejectedValue({ code: "stale", message: "The map was read again since this was selected." });
  loadMap.mockResolvedValue(fixtures.addedMap);
  render(<App />);
  await settle();
  fireEvent.click(within(screen.getByRole("region", { name: "Projects" })).getByRole("button", { name: /^crm/ }));
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("The map was read again");
  expect(screen.getByRole("heading", { name: "Choose the project's folder" })).toHaveFocus();
});

test("a reopened Needs approval checkout approves from the sheet and lands on its pane", async () => {
  await reopenNeedsApproval();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(screen.getByRole("heading", { name: "Approve production" })).toHaveFocus();
  expect(reviewTrust).toHaveBeenCalledWith(1, crmRow, "production");
  typeEnv("production");
  fireEvent.click(within(sheet()).getByRole("button", { name: "Approve and add" }));
  await settle();
  await settle();
  expect(approveTrust).toHaveBeenCalledWith(fixtures.trustReview.handle, "production");
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.getByRole("heading", { level: 1, name: "crm" })).toHaveFocus();
});

test.each(["check", "approval"])(
  "Finish later on a reopened checkout's %s step writes nothing and leaves its pane",
  async (at) => {
    await reopenNeedsApproval();
    if (at === "approval") {
      fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
      await settle();
      expect(screen.getByRole("heading", { name: "Approve production" })).toBeInTheDocument();
    }
    fireEvent.click(within(sheet()).getByRole("button", { name: "Finish later" }));
    await settle();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(approveTrust).not.toHaveBeenCalled();
    expect(addCheckout).not.toHaveBeenCalled();
    expect(addWithConfig).not.toHaveBeenCalled();
    const projects = screen.getByRole("region", { name: "Projects" });
    expect(within(projects).getByRole("button", { name: /^crm/ })).toHaveTextContent("Needs approval");
    expect(screen.getByRole("heading", { level: 1, name: "crm" })).toHaveFocus();
  },
);

test.each(["check", "approval"])("Finish later on a new project's %s step closes the sheet", async (at) => {
  await toCheck();
  if (at === "approval") {
    fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
    await settle();
  }
  fireEvent.click(within(sheet()).getByRole("button", { name: "Finish later" }));
  await settle();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(approveTrust).not.toHaveBeenCalled();
  expect(addWithConfig).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Add project" })).toHaveFocus();
});

test("done steps on a new config's rail are ticked and go back", async () => {
  await toNewCheck();
  const rail = within(sheet()).getByRole("navigation", { name: "Add project steps" });
  for (const name of [/Folder/, /Environment/, /SSH alias/]) {
    expect(within(within(rail).getByRole("button", { name })).getByRole("img", { name: "done" })).toBeInTheDocument();
  }
  expect(within(rail).queryByRole("button", { name: /Connection check/ })).toBeNull();
  fireEvent.click(within(rail).getByRole("button", { name: /SSH alias/ }));
  await settle();
  expect(screen.getByRole("heading", { name: "Pick the ssh alias" })).toHaveFocus();
});

// The ssh alias step: pick from ~/.ssh/config, or paste a Host block.

const continueButton = () => within(sheet()).getByRole("button", { name: "Continue" });
const hostField = (name: string) => within(sheet()).getByRole("textbox", { name });

async function noneFit() {
  await toAlias();
  fireEvent.click(within(sheet()).getByRole("radio", { name: "None of these fit" }));
  await settle();
  fireEvent.change(hostField("HostName"), { target: { value: "203.0.113.7" } });
  fireEvent.change(hostField("User"), { target: { value: "forge" } });
  await settle();
}

async function checkAgain() {
  fireEvent.click(within(sheet()).getByRole("button", { name: "Check again" }));
  await settle();
}

test("the ssh alias step lists the config's aliases and Continue waits for a pick", async () => {
  await toAlias();
  expect(screen.getByRole("heading", { name: "Pick the ssh alias" })).toHaveFocus();
  expect(within(sheet()).queryByRole("textbox", { name: "SSH host alias" })).toBeNull();
  const group = within(sheet()).getByRole("radiogroup", { name: "SSH alias" });
  const radios = within(group).getAllByRole("radio");
  expect(radios.map((r) => r.closest("label")?.textContent)).toEqual([
    "production1deploy@203.0.113.10",
    "staging1deploy@203.0.113.20",
    "None of these fit",
  ]);
  expect(continueButton()).toBeDisabled();

  fireEvent.click(within(group).getByRole("radio", { name: /^staging1/ }));
  expect(continueButton()).toBeEnabled();
  renderConfig.mockClear();
  fireEvent.click(continueButton());
  await settle();
  expect(renderConfig).toHaveBeenCalledWith(expect.objectContaining({ sshAlias: "staging1", path: "/var/www/api" }));
  expect(screen.getByRole("heading", { name: "Check the connection" })).toHaveFocus();
});

test("an alias ssh -G could not resolve still lists, and an unreadable config says why", async () => {
  sshAliases.mockResolvedValue({
    aliases: [{ alias: "broken", user: null, hostName: null }],
    identityFiles: [],
    error: "Couldn't read /Users/you/.ssh/config: Permission denied (os error 13)",
  });
  await toAlias();
  expect(within(sheet()).getByRole("radio", { name: /^broken/ }).closest("label")).toHaveTextContent(
    "ssh -G could not resolve it",
  );
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("Permission denied");
});

test("a failed read of the aliases leaves only None of these fit", async () => {
  sshAliases.mockRejectedValue({ code: "local", message: "HOME is not set." });
  await toAlias();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("HOME is not set.");
  expect(within(sheet()).getAllByRole("radio").map((r) => r.closest("label")?.textContent)).toEqual([
    "None of these fit",
  ]);
});

test("None of these fit asks for the server and shows the Host block with Copy and Open", async () => {
  await toAlias();
  fireEvent.click(within(sheet()).getByRole("radio", { name: "None of these fit" }));
  await settle();
  expect(hostField("Alias")).toHaveValue("api");
  expect(within(sheet()).getByRole("combobox", { name: "IdentityFile" })).toHaveValue("~/.ssh/id_ed25519");
  expect(within(sheet()).queryByRole("textbox", { name: "Port" })).toBeNull();
  expect(within(sheet()).queryByLabelText("Host block")).toBeNull();
  // An empty HostName is not flagged before anything is typed or checked.
  expect(hostField("HostName")).not.toHaveAttribute("aria-invalid");
  expect(within(sheet()).getByRole("button", { name: "Check again" })).toBeDisabled();

  fireEvent.click(within(sheet()).getByRole("button", { name: "More" }));
  expect(hostField("Port")).toHaveValue("22");

  fireEvent.change(hostField("HostName"), { target: { value: "203.0.113.7" } });
  fireEvent.change(hostField("User"), { target: { value: "forge" } });
  await settle();
  expect(hostBlock).toHaveBeenLastCalledWith({
    alias: "api",
    hostName: "203.0.113.7",
    user: "forge",
    identityFile: "~/.ssh/id_ed25519",
    port: "22",
  });
  const block = fixtures.hostBlocks.block;
  if (block.kind !== "block") throw new Error("fixture");
  expect(within(sheet()).getByLabelText("Host block").textContent).toBe(block.lines.join("\n"));

  fireEvent.click(within(sheet()).getByRole("button", { name: "Copy" }));
  await settle();
  expect(copyText).toHaveBeenCalledWith("Host api\n  HostName 203.0.113.7\n  User forge\n  IdentityFile ~/.ssh/id_ed25519\n");
  expect(within(sheet()).getByRole("button", { name: "Copied" })).toBeInTheDocument();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Open ~/.ssh/config" }));
  await settle();
  expect(openSshConfig).toHaveBeenCalledTimes(1);
  expect(continueButton()).toBeDisabled();
});

test("a field that can't go in the block is named under it", async () => {
  await noneFit();
  fireEvent.click(within(sheet()).getByRole("button", { name: "More" }));
  fireEvent.change(hostField("HostName"), { target: { value: "" } });
  fireEvent.change(hostField("Port"), { target: { value: "x" } });
  await settle();
  expect(hostField("Port")).toHaveAttribute("aria-invalid", "true");
  expect(hostField("Port")).toHaveAccessibleDescription("Use a port from 1 to 65535.");
  expect(hostField("HostName")).not.toHaveAttribute("aria-invalid");
  expect(within(sheet()).queryByLabelText("Host block")).toBeNull();
});

test("Check again says the alias does not match yet and Continue stays disabled", async () => {
  await noneFit();
  await checkAgain();
  expect(checkAlias).toHaveBeenCalledWith(expect.objectContaining({ alias: "api", hostName: "203.0.113.7" }));
  const status = within(sheet()).getByRole("status");
  expect(status).toHaveTextContent(
    "ssh -G api resolves to you@api, not forge@203.0.113.7 yet. Paste the block into ~/.ssh/config, save the file, then check again.",
  );
  expect(status).toHaveClass("tone-attention");
  expect(continueButton()).toBeDisabled();
});

test("Check again that matches enables Continue with the new alias", async () => {
  checkAlias.mockResolvedValue(fixtures.aliasChecks.matches);
  await noneFit();
  await checkAgain();
  const status = within(sheet()).getByRole("status");
  expect(status).toHaveTextContent("Found: ssh -G api resolves to forge@203.0.113.7.");
  expect(status).not.toHaveClass("tone-attention");
  expect(continueButton()).toBeEnabled();

  // Changing a field after the check needs another check.
  fireEvent.change(hostField("User"), { target: { value: "deploy" } });
  expect(continueButton()).toBeDisabled();
  expect(within(sheet()).queryByText(/^Found:/)).toBeNull();
  fireEvent.change(hostField("User"), { target: { value: "forge" } });
  expect(continueButton()).toBeEnabled();

  renderConfig.mockClear();
  fireEvent.click(continueButton());
  await settle();
  expect(renderConfig).toHaveBeenCalledWith(expect.objectContaining({ sshAlias: "api" }));
  expect(screen.getByRole("heading", { name: "Check the connection" })).toHaveFocus();
});

test("Check again that ssh can't run says what ssh printed", async () => {
  checkAlias.mockResolvedValue(fixtures.aliasChecks.failed);
  await noneFit();
  await checkAgain();
  expect(within(sheet()).getByRole("status")).toHaveTextContent("Bad configuration option: hostnme");
  expect(continueButton()).toBeDisabled();
});

test("Check again rejected by the app says why", async () => {
  checkAlias.mockRejectedValue({ code: "local", message: "the request failed inside the app" });
  await noneFit();
  await checkAgain();
  expect(within(sheet()).getByRole("status")).toHaveTextContent("the request failed inside the app");
  expect(continueButton()).toBeDisabled();
});

test("Check again refused for its fields names them", async () => {
  checkAlias.mockResolvedValue({ kind: "invalid", problems: [{ field: "user", message: "Use only letters." }] });
  await noneFit();
  await checkAgain();
  expect(hostField("User")).toHaveAccessibleDescription("Use only letters.");
  expect(continueButton()).toBeDisabled();
});

test("Open ~/.ssh/config that fails says why", async () => {
  openSshConfig.mockRejectedValue({ code: "notFound", message: "~/.ssh/config does not exist yet." });
  await noneFit();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Open ~/.ssh/config" }));
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("~/.ssh/config does not exist yet.");
});

test("Copy that fails says so", async () => {
  copyText.mockRejectedValue(new Error("denied"));
  await noneFit();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Copy" }));
  await settle();
  expect(within(sheet()).getByRole("button", { name: "Copy failed" })).toBeInTheDocument();
});

test("an alias the library refuses at Continue says why and keeps the step", async () => {
  await toAlias();
  fireEvent.click(within(sheet()).getByRole("radio", { name: /^staging1/ }));
  renderConfig.mockResolvedValue({
    kind: "invalid",
    problems: [{ field: "sshAlias", message: "Use the `Host` name from ~/.ssh/config, not user@host." }],
  });
  fireEvent.click(continueButton());
  await settle();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("not user@host");
  expect(screen.getByRole("heading", { name: "Pick the ssh alias" })).toBeInTheDocument();
});

test("going back to the ssh alias step keeps the pick", async () => {
  await toConfig();
  const rail = within(sheet()).getByRole("navigation", { name: "Add project steps" });
  fireEvent.click(within(rail).getByRole("button", { name: /SSH alias/ }));
  await settle();
  expect(screen.getByRole("heading", { name: "Pick the ssh alias" })).toHaveFocus();
  expect(within(sheet()).getByRole("radio", { name: /^staging1/ })).toBeChecked();
  expect(continueButton()).toBeEnabled();
  expect(sshAliases).toHaveBeenCalledTimes(1);
});

test("an environment answer refused at the ssh step's Continue goes back to its field", async () => {
  await toAlias();
  fireEvent.click(within(sheet()).getByRole("radio", { name: /^staging1/ }));
  renderConfig.mockResolvedValue(fixtures.configTexts.invalid);
  fireEvent.click(continueButton());
  await settle();
  expect(screen.getByRole("heading", { name: "Describe one environment" })).toBeInTheDocument();
  const env = within(sheet()).getByRole("textbox", { name: "Environment name" });
  expect(env).toHaveAttribute("aria-invalid", "true");
  expect(env).toHaveFocus();
});

test("a miss on User alone names the user ssh resolves to", async () => {
  checkAlias.mockResolvedValue({
    kind: "differs",
    resolved: { hostName: "203.0.113.7", user: "deploy", port: 22 },
  });
  await noneFit();
  await checkAgain();
  expect(within(sheet()).getByRole("status")).toHaveTextContent(
    "ssh -G api resolves to deploy@203.0.113.7, not forge@203.0.113.7 yet.",
  );
  expect(continueButton()).toBeDisabled();
});

test("a Host block the app could not build leaves Copy and Check again off", async () => {
  hostBlock.mockRejectedValue({ code: "local", message: "the request failed inside the app" });
  await noneFit();
  expect(within(sheet()).queryByLabelText("Host block")).toBeNull();
  expect(within(sheet()).getByRole("button", { name: "Copy" })).toBeDisabled();
  expect(within(sheet()).getByRole("button", { name: "Check again" })).toBeDisabled();
});

// The connection check step.

const checkList = () => within(sheet()).getByRole("list", { name: "Connection check" });
const checkLine = (label: string) => {
  const line = within(checkList())
    .getAllByRole("listitem")
    .find((li) => li.querySelector(".check-label")?.textContent === label);
  if (!line) throw new Error(`no line ${label}`);
  return line;
};
const reason = (label: string) => checkLine(label).querySelector(".check-reason");

async function runCheck() {
  fireEvent.click(within(sheet()).getByRole("button", { name: "Check connection" }));
  await settle();
}

test("an existing config's check is read only, runs only when asked, and every line passes", async () => {
  await toCheck();
  expect(screen.getByRole("heading", { name: "Check the connection" })).toHaveFocus();
  const banner = within(sheet()).getByText(/refresh its own cache files\.$/);
  expect(banner).toHaveTextContent(
    "Read only. Connects as production1 and looks. Shipslip's commands only read; starting php artisan can let Laravel refresh its own cache files.",
  );
  expect(banner).not.toHaveTextContent("nothing on the server changes");
  expect(checkConnection).not.toHaveBeenCalled();
  expect(within(sheet()).queryByRole("list", { name: "Connection check" })).toBeNull();

  await runCheck();
  expect(checkConnection).toHaveBeenCalledWith(2, crmRow, "production", expect.any(Function));
  const lines = within(checkList()).getAllByRole("listitem");
  expect(lines.map((li) => li.querySelector(".check-label")?.textContent)).toEqual([
    "Alias api-staging resolves",
    "ssh connects",
    "Remote path is a git checkout",
    "On branch main",
    "php artisan --version runs",
    "Laravel log readable",
  ]);
  for (const li of lines) {
    expect(within(li).getByRole("img", { name: "passed" })).toBeInTheDocument();
    expect(li.querySelector(".check-reason")).not.toHaveClass("tone-attention");
  }
  expect(reason("php artisan --version runs")).toHaveTextContent("Laravel Framework 11.9.2");
  expect(within(sheet()).getByRole("status")).toHaveTextContent("Connection check passed.");
  expect(within(sheet()).getByRole("button", { name: "Check again" })).toBeEnabled();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(screen.getByRole("heading", { name: "Approve production" })).toHaveFocus();
});

test("the check shows each line as it moves and waits to be asked again", async () => {
  let finish: (view: CheckView) => void = () => {};
  checkConnection.mockImplementation((_generation, _row, _env, onLines) => {
    onLines(fixtures.connectionChecks.running);
    return new Promise((resolve) => (finish = resolve));
  });
  await toCheck();
  await runCheck();
  expect(within(sheet()).getByRole("button", { name: "Checking…" })).toBeDisabled();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();
  expect(within(checkLine("Remote path is a git checkout")).getByRole("img", { name: "running" })).toBeInTheDocument();
  expect(within(checkLine("On branch main")).getByRole("img", { name: "waiting" })).toBeInTheDocument();
  await act(async () => finish(fixtures.connectionChecks.pass));
  expect(within(sheet()).getByRole("button", { name: "Check again" })).toBeEnabled();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeEnabled();
});

test("an unknown host key puts ssh <alias> true with Copy under its line, in amber", async () => {
  checkConnection.mockResolvedValue(fixtures.connectionChecks.unknownKey);
  await toCheck();
  await runCheck();
  const line = checkLine("ssh connects");
  expect(within(line).getByRole("img", { name: "did not pass" })).toBeInTheDocument();
  expect(line.querySelector(".check-reason")).toHaveClass("tone-attention");
  expect(line.querySelector(".check-reason")).toHaveTextContent(
    "This Mac has not seen the host key for api-staging. Run the command in Terminal, compare the fingerprint, answer yes, then check again.",
  );
  expect(within(line).getByText("ssh api-staging true")).toBeInTheDocument();
  fireEvent.click(within(line).getByRole("button", { name: "Copy ssh api-staging true" }));
  await settle();
  expect(copyText).toHaveBeenCalledWith("ssh api-staging true");
  for (const label of ["Remote path is a git checkout", "On branch main", "php artisan --version runs", "Laravel log readable"]) {
    expect(reason(label)).toHaveTextContent("Not run");
    expect(within(checkLine(label)).getByRole("img", { name: "not run" })).toBeInTheDocument();
  }
  expect(within(sheet()).queryByRole("alert")).toBeNull();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeEnabled();
});

test("a host key ssh refused without saying why is a boxed stop that never says changed, and Continue stays off", async () => {
  checkConnection.mockResolvedValue(fixtures.connectionChecks.unverifiedKey);
  await toCheck();
  await runCheck();
  const stop = within(sheet()).getByRole("alert");
  expect(
    within(stop).getByRole("heading", { name: "Shipslip can't verify the host key for production" }),
  ).toBeInTheDocument();
  expect(stop).toHaveTextContent(
    "ssh refused the host key for api-staging and did not say why, often because LogLevel in ~/.ssh/config is QUIET or FATAL. Find out why before going on.",
  );
  expect(stop).not.toHaveTextContent(/changed/i);
  expect(reason("ssh connects")).toHaveTextContent("Can't verify the host key");
  expect(within(sheet()).queryByRole("button", { name: /^Copy/ })).toBeNull();
  expect(within(checkList()).queryByText(/^ssh api-staging true$/)).toBeNull();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();
});

test("a changed host key is a boxed stop above the list with no command, and Continue stays off", async () => {
  checkConnection.mockResolvedValue(fixtures.connectionChecks.changedKey);
  await toCheck();
  await runCheck();
  const stop = within(sheet()).getByRole("alert");
  expect(within(stop).getByRole("heading", { name: "Host key changed for production. Shipslip stops here." })).toBeInTheDocument();
  expect(stop).toHaveTextContent(
    "The host key for api-staging is not the one ~/.ssh/known_hosts records. Find out why the key changed before going on.",
  );
  expect(stop.compareDocumentPosition(checkList()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(reason("ssh connects")).toHaveTextContent("Host key changed");
  expect(within(sheet()).queryByRole("button", { name: /^Copy/ })).toBeNull();
  expect(within(checkList()).queryByText(/^ssh api-staging true$/)).toBeNull();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();
  expect(within(sheet()).getByRole("button", { name: "Check again" })).toBeEnabled();

  checkConnection.mockResolvedValue(fixtures.connectionChecks.pass);
  fireEvent.click(within(sheet()).getByRole("button", { name: "Check again" }));
  await settle();
  expect(within(sheet()).queryByRole("alert")).toBeNull();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeEnabled();
});

test("a wrong branch is amber, never red, and lets the person go on to approval", async () => {
  checkConnection.mockResolvedValue(fixtures.connectionChecks.wrongBranch);
  await toCheck();
  await runCheck();
  const branch = reason("On branch main");
  expect(branch).toHaveTextContent("feature/search is checked out, not main");
  expect(branch).toHaveClass("tone-attention");
  expect(sheet().querySelector(".tone-bad, .mark-bad")).toBeNull();
  const icon = within(checkLine("On branch main")).getByRole("img", { name: "did not pass" });
  expect(icon).toHaveClass("step-icon-attention", "mark-attention");
  expect(checkLine("On branch main").querySelector(".step-icon-unknown")).toBeNull();
  expect(within(sheet()).getByRole("status")).toHaveTextContent("Connection check done: 1 did not pass.");
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(screen.getByRole("heading", { name: "Approve production" })).toHaveFocus();
});

test("a check the app could not start says why and offers Check again", async () => {
  checkConnection.mockRejectedValue({ code: "local", message: "production is no longer in .shipslip.toml." });
  await toCheck();
  await runCheck();
  expect(within(sheet()).getByRole("alert")).toHaveTextContent("production is no longer in .shipslip.toml.");
  expect(within(sheet()).getByRole("button", { name: "Check again" })).toBeEnabled();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeEnabled();
});

test("an existing config with two environments checks the one picked, and a new pick forgets the result", async () => {
  await toCheck();
  const picker = within(sheet()).getByRole("combobox", { name: "Environment" });
  expect(picker).toHaveValue("production");
  await runCheck();
  expect(within(sheet()).getByRole("list", { name: "Connection check" })).toBeInTheDocument();
  fireEvent.change(picker, { target: { value: "staging" } });
  expect(within(sheet()).queryByRole("list", { name: "Connection check" })).toBeNull();
  expect(within(sheet()).getByText(/refresh its own cache files\.$/)).toHaveTextContent("Connects as staging1");
  await runCheck();
  expect(checkConnection).toHaveBeenLastCalledWith(2, crmRow, "staging", expect.any(Function));
});

test("a new config's check runs on the answers just given, before anything is written", async () => {
  await toNewCheck();
  expect(railSteps()).toEqual(["Folder", "Environment", "SSH alias", "4Connection check", "5Approval"]);
  expect(within(sheet()).queryByRole("combobox", { name: "Environment" })).toBeNull();
  expect(within(sheet()).getByText(/refresh its own cache files\.$/)).toHaveTextContent("Connects as staging1");
  await runCheck();
  expect(checkRendered).toHaveBeenCalledTimes(1);
  expect(checkConnection).not.toHaveBeenCalled();
  expect(addWithConfig).not.toHaveBeenCalled();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(screen.getByRole("heading", { name: "Approve staging" })).toHaveFocus();
});

test("changing the answers after a check forgets its result", async () => {
  await toNewCheck();
  await runCheck();
  expect(within(sheet()).getByRole("list", { name: "Connection check" })).toBeInTheDocument();
  fireEvent.click(within(sheet()).getByRole("button", { name: /SSH alias/ }));
  await settle();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(within(sheet()).queryByRole("list", { name: "Connection check" })).toBeNull();
  expect(within(sheet()).getByRole("button", { name: "Check connection" })).toBeEnabled();
});

// The approval step: the config and the check result side by side.

const checkResult = () => within(sheet()).getByRole("region", { name: "Connection check" });
const resultLine = (label: string) => {
  const line = within(checkResult())
    .getAllByRole("listitem")
    .find((li) => li.querySelector(".check-summary-label")?.textContent === label);
  if (!line) throw new Error(`no result line ${label}`);
  return line;
};

test("an existing config's approval shows the check result beside its settings", async () => {
  await toCheck();
  await runCheck();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(screen.getByRole("heading", { name: "Approve production" })).toHaveFocus();
  const settings = within(sheet()).getByRole("region", { name: "Settings" });
  const result = checkResult();
  expect(settings.parentElement).toBe(result.parentElement);
  expect(within(result).getAllByRole("listitem")).toHaveLength(6);
  expect(within(resultLine("ssh connects")).getByRole("img", { name: "passed" })).toBeInTheDocument();
  expect(within(result).queryByText(/not run/i)).toBeNull();
});

test("a new config's approval shows the check result beside the rendered file", async () => {
  await toNewCheck();
  await runCheck();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  const config = within(sheet()).getByLabelText(".shipslip.toml");
  const result = checkResult();
  expect(config.parentElement).toBe(result.parentElement);
  expect(within(resultLine("Laravel log readable")).getByRole("img", { name: "passed" })).toBeInTheDocument();
});

test("a wrong branch reaches approval with its line shown as not passed", async () => {
  checkConnection.mockResolvedValue(fixtures.connectionChecks.wrongBranch);
  await toCheck();
  await runCheck();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  const branch = resultLine("On branch main");
  expect(within(branch).getByRole("img", { name: "did not pass" })).toBeInTheDocument();
  expect(branch.querySelector(".check-summary-reason")).toHaveTextContent("feature/search is checked out, not main");
  expect(branch.querySelector(".check-summary-reason")).toHaveClass("tone-attention");
  expect(branch.querySelector(".tone-bad")).toBeNull();
  expect(within(resultLine("ssh connects")).getByRole("img", { name: "passed" })).toBeInTheDocument();
});

test("an approval without a check run says the check was not run", async () => {
  await toApproval();
  const result = checkResult();
  expect(within(result).queryByRole("list")).toBeNull();
  expect(result).toHaveTextContent("Not run. Shipslip did not check the connection for production.");
});

test("a new config approved without a check says the check was not run", async () => {
  await toConfig();
  expect(checkResult()).toHaveTextContent("Not run. Shipslip did not check the connection for staging.");
});

test("a changed host key on another environment stays on screen while it keeps the sheet from approval", async () => {
  checkConnection.mockResolvedValue(fixtures.connectionChecks.changedKey);
  await toCheck();
  await runCheck();
  fireEvent.change(within(sheet()).getByRole("combobox", { name: "Environment" }), { target: { value: "staging" } });
  const stop = within(sheet()).getByRole("alert");
  expect(
    within(stop).getByRole("heading", { name: "Host key changed for production. Shipslip stops here." }),
  ).toBeInTheDocument();
  expect(stop).toHaveTextContent("The host key for api-staging is not the one ~/.ssh/known_hosts records.");
  expect(within(sheet()).queryByRole("list", { name: "Connection check" })).toBeNull();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();
});

test("a host key stop on each of two environments shows a box for each", async () => {
  checkConnection.mockResolvedValue(fixtures.connectionChecks.changedKey);
  await toCheck();
  await runCheck();
  fireEvent.change(within(sheet()).getByRole("combobox", { name: "Environment" }), { target: { value: "staging" } });
  checkConnection.mockResolvedValue(fixtures.connectionChecks.unverifiedKey);
  await runCheck();
  const stops = within(sheet()).getAllByRole("alert");
  expect(stops.map((stop) => within(stop).getByRole("heading").textContent)).toEqual([
    "Host key changed for production. Shipslip stops here.",
    "Shipslip can't verify the host key for staging",
  ]);
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();
});

test("a host key stop shows while its check is still running, and Continue stays off", async () => {
  checkConnection.mockImplementation((_generation, _row, _env, onLines) => {
    onLines({ ...fixtures.connectionChecks.changedKey, finished: false });
    return new Promise<CheckView>(() => {});
  });
  await toCheck();
  await runCheck();
  expect(
    within(within(sheet()).getByRole("alert")).getByRole("heading", {
      name: "Host key changed for production. Shipslip stops here.",
    }),
  ).toBeInTheDocument();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeDisabled();
});

test("a host key stop on one environment that passes when checked again is gone", async () => {
  checkConnection.mockResolvedValue(fixtures.connectionChecks.changedKey);
  await toCheck();
  await runCheck();
  checkConnection.mockResolvedValue(fixtures.connectionChecks.pass);
  fireEvent.click(within(sheet()).getByRole("button", { name: "Check again" }));
  await settle();
  expect(within(sheet()).queryByRole("alert")).toBeNull();
  expect(within(sheet()).getByRole("button", { name: "Continue" })).toBeEnabled();
});

test("a check started again forgets the result before it, and one that fails reads did not finish", async () => {
  await toCheck();
  await runCheck();
  checkConnection.mockRejectedValue({ code: "local", message: "production is no longer in .shipslip.toml." });
  fireEvent.click(within(sheet()).getByRole("button", { name: "Check again" }));
  await settle();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(within(checkResult()).queryByRole("list")).toBeNull();
  expect(checkResult()).toHaveTextContent(
    "Did not finish. Check the connection for production again from the check step.",
  );
});

test("a check that ends with lines still waiting is not shown as a result", async () => {
  checkConnection.mockResolvedValue(fixtures.connectionChecks.running);
  await toCheck();
  await runCheck();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(within(checkResult()).queryByRole("list")).toBeNull();
  expect(checkResult()).toHaveTextContent("Did not finish.");
});

test("an unreadable log reaches approval with its line shown as not passed", async () => {
  const pass = fixtures.connectionChecks.pass;
  const unreadable: CheckView = {
    ...pass,
    lines: pass.lines.map((line) =>
      line.label === "Laravel log readable"
        ? { ...line, state: "fail", reason: "storage/logs/laravel.log is not readable" }
        : line,
    ),
  };
  checkConnection.mockResolvedValue(unreadable);
  await toCheck();
  await runCheck();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  const log = resultLine("Laravel log readable");
  expect(within(log).getByRole("img", { name: "did not pass" })).toBeInTheDocument();
  expect(log.querySelector(".check-summary-reason")).toHaveTextContent("storage/logs/laravel.log is not readable");
});

test("the result shown is the check of the environment being approved", async () => {
  const config = fixtures.folders.config;
  if (config.kind !== "config") throw new Error("fixture");
  pickFolder.mockResolvedValue({ ...config, envs: config.envs.map((e) => ({ ...e, approved: false })) });
  await toCheck();
  fireEvent.change(within(sheet()).getByRole("combobox", { name: "Environment" }), { target: { value: "staging" } });
  await runCheck();
  fireEvent.click(within(sheet()).getByRole("button", { name: "Continue" }));
  await settle();
  expect(screen.getByRole("heading", { name: /^Approve / })).toHaveTextContent(
    `Approve ${config.envs[0].name}`,
  );
  expect(checkResult()).toHaveTextContent(`Not run. Shipslip did not check the connection for ${config.envs[0].name}.`);
});
