import { expect, test } from "vitest";
import * as fixtures from "./fixtures.gen";
import { applyBatch, endSentence, OUTPUT_LINES, runRows, watching } from "./run";
import type { OutputLine, RunBatch } from "./types";

const line = (sequence: number): OutputLine => ({
  sequence,
  text: `line ${sequence}`,
  source: { kind: "step", index: 1 },
});
const batch = (over: Partial<RunBatch>): RunBatch => ({ ...fixtures.runBatches.running, ...over });

test("a replay starts over; later batches add only newer lines", () => {
  const first = applyBatch(null, batch({ replay: true, after: 0, through: 2, output: [line(1), line(2)] }));
  expect(first.output.map((l) => l.sequence)).toEqual([1, 2]);
  const next = applyBatch(first, batch({ replay: false, after: 2, through: 4, output: [line(2), line(3)] }));
  expect(next.output.map((l) => l.sequence)).toEqual([1, 2, 3]);
  expect(next.through).toBe(4);
  const again = applyBatch(next, batch({ replay: true, after: 0, through: 5, output: [line(5)], droppedOutput: 4 }));
  expect(again.output.map((l) => l.sequence)).toEqual([5]);
  expect(again.dropped).toBe(4);
});

test("the window keeps the newest lines and counts the ones it let go", () => {
  const many = Array.from({ length: OUTPUT_LINES }, (_, i) => line(i + 1));
  const full = applyBatch(null, batch({ replay: true, output: many, droppedOutput: 3 }));
  const more = applyBatch(full, batch({ replay: false, after: OUTPUT_LINES, output: [line(OUTPUT_LINES + 1)] }));
  expect(more.output).toHaveLength(OUTPUT_LINES);
  expect(more.output.at(-1)?.sequence).toBe(OUTPUT_LINES + 1);
  expect(more.dropped).toBe(4);
});

test("rows run maintenance on, step 0, the recipe, then maintenance off", () => {
  const rows = runRows(fixtures.deployReview, fixtures.runBatches.running.state);
  expect(rows.map((r) => [r.index, r.progress?.status ?? null])).toEqual([
    ["on", "Ok"],
    ["0", "Ok"],
    ["1", "Running"],
    ["2", null],
    ["3", null],
    ["off", null],
  ]);
  expect(rows[1].owns({ kind: "step", index: 0 })).toBe(true);
  expect(rows[1].owns({ kind: "step", index: 1 })).toBe(false);
  expect(rows[0].owns({ kind: "maintenance", phase: "Down" })).toBe(true);
  const plain = runRows({ ...fixtures.deployReview, maintenance: false }, fixtures.runBatches.running.state);
  expect(plain.map((r) => r.index)).toEqual(["0", "1", "2", "3"]);
});

test("watching is between the watch starting and any result", () => {
  expect(watching(fixtures.runBatches.watching.state)).toBe(true);
  expect(watching(fixtures.runBatches.running.state)).toBe(false);
  expect(watching(fixtures.runBatches.succeeded.state)).toBe(false);
});

test("each way a run ends has one sentence", () => {
  const finished = fixtures.runBatches.failed.state.terminal!;
  expect(endSentence(finished)).toBe("Deploy failed at step 1.");
  expect(endSentence({ kind: "detached", step: 2 })).toBe(
    "Shipslip stopped watching during step 2. It may still be running on the server.",
  );
  expect(endSentence({ kind: "interrupted", step: 3, reason: "x" })).toBe(
    "Lost the connection during step 3. It may still be running on the server.",
  );
  expect(endSentence({ kind: "error", reason: "The event stream closed." })).toBe(
    "The event stream closed.",
  );
});
