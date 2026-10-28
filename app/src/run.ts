// What the Running screen draws, worked out from `RunBatch`es.

import type {
  DeployReview,
  OutputLine,
  OutputSource,
  Progress,
  RunBatch,
  RunState,
  Terminal,
} from "./types";

/** Lines kept in the window, as Rust keeps for replay. */
export const OUTPUT_LINES = 2000;

export interface RunLog {
  state: RunState;
  output: OutputLine[];
  /** Lines Rust dropped before this view saw them. */
  dropped: number;
  through: number;
}

/** Folds one batch in. A replay starts over; otherwise only newer lines are added. */
export function applyBatch(prev: RunLog | null, batch: RunBatch): RunLog {
  const kept = batch.replay || !prev ? [] : prev.output;
  const fresh = batch.output.filter((line) => line.sequence > (batch.replay ? 0 : batch.after));
  const output = [...kept, ...fresh].slice(-OUTPUT_LINES);
  const lost = kept.length + fresh.length - output.length;
  return {
    state: batch.state,
    output,
    dropped: batch.replay || !prev ? batch.droppedOutput + lost : prev.dropped + lost,
    through: batch.through,
  };
}

export interface RunRow {
  /** Matches the receipt's step index: "on", "0", "1"…, "off". */
  index: string;
  command: string;
  progress: Progress | null;
  owns: (source: OutputSource) => boolean;
}

/** Every step the run may take, in order, with what is known of each. */
export function runRows(review: DeployReview, state: RunState): RunRow[] {
  const rows: RunRow[] = [];
  if (review.maintenance) {
    rows.push({
      index: "on",
      command: "Maintenance on: php artisan down",
      progress: state.maintenanceDown,
      owns: (s) => s.kind === "maintenance" && s.phase === "Down",
    });
  }
  const step = (index: number, command: string): RunRow => ({
    index: String(index),
    command: state.steps[index]?.name ?? command,
    progress: state.steps[index] ?? null,
    owns: (s) => s.kind === "step" && s.index === index,
  });
  rows.push(step(0, `git fast-forward to ${review.targetSha.slice(0, 7)}`));
  review.steps.forEach((command, i) => rows.push(step(i + 1, command)));
  if (review.maintenance) {
    rows.push({
      index: "off",
      command: "Maintenance off: php artisan up",
      progress: state.maintenanceUp,
      owns: (s) => s.kind === "maintenance" && s.phase === "Up",
    });
  }
  return rows;
}

/** The log watch is on: started, not yet finished. */
export function watching(state: RunState): boolean {
  return state.watchWindowMs !== null && state.watch === null && state.terminal === null;
}

/** The one sentence a finished, stopped or lost run ends with. */
export function endSentence(terminal: Terminal): string {
  switch (terminal.kind) {
    case "finished":
      return terminal.summary;
    case "detached":
      return `Shipslip stopped watching during step ${terminal.step}. It may still be running on the server.`;
    case "interrupted":
      return `Lost the connection during step ${terminal.step}. It may still be running on the server.`;
    case "error":
      return terminal.reason;
  }
}
