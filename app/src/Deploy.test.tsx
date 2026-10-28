import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import * as fixtures from "./fixtures.gen";
import type {
  CommandError,
  DeployEntry,
  DeployReview,
  LoadResult,
  PrepareProgress,
  RunBatch,
  RunInfo,
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

beforeEach(() => {
  localStorage.clear();
  loadMap.mockReset().mockResolvedValue(fixtures.map);
  entry.mockReset().mockResolvedValue({ kind: "ready", production: true, needsTrust: false });
  prepare.mockReset();
  confirm.mockReset();
  cancel.mockReset().mockResolvedValue(undefined);
  stop.mockReset().mockResolvedValue(undefined);
  unfollow.mockReset();
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

test("a refusal shows the reason and the command to copy, announced at once", async () => {
  const { pending } = await openWorkspace();
  await act(async () => pending.reject(fixtures.refusal));
  expect(within(screen.getByRole("main")).getByText(fixtures.refusal.message)).toBeInTheDocument();
  expect(screen.getByText("ssh-add --apple-use-keychain")).toBeInTheDocument();
  expect(screen.getByTestId("announce-assertive")).toHaveTextContent(fixtures.refusal.message);
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
  entry.mockResolvedValue({ kind: "ready", production: false, needsTrust: false });
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

test("nothing to ship says so and offers no deploy", async () => {
  const { pending } = await openWorkspace();
  await act(async () => pending.resolve({ ...production, commits: [], moreCommits: 0 }));
  expect(screen.getByText("Already up to date")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /^Deploy to/ })).not.toBeInTheDocument();
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

  // Running ignores Escape and has no way back to the map yet.
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.getByRole("region", { name: "Steps" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Release map" })).not.toBeInTheDocument();

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
