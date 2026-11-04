import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import { api } from "./api";
import * as fixtures from "./fixtures.gen";
import type {
  CommandError,
  DeployEntry,
  DeployReview,
  LiveRuns,
  LoadResult,
  PrepareProgress,
  ReceiptResult,
  RecoveryReview,
  RunBatch,
  RunInfo,
  TrustReview,
} from "./types";

const loadMap = vi.fn<() => Promise<LoadResult>>();
const loadReceipt = vi.fn<(generation: number, id: number) => Promise<ReceiptResult>>();
vi.mock("./api", () => ({
  api: {
    loadMap: () => loadMap(),
    receiptsChanged: async () => false,
    compare: vi.fn(),
    loadReceipt: (generation: number, id: number) => loadReceipt(generation, id),
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
  loadReceipt.mockReset().mockResolvedValue({ kind: "notFound" });
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

test("a terminal run disables Deploy and offers only following it in its terminal", async () => {
  loadMap.mockResolvedValue(fixtures.terminalMap);
  entry.mockResolvedValue(fixtures.terminalEntry);
  render(<App />);
  await settle();
  const panel = screen.getByRole("complementary", { name: "Receipt" });
  const button = within(panel).getByRole("button", { name: "Deploy shop → staging…" });
  expect(button).toBeDisabled();
  expect(button).toHaveAccessibleDescription("A deploy is running in a terminal on this Mac. Follow it in that terminal.");
  expect(within(panel).getByText("Follow this run in the terminal where it started.")).toBeInTheDocument();
  expect(within(panel).queryByRole("button", { name: /Attach|Bring app out|Run from step|Rerun/ })).toBeNull();
  expect(within(panel).queryByText(/slip attach/)).toBeNull();
  expect(prepare).not.toHaveBeenCalled();
  expect(reviewAttach).not.toHaveBeenCalled();
});

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

/** The panel's deploy button names its app and env. */
const DEPLOY = /^Deploy \S+ → \S+…$/;

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
  fireEvent.click(screen.getByRole("button", { name: DEPLOY }));
  await settle();
  return { cell, pending, progress: (phase: PrepareProgress) => act(() => progress(phase)) };
}

const production = { ...fixtures.deployReview, env: "production" };

test("review deploy shows only for envs with a config, and says why it is off", async () => {
  entry.mockResolvedValue({ kind: "hidden" });
  const { unmount } = render(<App />);
  await settle();
  expect(entry).toHaveBeenCalledWith(1, selectedCell().row, selectedCell().env);
  expect(screen.queryByRole("button", { name: DEPLOY })).not.toBeInTheDocument();
  unmount();

  for (const [answer, reason] of [
    [{ kind: "unavailable", reason: "Checkout folder is missing" }, "Checkout folder is missing"],
    [{ kind: "running", handle: "run-1" }, "A deploy is running for this env"],
  ] as const) {
    entry.mockResolvedValue(answer);
    const view = render(<App />);
    await settle();
    const button = screen.getByRole("button", { name: DEPLOY });
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
  expect(loadMap).toHaveBeenCalledTimes(2);
  expect(screen.getByRole("button", { name: DEPLOY })).toHaveFocus();
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

test("preview says when the environment has no smoke check", async () => {
  const { pending } = await openWorkspace();
  await act(async () => pending.resolve(fixtures.deployReview));
  expect(screen.getByText("No smoke check configured for this environment.")).toBeInTheDocument();
});

test("a configured smoke check does not show the no-smoke notice", async () => {
  const { pending } = await openWorkspace();
  await act(async () => pending.resolve({ ...fixtures.deployReview, smokeConfigured: true }));
  expect(screen.queryByText("No smoke check configured for this environment.")).not.toBeInTheDocument();
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
    "onphp artisan down",
    "0git merge --ff-only bbbbbbb",
    "1composer install --no-dev",
    "2php artisan migrate --force",
    "3php artisan optimize",
    "offphp artisan uponce every step succeeds",
    "logwatch the Laravel log for 2 min",
  ]);
  const risks = within(screen.getByRole("region", { name: "Before you confirm" })).getAllByRole("listitem");
  expect(risks.map((r) => r.textContent)).toEqual([
    "The app is in maintenance mode from before the first step until every step succeeds. If a step fails, it stays in maintenance mode.",
    "Step 2 changes the database (php artisan migrate --force). Shipslip cannot undo it.",
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

async function startRun(info: RunInfo = fixtures.runInfo) {
  const opened = await openWorkspace();
  await act(async () => opened.pending.resolve(production));
  fireEvent.change(screen.getByLabelText("Type production to deploy"), {
    target: { value: "production" },
  });
  confirm.mockResolvedValue(info);
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
    "onphp artisan down",
    "0git fast-forward1s",
    "1composer install --no-dev",
    "2php artisan migrate --force",
    "3php artisan optimize",
    "offphp artisan up",
  ]);
  expect(steps.getAllByRole("img", { name: "done" })).toHaveLength(2);
  expect(steps.getAllByRole("img", { name: "not started" })).toHaveLength(3);
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
  expect(screen.getByRole("button", { name: "Stopping after this step…" })).toBeDisabled();
  expect(screen.queryByRole("button", { name: "Stop after this step" })).not.toBeInTheDocument();
});

test("a step shows the duration its finished event carries, and no timer while it runs", async () => {
  await startRun();
  const batch = fixtures.runBatches.running;
  await act(async () => send(batch));
  const steps = within(screen.getByRole("region", { name: "Steps" }));
  const head = () => steps.getByRole("button", { name: /composer install/ });
  expect(head()).toHaveTextContent(/--no-dev$/);
  const step = batch.state.steps["1"]!;
  await act(async () =>
    send({
      ...batch,
      replay: false,
      after: batch.through,
      through: batch.through + 1,
      output: [],
      state: {
        ...batch.state,
        steps: {
          ...batch.state.steps,
          "1": { ...step, status: "Ok", exitCode: 0, timing: { kind: "took", duration: "2m 44s", durationMs: 164_000 } },
        },
      },
    }),
  );
  expect(head()).toHaveTextContent(/--no-dev2m 44s$/);
  expect(screen.getByTestId("announce-polite")).toHaveTextContent("Step 1 done in 2 minutes 44 seconds");

  await act(async () =>
    send({
      ...batch,
      replay: false,
      after: batch.through + 1,
      through: batch.through + 2,
      output: [],
      state: {
        ...batch.state,
        steps: {
          ...batch.state.steps,
          "1": { ...step, status: "Unknown", exitCode: null, timing: { kind: "endNotKnown", startedAtMs: Date.UTC(2026, 9, 31, 14, 3) } },
        },
      },
    }),
  );
  expect(head()).toHaveTextContent(/--no-devstarted \d\d:\d\d.*, end not known$/);
  expect(head().querySelector("time")).toHaveAttribute("title", "2026-10-31 14:03 UTC");
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

test("a failed step leads with its outcome and opens output even after it was closed", async () => {
  await startRun();
  await act(async () => send(fixtures.runBatches.running));
  const step = () => within(screen.getByRole("region", { name: "Steps" }))
    .getByRole("button", { name: /composer install/ });
  fireEvent.click(step());
  expect(step()).toHaveAttribute("aria-expanded", "false");
  await act(async () => send(fixtures.runBatches.failedHeadline));
  expect(screen.getByRole("heading", { name: "Failed at step 1 · exit 1 · composer install --no-dev" }))
    .toHaveClass("tone-bad");
  expect(step()).toHaveAttribute("aria-expanded", "true");
  expect(screen.getByRole("region", { name: "Output of step 1" })).toBeInTheDocument();
  fireEvent.click(step());
  expect(step()).toHaveAttribute("aria-expanded", "false");
});

test.each([
  ["failedWatching", "Failed at step 1 · exit 9 · composer install --no-dev", "tone-bad"],
  ["stoppedWatching", "Stopped after step 1 · Steps 2, 3 did not run", "tone-attention"],
  ["succeededWatching", "Succeeded", null],
] as const)("%s keeps the outcome above the secondary log watch", async (fixture, text, tone) => {
  vi.useFakeTimers();
  try {
    await startRun({ ...fixtures.runInfo, maintenance: false });
    await act(async () => send(fixtures.runBatches[fixture]!));
    const headline = screen.getByRole("heading", { name: text });
    if (tone) expect(headline).toHaveClass(tone);
    else {
      for (const statusTone of ["tone-bad", "tone-attention", "tone-good"]) {
        expect(headline).not.toHaveClass(statusTone);
      }
    }
    const watch = screen.getByText("Watching the log · 2:00 left");
    expect(headline.compareDocumentPosition(watch) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(watch.parentElement).toBe(headline.parentElement);
    await act(async () => vi.advanceTimersByTime(1000));
    expect(screen.getByText("Watching the log · 1:59 left")).toBeInTheDocument();
    expect(headline).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop watching" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Stop after this step" })).not.toBeInTheDocument();
  } finally {
    vi.useRealTimers();
  }
});

test("a failed headline keeps command text literal and does not invent an exit code", async () => {
  await startRun();
  const batch = fixtures.runBatches.failedHeadline!;
  await act(async () => send({
    ...batch,
    state: { ...batch.state, headline: { kind: "failed", step: 1, exitCode: null, command: "<b>migrate</b>" } },
  }));
  const headline = screen.getByRole("heading", { name: "Failed at step 1 · exit code not known · <b>migrate</b>" });
  expect(headline.querySelector("b")).toBeNull();
});

test("a stopped headline names steps that did not run before any watch starts", async () => {
  await startRun();
  await act(async () => send(fixtures.runBatches.stoppedHeadline));
  expect(screen.getByRole("heading", { name: "Stopped after step 1 · Steps 2, 3 did not run" }))
    .toHaveClass("tone-attention");
  expect(screen.queryByText(/Watching the log/)).not.toBeInTheDocument();
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
  expect(screen.getByText(/^Step 1 of 3 · app in maintenance mode · \d/)).toBeInTheDocument();
  await act(async () => send(fixtures.runBatches.failed));
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(loadMap.mock.calls.length).toBe(reads + 1);
  await settle();
  // The new receipt slides up into the pane once, then later selections open plainly.
  expect(document.querySelector(".pane-slip-print")).not.toBeNull();
  fireEvent.click(screen.getAllByRole("gridcell").find((c) => c.getAttribute("aria-selected") === "false")!);
  await settle();
  expect(document.querySelector(".pane-slip-print")).toBeNull();
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
  fireEvent.click(screen.getByRole("button", { name: DEPLOY }));
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
  expect(loadMap).toHaveBeenCalledTimes(2);
  expect(screen.getByText(`Configuration approved for ${cell.env}`)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: DEPLOY })).toHaveFocus();
  // Approval alone never prepares a deploy.
  expect(prepare).not.toHaveBeenCalled();
});

test("a config that changed during the review must be reviewed again; escape goes back", async () => {
  entry.mockResolvedValue(ready({ needsTrust: true }));
  reviewTrust.mockResolvedValue(fixtures.trustReview);
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: DEPLOY }));
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
  fireEvent.click(screen.getByRole("button", { name: DEPLOY }));
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
    title: "Another run deploying",
    body: fixtures.lockHeld.message,
    command: null,
  },
  {
    error: fixtures.terminalLockHeld,
    title: "Another run deploying",
    body: fixtures.terminalLockHeld.message,
    command: null,
  },
  {
    error: fixtures.staleLock,
    title: "Stale lock",
    body: fixtures.staleLock.message,
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

test.each([fixtures.lockHeld, fixtures.staleLock])(
  "a $blockedKind can be checked again",
  async (error) => {
    const { pending } = await openWorkspace();
    await act(async () => pending.reject(error));
    prepare.mockReturnValue(new Promise(() => {}));
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    expect(prepare).toHaveBeenCalledTimes(2);
    expect(prepare).toHaveBeenLastCalledWith(1, expect.any(Number), expect.any(String), "Deploy", expect.any(Function));
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
  },
);

test.each([fixtures.lockHeld, fixtures.staleLock])(
  "a $blockedKind shows the holder and short commit without claiming live state",
  async (error) => {
    const { pending } = await openWorkspace();
    await act(async () => pending.reject(error));
    const main = screen.getByRole("main");
    expect(main).toHaveTextContent("alex@MacBook");
    expect(main).toHaveTextContent("1234567");
    expect(main).not.toHaveTextContent("1234567890abcdef");
    expect(main).not.toHaveTextContent(/\blive\b/i);
    expect(main).not.toHaveTextContent("Details");
  },
);

test("a recent heartbeat offers only Check again, not a terminal command", async () => {
  const { pending } = await openWorkspace();
  await act(async () => pending.reject(fixtures.lockHeld));
  const main = within(screen.getByRole("main"));
  expect(main.getByText(/Last heartbeat 30 seconds ago/)).toBeInTheDocument();
  expect(main.getByRole("button", { name: "Check again" })).toBeInTheDocument();
  expect(main.queryByText(/break-lock|terminal/i)).not.toBeInTheDocument();
  expect(main.queryByRole("button", { name: /^Copy/ })).not.toBeInTheDocument();
});

test("a stale lock offers review guidance and a terminal version note below the copied command", async () => {
  const { pending } = await openWorkspace();
  await act(async () => pending.reject(fixtures.staleLock));
  const main = within(screen.getByRole("main"));
  expect(main.getByText(/stopped sending a heartbeat 10 minutes ago/)).toBeInTheDocument();
  expect(main.getByText(/Review the holder and check the old run on the server/)).toBeInTheDocument();
  const command = main.getByText("slip break-lock production");
  const note = main.getByText(
    /This runs the slip installed in your terminal\. Requires slip 0\.12\.0 or newer\./,
  );
  expect(command.compareDocumentPosition(note) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  const copy = main.getByRole("button", { name: "Copy slip break-lock production" });
  vi.mocked(api.copyText).mockResolvedValueOnce(undefined);
  await act(async () => fireEvent.click(copy));
  expect(api.copyText).toHaveBeenCalledWith("cd '/Users/you/code/shop' && slip break-lock production");
  expect(main.getByRole("button", { name: "Copied" })).toBeInTheDocument();
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
  fireEvent.click(screen.getByRole("menuitem", { name: "Open config" }));
  await settle();
  expect(openConfig).toHaveBeenCalledWith(1, cell.row, cell.env);
  expect(prepare).not.toHaveBeenCalled();
  expect(screen.queryByRole("status")).toBeNull();

  openConfig.mockRejectedValue({ code: "notFound", message: "This checkout has no .shipslip.toml." });
  fireEvent.click(screen.getByRole("button", { name: /^More/ }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Open config" }));
  await settle();
  expect(screen.getByText("This checkout has no .shipslip.toml.")).toBeInTheDocument();
});

test("the trust review can open the config in the editor without approving anything", async () => {
  entry.mockResolvedValue(ready({ needsTrust: true }));
  reviewTrust.mockResolvedValue(fixtures.trustReview);
  const cell = await renderApp();
  fireEvent.click(screen.getByRole("button", { name: DEPLOY }));
  await settle();
  openConfig.mockResolvedValue(undefined);
  fireEvent.click(screen.getByRole("button", { name: "Edit .shipslip.toml in your editor" }));
  await settle();
  expect(openConfig).toHaveBeenCalledWith(1, cell.row, cell.env);
  expect(approveTrust).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Approve configuration" })).toBeDisabled();
});

const upReview = fixtures.upReviewAllRan;

test("a failed run's recovery: one recommended action, each with what it does", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[0] }));
  await renderApp();
  const pane = within(screen.getByRole("complementary", { name: "Receipt" }));
  expect(pane.getByText("Step 1 failed (exit 1): php artisan migrate --force")).toBeInTheDocument();
  const up = pane.getByRole("button", { name: "Bring app out of maintenance…" });
  const from = pane.getByRole("button", { name: "Run from step 1…" });
  expect(up).toHaveClass("button-primary");
  expect(from).not.toHaveClass("button-primary");
  expect(up).toHaveAccessibleDescription("Runs php artisan up to end maintenance mode. No deploy steps run.");
  expect(from).toHaveAccessibleDescription(
    "Runs step 1 and the steps after it on the commit already checked out. No fast-forward.",
  );
  expect(pane.getByText("Each choice opens a review. Nothing runs until you confirm.")).toBeInTheDocument();
  // With a recovery to make first, the deploy button steps back from primary.
  expect(pane.getByRole("button", { name: DEPLOY })).not.toHaveClass("button-primary");
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
  expect(rows.map((r) => r.textContent)).toEqual(["offphp artisan up"]);
  // One step: nothing after it to stop before.
  expect(screen.queryByRole("button", { name: "Stop after this step" })).toBeNull();
});

test("bring-up review with every step run lists nothing and offers no run from step", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[0] }));
  reviewUp.mockResolvedValue(upReview);
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Bring app out of maintenance…" }));
  await settle();
  expect(screen.queryByRole("region", { name: "Did not run" })).toBeNull();
  expect(screen.queryByRole("button", { name: /Run from step/ })).toBeNull();
  expect(screen.getByText("Takes the deploy lock when you confirm.")).toBeInTheDocument();
});

