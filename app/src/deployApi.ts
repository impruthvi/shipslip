// Deploying from the app: the commands behind the workspace. Separate from
// `api` so map screens and their tests need not know about it.

import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  DeployEntry,
  DeployReview,
  PrepareProgress,
  RunBatch,
  RunInfo,
  RunPlan,
} from "./types";

/** Rejections carry a `CommandError`. */
export interface DeployBackend {
  entry(generation: number, row: number, env: string): Promise<DeployEntry>;
  /** Connects, fetches, checks and takes the server lock; `onProgress` hears each phase. */
  prepare(
    generation: number,
    row: number,
    env: string,
    plan: RunPlan,
    onProgress: (phase: PrepareProgress) => void,
  ): Promise<DeployReview>;
  /** Starts the reviewed run. Production needs the env name typed. */
  confirm(handle: string, typedEnv: string | null): Promise<RunInfo>;
  /** Releases a review's server lock. */
  cancel(handle: string): Promise<void>;
  /** Streams a run to `onBatch`, acknowledging each batch after it is handled.
   * Resolves to an unsubscribe. */
  follow(handle: string, onBatch: (batch: RunBatch) => void): Promise<() => void>;
  stop(handle: string, stop: "afterStep" | "watch"): Promise<void>;
}

const tauriDeploy: DeployBackend = {
  entry: (generation, row, env) => invoke<DeployEntry>("deploy_entry", { generation, row, env }),
  prepare: (generation, row, env, plan, onProgress) => {
    const progress = new Channel<PrepareProgress>();
    progress.onmessage = onProgress;
    return invoke<DeployReview>("prepare_deploy", { generation, row, env, plan, progress });
  },
  confirm: (handle, typedEnv) => invoke<RunInfo>("confirm_deploy", { handle, typedEnv }),
  cancel: (handle) => invoke("cancel_deploy", { handle }),
  follow: async (handle, onBatch) => {
    const channel = new Channel<RunBatch>();
    channel.onmessage = (batch) => {
      onBatch(batch);
      // A run that has gone has nothing more to send.
      invoke("ack_run", { handle, subscription: batch.subscription, through: batch.through }).catch(
        () => {},
      );
    };
    const subscription = await invoke<string>("subscribe_run", { handle, channel });
    return () => {
      invoke("unsubscribe_run", { handle, subscription }).catch(() => {});
    };
  },
  stop: (handle, stop) => invoke("stop_run", { handle, stop }),
};

let backend: Promise<DeployBackend> | undefined;

// A plain browser (vite dev without Tauri) gets fixture data instead.
const inBrowser = () => import.meta.env.DEV && !("__TAURI_INTERNALS__" in window);

function getBackend(): Promise<DeployBackend> {
  if (inBrowser()) {
    backend ??= import("./preview").then((preview) => preview.deploy);
  }
  backend ??= Promise.resolve(tauriDeploy);
  return backend;
}

export const deploy: DeployBackend = {
  entry: async (...args) => (await getBackend()).entry(...args),
  prepare: async (...args) => (await getBackend()).prepare(...args),
  confirm: async (...args) => (await getBackend()).confirm(...args),
  cancel: async (handle) => (await getBackend()).cancel(handle),
  follow: async (...args) => (await getBackend()).follow(...args),
  stop: async (...args) => (await getBackend()).stop(...args),
};
