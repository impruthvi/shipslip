import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import type {
  CommandError,
  DeployEntry,
  DeployReview,
  LiveRuns,
  LoadResult,
  PrepareProgress,
  RecoveryReview,
  RunBatch,
  RunInfo,
  TrustReview,
} from "./types";

const loadMap = vi.fn<() => Promise<LoadResult>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    compare: vi.fn(),
    loadReceipt: async () => ({ kind: "notFound" }),
    copyText: vi.fn(),
    onFocus: async () => () => {},
    receiptSeen: async () => {},
  },
}));

const entry = vi.fn<(generation: number, row: number, env: string) => Promise<DeployEntry>>();
const prepare = vi.fn<
  (
    generation: number,
    row: number,
    env: string,
    plan: unknown,
    onProgress: (phase: PrepareProgress) => void,
  ) => Promise<DeployReview>
>();
const confirm = vi.fn<(handle: string, typed: string | null) => Promise<RunInfo>>();
const cancel = vi.fn<(handle: string) => Promise<void>>();
const stop = vi.fn<(handle: string, what: string) => Promise<void>>();
const reviewTrust = vi.fn<(generation: number, row: number, env: string) => Promise<TrustReview>>();
const approveTrust = vi.fn<(handle: string, typed: string) => Promise<void>>();
const openConfig = vi.fn<(generation: number, row: number, env: string) => Promise<void>>();
const reviewUp = vi.fn<(generation: number, row: number, env: string) => Promise<RecoveryReview>>();
const reviewAttach = vi.fn<(generation: number, row: number, env: string) => Promise<RecoveryReview>>();
const liveRuns = vi.fn<() => Promise<LiveRuns>>();
let send: (batch: RunBatch) => void = () => {};
const unfollow = vi.fn();
vi.mock("./deployApi", () => ({
  deploy: {
    entry: (...args: [number, number, string]) => entry(...args),
    prepare: (...args: Parameters<typeof prepare>) => prepare(...args),
    confirm: (...args: [string, string | null]) => confirm(...args),
    cancel: (handle: string) => cancel(handle),
    follow: async (_handle: string, onBatch: (batch: RunBatch) => void) => {
      send = onBatch;
      return unfollow;
    },
    stop: (...args: [string, string]) => stop(...args),
    reviewTrust: (...args: [number, number, string]) => reviewTrust(...args),
    approveTrust: (...args: [string, string]) => approveTrust(...args),
    openConfig: (...args: [number, number, string]) => openConfig(...args),
    reviewUp: (...args: [number, number, string]) => reviewUp(...args),
    reviewAttach: (...args: [number, number, string]) => reviewAttach(...args),
    liveRuns: () => liveRuns(),
  },
}));

/** A promise and the means to settle it from the test. */
function later<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** A ready entry for the shop fixture's three steps. */
function ready(over: Partial<Extract<DeployEntry, { kind: "ready" }>> = {}): DeployEntry {
  return {
    kind: "ready",
    production: true,
    needsTrust: false,
    steps: fixtures.deployReview.steps,
    recovery: null,
    ...over,
  };
}

beforeEach(() => {
  localStorage.clear();
  loadMap.mockReset().mockResolvedValue(fixtures.map);
  entry.mockReset().mockResolvedValue(ready());
  prepare.mockReset();
  confirm.mockReset();
  cancel.mockReset().mockResolvedValue(undefined);
  stop.mockReset().mockResolvedValue(undefined);
  unfollow.mockReset();
  reviewTrust.mockReset();
  approveTrust.mockReset();
  openConfig.mockReset();
  reviewUp.mockReset();
  reviewAttach.mockReset();
  liveRuns.mockReset().mockResolvedValue({ runs: [], closedPreviews: [] });
});

async function settle() {
  await act(async () => {});
}

/** The launch selection's app and env, as the heading names them. */
function selectedCell() {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  const cell = screen
    .getAllByRole("gridcell")
    .find((c) => c.getAttribute("aria-selected") === "true")!;
  const { rows, envs } = fixtures.map.map;
  const row = Number(cell.dataset.row);
  return { row, env: envs[Number(cell.dataset.col)], label: rows[row].label };
}

async function openWorkspace() {
  render(<App />);
  await settle();
  const cell = selectedCell();
  const pending = later<DeployReview>();
  let progress: (phase: PrepareProgress) => void = () => {};
  prepare.mockImplementation((_g, _r, _e, _p, onProgress) => {
    progress = onProgress;
    return pending.promise;
  });
  fireEvent.click(screen.getByRole("button", { name: "Review deploy…" }));
  await settle();
  return { cell, pending, progress: (phase: PrepareProgress) => act(() => progress(phase)) };
}

const production = { ...fixtures.deployReview, env: "production" };