test("bring-up review lists the steps that never ran and offers run from step in place", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[0] }));
  reviewUp.mockResolvedValue(fixtures.upReview);
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Bring app out of maintenance…" }));
  await settle();
  const notRun = within(screen.getByRole("region", { name: "Did not run" }));
  expect(notRun.getByText("These steps did not run. The app will come up without them.")).toBeInTheDocument();
  expect(notRun.getAllByRole("listitem").map((s) => s.textContent)).toEqual([
    "2php artisan migrate --force",
    "3php artisan optimize",
  ]);
  // Bringing the app up still runs only php artisan up.
  const steps = within(screen.getByRole("region", { name: "Steps" })).getAllByRole("listitem");
  expect(steps.map((s) => s.textContent)).toEqual(["offphp artisan up"]);
  expect(screen.getByText("Takes the deploy lock when you confirm.")).toBeInTheDocument();

  const from = notRun.getByRole("button", { name: "Run from step 2 instead…" });
  expect(from).toHaveAccessibleDescription(
    "Runs step 2 and the steps after it on the commit already checked out. No fast-forward.",
  );
  const pending = later<DeployReview>();
  prepare.mockReturnValue(pending.promise);
  fireEvent.click(from);
  await settle();
  // The bring-up's reservation goes back before run from step prepares.
  expect(cancel).toHaveBeenCalledWith("up-1");
  expect(prepare).toHaveBeenCalledWith(1, expect.any(Number), "production", { FromStep: 2 }, expect.any(Function));
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(/^Run from step 2 · .+ → .+$/);
  await act(async () => pending.resolve({ ...production, runPlan: { FromStep: 2 } }));
  expect(screen.getByRole("button", { name: "Run from step 2" })).toBeDisabled();
  expect(confirm).not.toHaveBeenCalled();
});

