import { beforeEach, expect, test, vi } from "vitest";

const invoke = vi.fn();
class Channel<T> {
  onmessage: (message: T) => void = () => {};
}
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel }));
vi.unmock("./deployApi");

beforeEach(() => {
  vi.resetModules();
  invoke.mockReset().mockResolvedValue(undefined);
  Object.assign(window, { __TAURI_INTERNALS__: {} });
});

test("deploy commands get the argument names Rust expects", async () => {
  const { deploy } = await import("./deployApi");
  const phases: unknown[] = [];
  await deploy.entry(3, 1, "production");
  await deploy.prepare(3, 1, "production", "Deploy", (phase) => phases.push(phase));
  await deploy.confirm("review-1", "production");
  await deploy.cancel("review-1");
  await deploy.stop("run-1", "afterStep");
  const [prepare] = invoke.mock.calls.filter(([name]) => name === "prepare_deploy");
  prepare[1].progress.onmessage({ phase: "checking" });
  expect(phases).toEqual([{ phase: "checking" }]);
  expect(invoke.mock.calls.map(([name, args]) => [name, { ...args, progress: undefined }])).toEqual([
    ["deploy_entry", { generation: 3, row: 1, env: "production" }],
    ["prepare_deploy", { generation: 3, row: 1, env: "production", plan: "Deploy" }],
    ["confirm_deploy", { handle: "review-1", typedEnv: "production" }],
    ["cancel_deploy", { handle: "review-1" }],
    ["stop_run", { handle: "run-1", stop: "afterStep" }],
  ]);
});

test("following a run acks each batch after handling it, and unsubscribes", async () => {
  const { deploy } = await import("./deployApi");
  invoke.mockImplementation(async (name: string) => (name === "subscribe_run" ? "sub-1" : undefined));
  const seen: number[] = [];
  const unfollow = await deploy.follow("run-1", (batch) => seen.push(batch.through));
  const [, { channel }] = invoke.mock.calls.find(([name]) => name === "subscribe_run")!;
  channel.onmessage({ subscription: "sub-1", through: 7 });
  expect(seen).toEqual([7]);
  unfollow();
  expect(invoke.mock.calls.slice(1)).toEqual([
    ["ack_run", { handle: "run-1", subscription: "sub-1", through: 7 }],
    ["unsubscribe_run", { handle: "run-1", subscription: "sub-1" }],
  ]);
});

test("a run that has gone does not throw when acked or unsubscribed", async () => {
  const { deploy } = await import("./deployApi");
  invoke.mockImplementation(async (name: string) => {
    if (name === "subscribe_run") return "sub-1";
    throw { code: "notFound", message: "This run is no longer retained." };
  });
  const unfollow = await deploy.follow("run-1", () => {});
  const [, { channel }] = invoke.mock.calls.find(([name]) => name === "subscribe_run")!;
  channel.onmessage({ subscription: "sub-1", through: 1 });
  unfollow();
  await new Promise((resolve) => setTimeout(resolve, 0));
});