test("review deploy shows only for envs with a config, and says why it is off", async () => {
  entry.mockResolvedValue({ kind: "hidden" });
  const { unmount } = render(<App />);
  await settle();
  expect(entry).toHaveBeenCalledWith(1, selectedCell().row, selectedCell().env);
  expect(screen.queryByRole("button", { name: "Review deploy…" })).not.toBeInTheDocument();
  unmount();

  for (const [answer, reason] of [
    [{ kind: "unavailable", reason: "Checkout folder is missing" }, "Checkout folder is missing"],
    [{ kind: "running", handle: "run-1" }, "A deploy is running for this env"],
  ] as const) {
    entry.mockResolvedValue(answer);
    const view = render(<App />);
    await settle();
    const button = screen.getByRole("button", { name: "Review deploy…" });
    expect(button).toBeDisabled();
    expect(button).toHaveAccessibleDescription(reason);
    view.unmount();
  }
});

test("preparing names each phase, then cancel releases the lock and returns to the map", async () => {
  const { cell, pending, progress } = await openWorkspace();
  expect(screen.queryByRole("grid")).not.toBeInTheDocument();
  expect(screen.queryByRole("complementary", { name: "Receipt" })).not.toBeInTheDocument();
  const heading = screen.getByRole("heading", { level: 1 });
  expect(heading).toHaveTextContent(`Deploy ${cell.label} → ${cell.env}`);
  expect(heading).toHaveFocus();
  expect(screen.getByText("PRODUCTION")).toBeInTheDocument();
  expect(prepare).toHaveBeenCalledWith(1, cell.row, cell.env, "Deploy", expect.any(Function));

  progress({ phase: "connecting", alias: "shop-prod" });
  progress({ phase: "fetching", branch: "main" });
  const main = within(screen.getByRole("main"));
  expect(main.getByText("Connecting to shop-prod…")).toBeInTheDocument();
  expect(main.getByText("Fetching main…")).toBeInTheDocument();
  expect(screen.getByTestId("announce-polite")).toHaveTextContent("Fetching main…");

  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.getByText("Releasing the server lock…")).toBeInTheDocument();
  expect(cancel).not.toHaveBeenCalled();
  await act(async () => pending.resolve(production));
  expect(cancel).toHaveBeenCalledWith("review-1");
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Review deploy…" })).toHaveFocus();
});

test("escape cancels preparing; a prepare that fails after cancel just closes", async () => {
  const { pending } = await openWorkspace();
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.getByText("Releasing the server lock…")).toBeInTheDocument();
  await act(async () => pending.reject({ code: "connection", message: "gone" }));
  expect(cancel).not.toHaveBeenCalled();
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
});

test("a refusal shows its title, the command to copy and the raw error under Details", async () => {
  const { pending, progress } = await openWorkspace();
  progress({ phase: "connecting", alias: "shop-prod" });
  await act(async () => pending.reject(fixtures.refusal));
  const main = within(screen.getByRole("main"));
  expect(main.getByRole("heading", { level: 2, name: "Your ssh key isn't loaded" })).toBeInTheDocument();
  expect(main.getByText(/· production · shop-prod$/)).toBeInTheDocument();
  expect(screen.getByText("ssh-add --apple-use-keychain")).toBeInTheDocument();
  expect(main.getByText("Details").closest("details")).toHaveTextContent(fixtures.refusal.message);
  expect(screen.getByTestId("announce-assertive")).toHaveTextContent("Your ssh key isn't loaded");
  expect(screen.getByRole("heading", { level: 1 })).toHaveFocus();
  fireEvent.click(screen.getByRole("button", { name: "Back" }));
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
});

test("preview shows what ships; production deploys only once its name is typed", async () => {
  const { pending } = await openWorkspace();
  await act(async () => pending.resolve(production));
  expect(screen.getByText("aaaaaaa → bbbbbbb")).toBeInTheDocument();
  expect(screen.getByText("· 12 commits")).toBeInTheDocument();
  expect(screen.getByText("3 steps · maintenance on · log watch 2 min")).toBeInTheDocument();
  expect(screen.getByText("shop-prod · /var/www/shop")).toBeInTheDocument();
  expect(screen.getByText(/^Server lock held since \d\d:\d\d · Back releases it$/)).toBeInTheDocument();

  const commits = within(screen.getByRole("region", { name: "Commits" })).getAllByRole("listitem");
  expect(commits).toHaveLength(5);
  fireEvent.click(screen.getByRole("button", { name: "and 7 more" }));
  expect(within(screen.getByRole("region", { name: "Commits" })).getAllByRole("listitem")).toHaveLength(7);
  expect(screen.getByText("and 5 more")).toBeInTheDocument();
  const steps = within(screen.getByRole("region", { name: "Steps" })).getAllByRole("listitem");
  expect(steps.map((s) => s.textContent)).toEqual([
    "1composer install --no-dev",
    "2php artisan migrate --force",
    "3php artisan optimize",
  ]);

  const deploy = screen.getByRole("button", { name: "Deploy to production" });
  expect(deploy).toBeDisabled();
  const typed = screen.getByLabelText("Type production to deploy");
  fireEvent.change(typed, { target: { value: "Production" } });
  expect(deploy).toBeDisabled();
  fireEvent.change(typed, { target: { value: "production" } });
  expect(deploy).toBeEnabled();
  confirm.mockReturnValue(new Promise(() => {}));
  fireEvent.click(deploy);
  expect(confirm).toHaveBeenCalledWith("review-1", "production");
  expect(screen.getByRole("button", { name: "Starting…" })).toBeDisabled();
});