test("run from step from the bring-up review says so when its reservation can't be released", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[0] }));
  reviewUp.mockResolvedValue(fixtures.upReview);
  cancel.mockRejectedValue({ code: "local", message: "Couldn't release the reservation." });
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Bring app out of maintenance…" }));
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "Run from step 2 instead…" }));
  await settle();
  expect(prepare).not.toHaveBeenCalled();
  expect(screen.getByText("Couldn't release the reservation.")).toBeInTheDocument();
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
  expect(loadMap).toHaveBeenCalledTimes(2);
});

test("attach review: the running step is watched, apart from the steps that will run", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[1] }));
  expect(fixtures.recoveries[1]?.sentence).toBe(
    "The run did not finish. It may still be running on the server.",
  );
  reviewAttach.mockResolvedValue(fixtures.attachReview);
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Attach…" }));
  await settle();
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(/^Attach to .+ → .+$/);
  expect(screen.getByText("Run 1791795000000-0000beef · target bbbbbbb")).toBeInTheDocument();
  expect(screen.queryByText(/phase/i)).not.toBeInTheDocument();

  const watched = within(screen.getByRole("region", { name: "Watched" }));
  expect(
    watched.getByText("Step 2 was running. Shipslip will watch it until it finishes and will not run it again."),
  ).toBeInTheDocument();
  // Its own marker, not a step number.
  expect(watched.getAllByRole("listitem").map((s) => s.textContent)).toEqual(["watchphp artisan migrate --force"]);

  const next = within(screen.getByRole("region", { name: "Steps to run" }));
  expect(next.getByText("If step 2 succeeds, these run next:")).toBeInTheDocument();
  expect(next.getAllByRole("listitem").map((s) => s.textContent)).toEqual([
    "3php artisan optimize",
    "offphp artisan up",
  ]);
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

test("attach review with nothing left to run says so", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[1] }));
  reviewAttach.mockResolvedValue({ ...fixtures.attachReview, watched: null, remainingSteps: [] });
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Attach…" }));
  await settle();
  expect(screen.queryByRole("region", { name: "Watched" })).not.toBeInTheDocument();
  const next = within(screen.getByRole("region", { name: "Steps to run" }));
  expect(next.getByText("No steps left to run")).toBeInTheDocument();
});

test("an attached run starts with what its journal recorded and reads like a normal run", async () => {
  entry.mockResolvedValue(ready({ recovery: fixtures.recoveries[1] }));
  reviewAttach.mockResolvedValue(fixtures.attachReview);
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: "Attach…" }));
  await settle();
  fireEvent.change(screen.getByLabelText("Type production to continue"), {
    target: { value: "production" },
  });
  confirm.mockResolvedValue(fixtures.attachedRunInfo);
  fireEvent.click(screen.getByRole("button", { name: "Attach and continue" }));
  await settle();
  expect(confirm).toHaveBeenCalledWith("attach-1", "production");
  await act(async () => send(fixtures.runBatches.attached));
  const steps = within(screen.getByRole("region", { name: "Steps" }));
  expect(steps.getAllByRole("button").map((b) => b.textContent)).toEqual([
    "onphp artisan down800 ms",
    "0git fast-forward to bbbbbbb1s",
    "1composer install --no-dev2m 44s",
    "2php artisan migrate --force",
    "3php artisan optimize",
    "offphp artisan up",
  ]);
  expect(steps.getAllByRole("img", { name: "done" })).toHaveLength(3);
  expect(steps.getByRole("img", { name: "running now" })).toBeInTheDocument();
  expect(steps.getAllByRole("img", { name: "not started" })).toHaveLength(2);
  expect(screen.getByText(/^Step 2 of 3 · app in maintenance mode · /)).toBeInTheDocument();
});