test("a non-production preview deploys without typing; nothing to ship hides Deploy", async () => {
  entry.mockResolvedValue(ready({ production: false }));
  const { pending } = await openWorkspace();
  await act(async () =>
    pending.resolve({ ...fixtures.deployReview, env: "staging", production: false }),
  );
  expect(screen.queryByText("PRODUCTION")).not.toBeInTheDocument();
  expect(screen.queryByLabelText(/^Type/)).not.toBeInTheDocument();
  confirm.mockReturnValue(new Promise(() => {}));
  fireEvent.click(screen.getByRole("button", { name: "Deploy to staging" }));
  expect(confirm).toHaveBeenCalledWith("review-1", null);
});

test("nothing to ship says so, offers no deploy, and Rerun swaps the preview for a rerun", async () => {
  const { pending } = await openWorkspace();
  await act(async () => pending.resolve({ ...production, commits: [], moreCommits: 0 }));
  expect(screen.getByText(/Already up to date/)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /^Deploy to/ })).not.toBeInTheDocument();

  const rerun = later<DeployReview>();
  prepare.mockReturnValue(rerun.promise);
  fireEvent.click(screen.getByRole("button", { name: "Rerun…" }));
  await settle();
  expect(cancel).toHaveBeenCalledWith("review-1");
  expect(prepare).toHaveBeenLastCalledWith(1, expect.any(Number), "production", "Rerun", expect.any(Function));
  await act(async () => rerun.resolve({ ...production, handle: "review-2", runPlan: "Rerun" }));
  expect(screen.getByText(/On the deployed commit/)).toBeInTheDocument();
  expect(screen.queryByRole("region", { name: "Commits" })).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("Type production to continue"), { target: { value: "production" } });
  confirm.mockReturnValue(new Promise(() => {}));
  fireEvent.click(screen.getByRole("button", { name: "Rerun all steps" }));
  expect(confirm).toHaveBeenCalledWith("review-2", "production");
});

test("back and escape on preview release the lock; a failed release explains it", async () => {
  const { pending } = await openWorkspace();
  await act(async () => pending.resolve(production));
  const release = later<void>();
  cancel.mockReturnValue(release.promise);
  fireEvent.keyDown(window, { key: "Escape" });
  expect(cancel).toHaveBeenCalledWith("review-1");
  expect(screen.getByText("Releasing the server lock…")).toBeInTheDocument();
  const failed: CommandError = {
    code: "connection",
    message: "Couldn't release the server lock. After 2 minutes, slip break-lock production can clear it.",
    nextStep: {
      kind: "command",
      prefix: "cd '/Users/you/code/shop' && ",
      command: "slip break-lock production",
      copyText: "cd '/Users/you/code/shop' && slip break-lock production",
    },
  };
  await act(async () => release.reject(failed));
  expect(within(screen.getByRole("main")).getByText(failed.message)).toBeInTheDocument();
  expect(screen.getByText("slip break-lock production")).toBeInTheDocument();
});

async function startRun() {
  const opened = await openWorkspace();
  await act(async () => opened.pending.resolve(production));
  fireEvent.change(screen.getByLabelText("Type production to deploy"), {
    target: { value: "production" },
  });
  confirm.mockResolvedValue(fixtures.runInfo);
  fireEvent.click(screen.getByRole("button", { name: "Deploy to production" }));
  await settle();
  return opened;
}

test("running lists every step with its mark and follows the running step's output", async () => {
  await startRun();
  expect(screen.getByText("Connecting to shop-prod…")).toBeInTheDocument();
  await act(async () => send(fixtures.runBatches.running));
  const steps = within(screen.getByRole("region", { name: "Steps" }));
  const heads = steps.getAllByRole("button").map((b) => b.textContent);
  expect(heads).toEqual([
    "✓doneonMaintenance on: php artisan down",
    "✓done0git fast-forward",
    expect.stringMatching(/^1composer install --no-dev\d+s$/),
    "·not started2php artisan migrate --force",
    "·not started3php artisan optimize",
    "·not startedoffMaintenance off: php artisan up",
  ]);
  expect(steps.getByRole("img", { name: "running now" })).toBeInTheDocument();
  expect(screen.getByRole("region", { name: "Output of step 1" })).toHaveTextContent(
    "composer install --no-dev: working",
  );
  expect(screen.queryByRole("region", { name: "Output of step 0" })).not.toBeInTheDocument();
  fireEvent.click(steps.getByRole("button", { name: /git fast-forward/ }));
  expect(screen.getByRole("region", { name: "Output of step 0" })).toBeInTheDocument();
  expect(screen.getByTestId("announce-polite")).toHaveTextContent(
    "Step 1 of 3 started: composer install --no-dev",
  );

  // Running ignores Escape.
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.getByRole("region", { name: "Steps" })).toBeInTheDocument();

  fireEvent.click(screen.getByRole("button", { name: "Stop after this step" }));
  expect(stop).toHaveBeenCalledWith("run-1", "afterStep");
  expect(screen.getByText("Stopping after this step…")).toBeInTheDocument();
});