test("an unfinished run says the recorded cause, and only the panel's Attach", async () => {
  const sentences = {
    detached: "Detached on quit during step 1. The step kept running on the server.",
    appClosed: "The app closed during step 1. The step may still be running on the server.",
    connectionLost: "Lost the connection during step 1. The step may still be running on the server.",
    notRecorded: "The run did not finish. Step 1 may still be running on the server.",
  };
  expect(Object.keys(fixtures.unfinishedRecoveries).sort()).toEqual(Object.keys(sentences).sort());
  for (const [cause, sentence] of Object.entries(sentences)) {
    entry.mockResolvedValue(ready({ recovery: fixtures.unfinishedRecoveries[cause] }));
    await renderApp();
    const pane = within(screen.getByRole("complementary", { name: "Receipt" }));
    expect(pane.getByText(sentence)).toBeInTheDocument();
    expect(pane.getByRole("button", { name: "Attach…" })).toBeInTheDocument();
    expect(pane.queryByText(/slip attach/)).toBeNull();
    cleanup();
  }
});

test("the unfinished run's slip drops its slip attach hint only where the panel offers Attach", async () => {
  const recovery = fixtures.unfinishedRecoveries.appClosed!;
  const unfinished = fixtures.slips.unfinished;
  if (unfinished.kind !== "receipt") throw new Error("fixture");
  const slip = { ...unfinished, runId: recovery.runId };
  loadReceipt.mockImplementation(async (generation, id) => ({ kind: "opened", generation, id, slip }));
  entry.mockResolvedValue(ready({ recovery }));
  await renderApp();
  const pane = within(screen.getByRole("complementary", { name: "Receipt" }));
  expect(pane.getByText("Unfinished")).toBeInTheDocument();
  expect(pane.queryByRole("button", { name: /^Copy slip attach/ })).toBeNull();
  cleanup();

  entry.mockResolvedValue(ready({ recovery: { ...recovery, runId: "another-run" } }));
  await renderApp();
  expect(screen.getByRole("button", { name: /^Copy slip attach/ })).toBeInTheDocument();
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
  // Step 2's migration is skipped, so it is not named as a risk.
  expect(screen.getByRole("region", { name: "Before you confirm" })).not.toHaveTextContent("changes the database");
  expect(screen.getByText("1 step · maintenance on · log watch 2 min")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Run from step 3" })).toBeDisabled();
});

test("the More menu offers Rerun and Run from step", async () => {
  await renderApp();
  const more = screen.getByRole("button", { name: /^More/ });
  fireEvent.click(more);
  expect(more).toHaveAttribute("aria-expanded", "true");
  const menu = within(screen.getByRole("menu", { name: "More" }));
  expect(menu.getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
    "Rerun…",
    "Run from step…",
    "Open config",
  ]);
  expect(menu.getByRole("menuitem", { name: "Rerun…" })).toHaveFocus();
  fireEvent.keyDown(menu.getByRole("menuitem", { name: "Rerun…" }), { key: "ArrowDown" });
  expect(menu.getByRole("menuitem", { name: "Run from step…" })).toHaveFocus();
  fireEvent.keyDown(menu.getByRole("menuitem", { name: "Run from step…" }), { key: "Escape" });
  expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  expect(more).toHaveFocus();

  fireEvent.click(more);
  prepare.mockReturnValue(new Promise(() => {}));
  fireEvent.click(screen.getByRole("menuitem", { name: "Rerun…" }));
  expect(prepare).toHaveBeenCalledWith(1, expect.any(Number), expect.any(String), "Rerun", expect.any(Function));
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
});

test("run from step in the menu defaults to step 1 with no recovery", async () => {
  await renderApp();
  fireEvent.click(screen.getByRole("button", { name: /^More/ }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Run from step…" }));
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

test("a live run shows on its cell and in the sidebar, which leads back to it", async () => {
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
  const sidebar = () => within(screen.getByRole("navigation"));
  expect(grid.getByText("Running")).toHaveClass("flap");
  const link = sidebar().getByRole("button", { name: new RegExp(`^${cell.label} ${cell.env} · 2m 1\\ds$`) });

  fireEvent.click(link);
  await settle();
  expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(`Deploy ${cell.label} → ${cell.env}`);
  expect(screen.getByText("Connecting to shop-prod…")).toBeInTheDocument();

  // Leave running goes back to the map and returns focus to the sidebar link.
  const reads = loadMap.mock.calls.length;
  fireEvent.click(screen.getByRole("button", { name: "Leave running" }));
  await settle();
  expect(screen.getByRole("grid", { name: "Release map" })).toBeInTheDocument();
  expect(loadMap).toHaveBeenCalledTimes(reads + 1);
  expect(stop).not.toHaveBeenCalled();
  expect(cancel).not.toHaveBeenCalled();
  expect(unfollow).toHaveBeenCalled();
  expect(sidebar().getByRole("button", { name: new RegExp(`^${cell.label} ${cell.env} · `) })).toHaveFocus();
});

test("several live runs each get a sidebar line", async () => {
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
  const sidebar = within(screen.getByRole("navigation"));
  expect(sidebar.getByRole("button", { name: new RegExp(`^${cell.label} ${cell.env} · `) })).toBeInTheDocument();
  expect(sidebar.getByRole("button", { name: /^other staging · / })).toBeInTheDocument();
});

test("a run that ends in the background re-reads the map and the entry", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  try {
    render(<App />);
    await settle();
    const cell = selectedCell();
    liveRuns.mockResolvedValue({ runs: [liveRun(cell, 1000)], closedPreviews: [] });
    await act(async () => vi.advanceTimersByTimeAsync(2000));
    const runs = () => within(screen.getByRole("region", { name: "Deploys running" }));
    expect(runs().getByRole("button", { name: new RegExp(`^${cell.label} ${cell.env} · `) })).toBeInTheDocument();
    const reads = loadMap.mock.calls.length;
    const asks = entry.mock.calls.length;
    liveRuns.mockResolvedValue({ runs: [], closedPreviews: [] });
    await act(async () => vi.advanceTimersByTimeAsync(2000));
    expect(loadMap.mock.calls.length).toBe(reads + 1);
    expect(entry.mock.calls.length).toBeGreaterThan(asks);
    expect(runs().queryByRole("button")).not.toBeInTheDocument();
    expect(runs().getByText("None started here")).toBeInTheDocument();
  } finally {
    vi.useRealTimers();
  }
});


test("a connection gap replaces step activity with amber evidence and a gap timer, then re-attaches", async () => {
  const gap = fixtures.runBatches.stepGap;
  const lostAt = gap.state.connectionGap!.lostAtMs;
  vi.useFakeTimers();
  vi.setSystemTime(lostAt + 12_000);
  try {
    await startRun();
    await act(async () => send(gap));
    const message = screen.getByText(/Connection lost during step 1 at/);
    expect(message).toHaveClass("tone-attention");
    expect(message.querySelector("time")).toHaveAttribute("dateTime", new Date(lostAt).toISOString());
    await act(async () => vi.advanceTimersByTime(1000));
    expect(message).toHaveTextContent("Connection gap · 13s");
    expect(screen.getByText("Shipslip will reconnect to watch the step and will not run it again.")).toBeInTheDocument();
    const steps = within(screen.getByRole("region", { name: "Steps" }));
    expect(steps.queryByRole("img", { name: "running now" })).toBeNull();
    expect(steps.getByRole("img", { name: "not known" })).toHaveClass("mark-attention");
    expect(screen.queryByText(/^Step 1 of 3 ·/)).toBeNull();
    await act(async () => send(fixtures.runBatches.stepReattached));
    expect(screen.queryByText(/Connection lost during/)).toBeNull();
    expect(steps.getByRole("img", { name: "running now" })).toBeInTheDocument();
    expect(steps.getByText(/re-attached at/)).toHaveClass("muted");
    expect(screen.getByText(/^Step 1 of 3 ·/)).toBeInTheDocument();
  } finally {
    vi.useRealTimers();
  }
});

test("a log-watch gap hides the countdown and clears when observation resumes", async () => {
  await startRun();
  await act(async () => send(fixtures.runBatches.watchGap));
  expect(screen.getByText(/Connection lost during the log watch at/)).toHaveClass("tone-attention");
  expect(screen.getByText("Shipslip will reconnect to watch the log. It will not run any step again.")).toBeInTheDocument();
  expect(screen.queryByText(/^[\d:]+ left$/)).toBeNull();
  expect(screen.queryByText(/^Watching the log ·/)).toBeNull();
  expect(screen.getByRole("region", { name: "Steps" }).querySelector(".mark-spin")).toBeNull();
  await act(async () => send(fixtures.runBatches.watchReattached));
  expect(screen.queryByText(/Connection lost during/)).toBeNull();
  expect(screen.getByText(/^Watching the log ·/)).toBeInTheDocument();
  const watch = within(screen.getByRole("region", { name: "Log watch" }));
  expect(watch.getByText(/re-attached at/)).toHaveClass("muted");
  expect(watch.getByText(/^[\d:]+ left$/)).toBeInTheDocument();
});

test.each(["Down", "Up"] as const)("a maintenance %s gap replaces that phase's running indicator", async (phase) => {
  await startRun();
  const batch = fixtures.runBatches.stepGap;
  const progress = { ...batch.state.steps["1"], name: null };
  await act(async () => send({
    ...batch,
    state: {
      ...batch.state,
      steps: {},
      maintenanceDown: phase === "Down" ? progress : null,
      maintenanceUp: phase === "Up" ? progress : null,
      connectionGap: { ...batch.state.connectionGap!, target: { kind: "maintenance", phase } },
    },
  }));
  expect(screen.getByText(new RegExp(`Connection lost during maintenance ${phase === "Down" ? "on" : "off"} at`)))
    .toHaveClass("tone-attention");
  const steps = within(screen.getByRole("region", { name: "Steps" }));
  expect(steps.getByRole("img", { name: "not known" })).toBeInTheDocument();
  expect(steps.queryByRole("img", { name: "running now" })).toBeNull();
});

test("a known outcome stays above a gap during its log watch", async () => {
  await startRun();
  await act(async () => send({
    ...fixtures.runBatches.failedWatching,
    state: { ...fixtures.runBatches.failedWatching.state, connectionGap: fixtures.runBatches.watchGap.state.connectionGap },
  }));
  const headline = screen.getByRole("heading", { name: /^Failed at step 1/ });
  expect(headline).toHaveClass("tone-bad");
  const gap = screen.getByText(/Connection lost during the log watch at/);
  expect(headline.compareDocumentPosition(gap) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(screen.queryByText(/^Watching the log ·/)).toBeNull();
});