test("remote output is shown as text, never as markup", async () => {
  await startRun();
  const batch = fixtures.runBatches.running;
  await act(async () =>
    send({
      ...batch,
      output: [{ sequence: 8, text: "<img src=x onerror=alert(1)>", source: { kind: "step", index: 1 } }],
    }),
  );
  const output = screen.getByRole("region", { name: "Output of step 1" });
  expect(output).toHaveTextContent("<img src=x onerror=alert(1)>");
  expect(output.querySelector("img")).toBeNull();
});

test("the log watch counts down, lists new errors and can be stopped", async () => {
  await startRun();
  await act(async () => send(fixtures.runBatches.watching));
  const watch = within(screen.getByRole("region", { name: "Log watch" }));
  expect(watch.getByText("2:00 left")).toBeInTheDocument();
  expect(watch.getByText("RuntimeException: Payment gateway timed out")).toBeInTheDocument();
  expect(watch.getByText("app/Billing.php:42")).toBeInTheDocument();
  expect(screen.getByTestId("announce-polite")).toHaveTextContent("Log watch started, 2 minutes");
  fireEvent.click(screen.getByRole("button", { name: "Stop watching" }));
  expect(stop).toHaveBeenCalledWith("run-1", "watch");
  expect(screen.getByText("Stopping the log watch…")).toBeInTheDocument();
});

test("a failed stop says why and can be tried again", async () => {
  await startRun();
  await act(async () => send(fixtures.runBatches.running));
  stop.mockRejectedValue({ code: "notFound", message: "This run is not running any more." });
  fireEvent.click(screen.getByRole("button", { name: "Stop after this step" }));
  await settle();
  expect(screen.getByText("This run is not running any more.")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Stop after this step" })).toBeEnabled();
});

test("when the run ends the workspace closes, the map re-reads and the end is announced", async () => {
  await startRun();
  await act(async () => send(fixtures.runBatches.running));
  const reads = loadMap.mock.calls.length;
  await act(async () => send(fixtures.runBatches.failed));
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(loadMap.mock.calls.length).toBe(reads + 1);
  expect(screen.getByTestId("announce-assertive")).toHaveTextContent("Deploy failed at step 1.");
  expect(unfollow).toHaveBeenCalled();
});

test("a run that can no longer be followed closes with its reason", async () => {
  const opened = await openWorkspace();
  await act(async () => opened.pending.resolve({ ...production, production: false, env: "staging" }));
  confirm.mockResolvedValue(fixtures.runInfo);
  fireEvent.click(screen.getByRole("button", { name: "Deploy to staging" }));
  await settle();
  await act(async () =>
    send({
      ...fixtures.runBatches.running,
      state: {
        ...fixtures.runBatches.running.state,
        terminal: { kind: "interrupted", step: 1, reason: "connection lost" },
      },
    }),
  );
  expect(screen.getByTestId("announce-assertive")).toHaveTextContent(
    "Lost the connection during step 1. It may still be running on the server.",
  );
});

async function renderApp() {
  render(<App />);
  await settle();
  return selectedCell();
}

test("changed settings open the trust review; approving returns to the pane with a note", async () => {
  entry.mockResolvedValue(ready({ needsTrust: true }));
  reviewTrust.mockResolvedValue(fixtures.trustReview);
  const cell = await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Review deploy…" }));
  await settle();
  expect(prepare).not.toHaveBeenCalled();
  expect(reviewTrust).toHaveBeenCalledWith(1, cell.row, cell.env);
  const heading = screen.getByRole("heading", { level: 1 });
  expect(heading).toHaveTextContent(`Review configuration · ${cell.label} · ${cell.env}`);
  expect(heading).toHaveFocus();

  const settings = within(screen.getByRole("region", { name: "Settings" }));
  const branch = settings.getByRole("heading", { level: 3, name: "branch" }).closest("li")!;
  expect(branch).toHaveTextContent(/Previous.*main.*Proposed.*release/);
  const steps = settings.getByRole("heading", { level: 3, name: "recipe steps" }).closest("li")!;
  expect(within(steps).getAllByRole("list")).toHaveLength(2);
  expect(settings.queryByRole("heading", { name: "ssh" })).not.toBeInTheDocument();
  fireEvent.click(settings.getByRole("button", { name: "9 unchanged" }));
  expect(settings.getByRole("heading", { level: 3, name: "ssh" })).toBeInTheDocument();

  expect(
    screen.getByText("Approving lets you prepare a deploy. Deploying needs its own confirmation."),
  ).toBeInTheDocument();
  const approve = screen.getByRole("button", { name: "Approve configuration" });
  expect(approve).toBeDisabled();
  fireEvent.change(screen.getByLabelText("Type production to approve"), {
    target: { value: "production" },
  });
  approveTrust.mockResolvedValue(undefined);
  entry.mockResolvedValue(ready());
  fireEvent.click(approve);
  await settle();
  expect(approveTrust).toHaveBeenCalledWith("trust-1", "production");
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(screen.getByText(`Configuration approved for ${cell.env}`)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Review deploy…" })).toHaveFocus();
  // Approval alone never prepares a deploy.
  expect(prepare).not.toHaveBeenCalled();
});

test("a config that changed during the review must be reviewed again; escape goes back", async () => {
  entry.mockResolvedValue(ready({ needsTrust: true }));
  reviewTrust.mockResolvedValue(fixtures.trustReview);
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Review deploy…" }));
  await settle();
  fireEvent.change(screen.getByLabelText("Type production to approve"), {
    target: { value: "production" },
  });
  approveTrust.mockRejectedValue({
    code: "changed",
    message: "The config changed. Review it again before continuing.",
  });
  fireEvent.click(screen.getByRole("button", { name: "Approve configuration" }));
  await settle();
  expect(
    within(screen.getByRole("main")).getByText(/Config changed again; review the new version\./),
  ).toBeInTheDocument();
  expect(screen.getByTestId("announce-assertive")).toHaveTextContent("Config changed again");
  expect(screen.getByRole("button", { name: "Approve configuration" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "Review again" }));
  await settle();
  expect(reviewTrust).toHaveBeenCalledTimes(2);
  expect(screen.getByRole("button", { name: "Approve configuration" })).toBeDisabled();

  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(approveTrust).toHaveBeenCalledTimes(1);
});

test("a failed approval for another reason shows the refusal screen", async () => {
  entry.mockResolvedValue(ready({ needsTrust: true }));
  reviewTrust.mockResolvedValue(fixtures.trustReview);
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Review deploy…" }));
  await settle();
  fireEvent.change(screen.getByLabelText("Type production to approve"), {
    target: { value: "production" },
  });
  approveTrust.mockRejectedValue({ code: "local", message: "trust store is read-only" });
  fireEvent.click(screen.getByRole("button", { name: "Approve configuration" }));
  await settle();
  expect(screen.getByRole("heading", { level: 2, name: "Shipslip couldn't continue" })).toBeInTheDocument();
  expect(screen.getByText("trust store is read-only")).toBeInTheDocument();
});

test.each([
  {
    error: {
      code: "connection",
      connectionKind: "hostKeyUnknown",
      message: "Host key verification failed.",
      nextStep: { kind: "command", prefix: "", command: "ssh shop-prod", copyText: "ssh shop-prod" },
    } as CommandError,
    title: "Shipslip doesn't know this server yet",
    body: "Connect once in Terminal and compare the fingerprint.",
    command: "ssh shop-prod",
  },
  {
    // Even if a command arrived, a changed host key never offers one.
    error: {
      code: "connection",
      connectionKind: "hostKeyChanged",
      message: "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!",
      nextStep: { kind: "command", prefix: "", command: "ssh shop-prod", copyText: "ssh shop-prod" },
    } as CommandError,
    title: "This server's identity changed",
    body: "Verify with your hosting provider before connecting. Shipslip will not continue.",
    command: null,
  },
  {
    error: fixtures.lockHeld,
    title: "Another run holds the server lock",
    body: fixtures.lockHeld.message,
    command: "slip break-lock production",
  },
  {
    error: {
      code: "blocked",
      blockedKind: "fetchFailed",
      message: "The server could not fetch origin: denied.",
    } as CommandError,
    title: "The server couldn't fetch the code",
    body: "The server could not fetch origin: denied.",
    command: null,
  },
])("refusal: $title", async ({ error, title, body, command }) => {
  const { pending } = await openWorkspace();
  await act(async () => pending.reject(error));
  const main = within(screen.getByRole("main"));
  expect(main.getByRole("heading", { level: 2, name: title })).toBeInTheDocument();
  expect(main.getByText(body)).toBeInTheDocument();
  expect(screen.getByTestId("announce-assertive")).toHaveTextContent(title);
  if (command) expect(main.getByText(command)).toBeInTheDocument();
  else expect(main.queryByRole("button", { name: /^Copy/ })).not.toBeInTheDocument();
  if (error.connectionKind === "hostKeyChanged") expect(main.queryByText("ssh shop-prod")).toBeNull();
});

test("a held lock can be checked again", async () => {
  const { pending } = await openWorkspace();
  await act(async () => pending.reject(fixtures.lockHeld));
  prepare.mockReturnValue(new Promise(() => {}));
  fireEvent.click(screen.getByRole("button", { name: "Check again" }));
  expect(prepare).toHaveBeenCalledTimes(2);
  expect(prepare).toHaveBeenLastCalledWith(1, expect.any(Number), expect.any(String), "Deploy", expect.any(Function));
  expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
});

test("an env already up to date offers a rerun from its refusal", async () => {
  const { pending } = await openWorkspace();
  await act(async () =>
    pending.reject({
      code: "blocked",
      blockedKind: "upToDate",
      message: "The server is already on the newest commit. Rerun runs the steps again on it.",
    } as CommandError),
  );
  expect(screen.getByRole("heading", { level: 2, name: "Already up to date" })).toBeInTheDocument();
  prepare.mockReturnValue(new Promise(() => {}));
  fireEvent.click(screen.getByRole("button", { name: "Rerun…" }));
  expect(prepare).toHaveBeenLastCalledWith(1, expect.any(Number), expect.any(String), "Rerun", expect.any(Function));
});

test("settings that changed after the map was read lead to their review, not a CLI command", async () => {
  const { pending } = await openWorkspace();
  await act(async () =>
    pending.reject({
      code: "untrusted",
      message: "config for `production` is untrusted; run `slip trust production` first. Review and approve these settings first.",
    } as CommandError),
  );
  const main = within(screen.getByRole("main"));
  expect(main.getByRole("heading", { level: 2, name: "These settings need approval" })).toBeInTheDocument();
  expect(
    main.getByText("The settings for this env changed since they were approved. Review them before deploying."),
  ).toBeInTheDocument();
  reviewTrust.mockResolvedValue(fixtures.trustReview);
  fireEvent.click(main.getByRole("button", { name: "Review configuration" }));
  await settle();
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(/^Review configuration · /);
  expect(screen.getByRole("button", { name: "Approve configuration" })).toBeInTheDocument();
});

test("commands in library sentences show as code, never as markup", async () => {
  entry.mockResolvedValue(
    ready({
      recovery: {
        ...fixtures.recoveries[0]!,
        sentence: "The log showed 1 new error group after this run. `slip logs <b>production</b>` shows the log.",
        advice: null,
      },
    }),
  );
  await renderApp();
  const pane = within(screen.getByRole("complementary", { name: "Receipt" }));
  const code = pane.getByText("slip logs <b>production</b>");
  expect(code.tagName).toBe("CODE");
  expect(pane.queryByText(/`/)).toBeNull();
  expect(code.querySelector("b")).toBeNull();
});

test("Open config in More opens the selected env's config, and says why when it can't", async () => {
  entry.mockResolvedValue(ready());
  const cell = await renderApp();
  fireEvent.click(screen.getByRole("button", { name: /^More/ }));
  openConfig.mockResolvedValue(undefined);
  fireEvent.click(screen.getByRole("button", { name: "Open config" }));
  await settle();
  expect(openConfig).toHaveBeenCalledWith(1, cell.row, cell.env);
  expect(prepare).not.toHaveBeenCalled();
  expect(screen.queryByRole("status")).toBeNull();

  openConfig.mockRejectedValue({ code: "notFound", message: "This checkout has no .shipslip.toml." });
  fireEvent.click(screen.getByRole("button", { name: "Open config" }));
  await settle();
  expect(screen.getByText("This checkout has no .shipslip.toml.")).toBeInTheDocument();
});

test("the trust review can open the config in the editor without approving anything", async () => {
  entry.mockResolvedValue(ready({ needsTrust: true }));
  reviewTrust.mockResolvedValue(fixtures.trustReview);
  const cell = await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Review deploy…" }));
  await settle();
  openConfig.mockResolvedValue(undefined);
  fireEvent.click(screen.getByRole("button", { name: "Edit .shipslip.toml in your editor" }));
  await settle();
  expect(openConfig).toHaveBeenCalledWith(1, cell.row, cell.env);
  expect(approveTrust).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Approve configuration" })).toBeDisabled();
});

const upReview: RecoveryReview = {
  handle: "up-1",
  env: "production",
  production: true,
  operation: "up",
  sshAlias: "shop-prod",
  path: "/var/www/shop",
  runId: null,
  targetSha: null,
  phase: "Bring this app out of maintenance mode",
  remainingSteps: [{ index: null, command: "php artisan up" }],
};

test("a failed run's recovery: one recommended action, each with its command", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[0] }));
  await renderApp();
  const pane = within(screen.getByRole("complementary", { name: "Receipt" }));
  expect(pane.getByText("Step 1 failed (exit 1): php artisan migrate --force")).toBeInTheDocument();
  const up = pane.getByRole("button", { name: "Bring app out of maintenance…" });
  const from = pane.getByRole("button", { name: "Run from step 1…" });
  expect(up).toHaveClass("button-primary");
  expect(from).not.toHaveClass("button-primary");
  expect(pane.getByText("slip up production")).toBeInTheDocument();
  expect(pane.getByText("slip from-step production 1")).toBeInTheDocument();
});

test("bring the app up: a confirm shows the one step and needs the name typed", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[0] }));
  reviewUp.mockResolvedValue(upReview);
  const cell = await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Bring app out of maintenance…" }));
  await settle();
  expect(reviewUp).toHaveBeenCalledWith(1, cell.row, cell.env);
  expect(screen.getByRole("heading", { level: 1 })).toHaveFocus();
  const steps = within(screen.getByRole("region", { name: "Steps" })).getAllByRole("listitem");
  expect(steps.map((s) => s.textContent)).toEqual(["offphp artisan up"]);
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(/^Bring .+ → .+ out of maintenance$/);
  const go = screen.getByRole("button", { name: "Bring app out of maintenance" });
  expect(go).toBeDisabled();
  fireEvent.change(screen.getByLabelText("Type production to continue"), {
    target: { value: "production" },
  });
  confirm.mockResolvedValue({ ...fixtures.runInfo, operation: "up", runPlan: null, steps: [], handle: "up-1" });
  fireEvent.click(go);
  await settle();
  expect(confirm).toHaveBeenCalledWith("up-1", "production");
  await act(async () => send({ ...fixtures.runBatches.running, handle: "up-1" }));
  const rows = within(screen.getByRole("region", { name: "Steps" })).getAllByRole("button");
  expect(rows.map((r) => r.textContent)).toEqual(["·not startedoffMaintenance off: php artisan up"]);
  // One step: nothing after it to stop before.
  expect(screen.queryByRole("button", { name: "Stop after this step" })).toBeNull();
});

test("back from a recovery confirm frees its reservation", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[0] }));
  reviewUp.mockResolvedValue(upReview);
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Bring app out of maintenance…" }));
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "Back" }));
  await settle();
  expect(cancel).toHaveBeenCalledWith("up-1");
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
});

test("attach shows the recorded run and what is left; a missing run is refused", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[1] }));
  expect(fixtures.recoveries[1]?.sentence).toBe(
    "The run did not finish. It may still be running on the server.",
  );
  reviewAttach.mockResolvedValue({
    ...upReview,
    handle: "attach-1",
    operation: "attach",
    runId: "1791795000000-0000beef",
    targetSha: "b".repeat(40),
    phase: "Steps",
    remainingSteps: [
      { index: 2, command: "php artisan migrate --force" },
      { index: 3, command: "php artisan optimize" },
    ],
  });
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Attach…" }));
  await settle();
  expect(screen.getByText("Run 1791795000000-0000beef · target bbbbbbb")).toBeInTheDocument();
  expect(screen.getByText("Last phase: Steps")).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(/^Attach to .+ → .+$/);
  const left = within(screen.getByRole("region", { name: "Steps" })).getAllByRole("listitem");
  expect(left.map((s) => s.textContent)).toEqual(["2php artisan migrate --force", "3php artisan optimize"]);
  expect(screen.getByRole("button", { name: "Attach and continue" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "Back" }));
  await settle();

  reviewAttach.mockRejectedValue({
    code: "notFound",
    message: "There is no unfinished run for this checkout and environment.",
  });
  fireEvent.click(screen.getByRole("button", { name: "Attach…" }));
  await settle();
  expect(screen.getByRole("heading", { level: 2, name: "Nothing to continue" })).toBeInTheDocument();
});

test("an unknown result says to check the server and offers no action", async () => {
  entry.mockResolvedValue(
    ready({
      recovery: {
        runId: "r",
        sentence: "Lost the connection during step 3. It may still be running on the server.",
        advice: "Check the server before running anything.",
        actions: [],
      },
    }),
  );
  await renderApp();
  const pane = within(screen.getByRole("complementary", { name: "Receipt" }));
  expect(pane.getByText("Check the server before running anything.")).toBeInTheDocument();
  expect(pane.queryByRole("button", { name: /Attach|Run from step|maintenance/ })).toBeNull();
});

test("run from step: pick first, then the preview strikes skipped steps", async () => {
  entry.mockResolvedValue(
    ready({
      recovery: {
        ...fixtures.recoveries[0]!,
        actions: [{ kind: "fromStep", step: 2, command: "slip from-step production 2" }],
      },
    }),
  );
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Run from step 2…" }));
  expect(prepare).not.toHaveBeenCalled();
  expect(screen.getByRole("radio", { name: /2\s*php artisan migrate/ })).toBeChecked();
  expect(screen.getByRole("radio", { name: /1\s*composer install/ }).closest("li")).toHaveClass(
    "step-skipped",
  );
  fireEvent.click(screen.getByRole("radio", { name: /3\s*php artisan optimize/ }));
  const pending = later<DeployReview>();
  prepare.mockReturnValue(pending.promise);
  fireEvent.click(screen.getByRole("button", { name: "Prepare run from step 3" }));
  expect(prepare).toHaveBeenCalledWith(1, expect.any(Number), expect.any(String), { FromStep: 3 }, expect.any(Function));
  await act(async () => pending.resolve({ ...production, runPlan: { FromStep: 3 } }));
  const steps = within(screen.getByRole("region", { name: "Steps" })).getAllByRole("listitem");
  expect(steps.filter((s) => s.classList.contains("step-skipped"))).toHaveLength(2);
  expect(screen.getByText("1 step · maintenance on · log watch 2 min")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Run from step 3" })).toBeDisabled();
});

test("the More menu offers Rerun and Run from step", async () => {
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: /^More/ }));
  prepare.mockReturnValue(new Promise(() => {}));
  fireEvent.click(screen.getByRole("button", { name: "Rerun…" }));
  expect(prepare).toHaveBeenCalledWith(1, expect.any(Number), expect.any(String), "Rerun", expect.any(Function));
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
});

test("run from step in the menu defaults to step 1 with no recovery", async () => {
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: /^More/ }));
  fireEvent.click(screen.getByRole("button", { name: "Run from step…" }));
  expect(screen.getByRole("radio", { name: /1\s*composer install/ })).toBeChecked();
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
});

test("needing approval sends every entry action to the trust review first", async () => {
  entry.mockResolvedValue(ready({ needsTrust: true, recovery: fixtures.recoveries[0] }));
  reviewTrust.mockResolvedValue(fixtures.trustReview);
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Bring app out of maintenance…" }));
  await settle();
  expect(reviewUp).not.toHaveBeenCalled();
  expect(screen.getByRole("region", { name: "Settings" })).toBeInTheDocument();
});

test("a lock taken while the preview was open says nothing ran and offers Prepare again", async () => {
  await startRun();
  await act(async () =>
    send({
      ...fixtures.runBatches.lockTaken,
    }),
  );
  expect(fixtures.runBatches.lockTaken.state.terminal).toMatchObject({ lockTaken: true });
  const message =
    "Another run took the server lock while this preview was open. Nothing ran on the server.";
  expect(within(screen.getByRole("main")).getByText(message)).toBeInTheDocument();
  expect(screen.getByTestId("announce-assertive")).toHaveTextContent(message);
  prepare.mockReturnValue(new Promise(() => {}));
  fireEvent.click(screen.getByRole("button", { name: "Prepare again" }));
  expect(prepare).toHaveBeenCalledTimes(2);
  expect(prepare).toHaveBeenLastCalledWith(1, expect.any(Number), expect.any(String), "Deploy", expect.any(Function));
});

/** A run of the selected cell, started `ago` ms before now. */
function liveRun(cell: { row: number; env: string; label: string }, ago: number, handle = "run-1"): RunInfo {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  return {
    ...fixtures.runInfo,
    handle,
    checkout: fixtures.map.map.rows[cell.row].repoRoot,
    env: cell.env,
    project: cell.label,
    startedAtMs: Date.now() - ago,
  };
}

test("a preview closed by a reload is noted on its cell", async () => {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture");
  const first = render(<App />);
  await settle();
  const cell = selectedCell();
  first.unmount();
  liveRuns.mockResolvedValue({
    runs: [],
    closedPreviews: [
      { checkout: fixtures.map.map.rows[cell.row].repoRoot, env: cell.env, released: true },
    ],
  });
  render(<App />);
  await settle();
  expect(
    screen.getByText("The preview closed when the window reloaded. The server lock was released; nothing ran."),
  ).toBeInTheDocument();
});

test("a live run shows on its cell and in the toolbar, which leads back to it", async () => {
  const first = render(<App />);
  await settle();
  const cell = selectedCell();
  first.unmount();
  liveRuns.mockResolvedValue({ runs: [liveRun(cell, 134_000)], closedPreviews: [] });
  render(<App />);
  await settle();
  const grid = within(screen.getByRole("grid", { name: "Release map" }));
  expect(grid.getByText(/^Deploying… 2m 1\ds$/)).toBeInTheDocument();
  expect(grid.getByRole("img", { name: "running now" })).toBeInTheDocument();
  const link = screen.getByRole("button", { name: `1 deploy running · ${cell.label} → ${cell.env}` });

  fireEvent.click(link);
  await settle();
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(`Deploy ${cell.label} → ${cell.env}`);
  expect(screen.getByText("Connecting to shop-prod…")).toBeInTheDocument();

  // Release map leaves the run going and returns focus to the toolbar link.
  fireEvent.click(screen.getByRole("button", { name: "Release map" }));
  await settle();
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(stop).not.toHaveBeenCalled();
  expect(cancel).not.toHaveBeenCalled();
  expect(unfollow).toHaveBeenCalled();
  expect(screen.getByRole("button", { name: /^1 deploy running/ })).toHaveFocus();
});

test("several live runs share one toolbar menu", async () => {
  const first = render(<App />);
  await settle();
  const cell = selectedCell();
  first.unmount();
  liveRuns.mockResolvedValue({
    runs: [liveRun(cell, 1000), { ...liveRun(cell, 1000, "run-2"), project: "other", env: "staging" }],
    closedPreviews: [],
  });
  render(<App />);
  await settle();
  fireEvent.click(screen.getByRole("button", { name: /^2 deploys running/ }));
  expect(screen.getByRole("button", { name: `${cell.label} → ${cell.env}` })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "other → staging" })).toBeInTheDocument();
});

test("a run that ends in the background re-reads the map and the entry", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  try {
    render(<App />);
    await settle();
    const cell = selectedCell();
    liveRuns.mockResolvedValue({ runs: [liveRun(cell, 1000)], closedPreviews: [] });
    await act(async () => vi.advanceTimersByTimeAsync(2000));
    expect(screen.getByRole("button", { name: /^1 deploy running/ })).toBeInTheDocument();
    const reads = loadMap.mock.calls.length;
    const asks = entry.mock.calls.length;
    liveRuns.mockResolvedValue({ runs: [], closedPreviews: [] });
    await act(async () => vi.advanceTimersByTimeAsync(2000));
    expect(loadMap.mock.calls.length).toBe(reads + 1);
    expect(entry.mock.calls.length).toBeGreaterThan(asks);
    expect(screen.queryByRole("button", { name: /deploy running/ })).not.toBeInTheDocument();
  } finally {
    vi.useRealTimers();
  }
});
