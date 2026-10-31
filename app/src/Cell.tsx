import { useState } from "react";
import type { CompareLine } from "./compare";
import { Mark, markLabel } from "./Mark";
import { lowerFirst } from "./text";
import { elapsed } from "./time";
import type { CellView, EnvCellView, FlagView, OutcomeView, RunInfo, Tone } from "./types";

const toneClass: Record<Tone, string> = { good: "tone-good", bad: "tone-bad", neutral: "" };

export type StatusTone = "ink" | "bad" | "attention" | "running" | "muted";

export interface Status {
  word: string;
  tone: StatusTone;
}

const has = (flags: FlagView[], kind: FlagView["kind"]) => flags.some((flag) => flag.kind === kind);

/** A run that did not succeed, in one word. Red only for a step that failed;
 * a stop, a lost result or possible maintenance mode is amber. */
function ended(outcome: OutcomeView, flags: FlagView[], last: boolean): Status | null {
  if (outcome.tone !== "bad") return null;
  if (outcome.kind === "failed") return { word: last ? "Last failed" : "Failed", tone: "bad" };
  if (outcome.kind === "stoppedInMaintenance" || has(flags, "appLeftDown")) {
    return { word: "Maintenance?", tone: "attention" };
  }
  switch (outcome.kind) {
    case "stopped":
      return { word: last ? "Last stopped" : "Stopped", tone: "attention" };
    case "unknown":
      return { word: last ? "Last unknown" : "Not known", tone: "attention" };
    default:
      return { word: "Unfinished", tone: "attention" };
  }
}

/** The one word a board row shows for a cell. The latest risk wins over an
 * older success: a failed last attempt outranks the success it sits on. */
export function status(cell: EnvCellView, live?: string): Status {
  if (live) return { word: "Running", tone: "running" };
  if (cell.mark === "notKnown") return { word: "Not known", tone: "attention" };
  const last = cell.lastAttempt && ended(cell.lastAttempt.outcome, cell.lastAttempt.flags, true);
  if (last) return last;
  const own = cell.outcome && ended(cell.outcome, cell.flags, false);
  if (own) return cell.mark === "incomplete" && own.tone === "bad" ? { word: "Incomplete", tone: "bad" } : own;
  if (has(cell.flags, "newErrors")) return { word: "New errors", tone: "bad" };
  if (has(cell.flags, "smokeFailed")) return { word: "Smoke failed", tone: "bad" };
  if (has(cell.flags, "appLeftDown")) return { word: "Maintenance?", tone: "attention" };
  if (cell.qualifiers.length > 0) return { word: "Out of date?", tone: "attention" };
  if (cell.targetMoved) return { word: "Target moved", tone: "attention" };
  if (cell.needsAttention) return { word: "Check", tone: "attention" };
  if (cell.mark === "noCode") return { word: "No code", tone: "muted" };
  if (cell.outcome?.tone === "good") return { word: "Succeeded", tone: "ink" };
  return { word: "Recorded", tone: "ink" };
}

/** Why a cell reads as it does, in one sentence: the row's second line and
 * the sidebar's reason. */
export function reason(cell: EnvCellView, live?: string): { text: string; className: string } | null {
  if (live) return { text: live, className: "muted" };
  const last = cell.lastAttempt;
  if (last && last.outcome.tone === "bad") {
    return { text: `Last: ${lowerFirst(last.outcome.text)} · ${last.age}`, className: "" };
  }
  if (cell.qualifiers.length > 0) return { text: cell.qualifiers[0], className: "tone-attention sentence" };
  if (cell.targetMoved) return { text: cell.targetMoved, className: "tone-attention sentence" };
  const flag = cell.flags.find((f) => f.bad);
  if (flag) return { text: flag.text, className: "sentence" };
  const state = stateLine(cell);
  // "Succeeded" under a "Succeeded" status word says nothing new.
  // Success stays quiet ink: a green line would pull the eye to the healthy cell.
  if (state) {
    const word = status(cell).word;
    if (state.text === word) return null;
    const rest = state.text.startsWith(`${word} · `) ? state.text.slice(word.length + 3) : null;
    const text = rest ? rest[0].toUpperCase() + rest.slice(1) : state.text;
    return { text, className: state.className === "tone-good" ? "muted" : state.className };
  }
  return cell.words ? { text: cell.words, className: "muted sentence" } : null;
}

/** A split-flap status cell. A new word turns in over two frames; reduced
 * motion swaps it at once. */
export function Flap({ status }: { status: Status }) {
  const [shown, setShown] = useState(status.word);
  const [turns, setTurns] = useState(0);
  if (shown !== status.word) {
    setShown(status.word);
    setTurns(turns + 1);
  }
  return (
    <span key={turns} className={`flap flap-${status.tone}${turns > 0 ? " flap-turn" : ""}`}>
      {status.word}
    </span>
  );
}

/** How the code got there, in the run's own words. */
export function stateLine(cell: EnvCellView): { text: string; className: string } | null {
  const outcome = cell.outcome?.text;
  switch (cell.mark) {
    case "recorded":
    case "incomplete":
      return {
        text: [outcome, ...cell.evidence].filter(Boolean).join(" · "),
        className: toneClass[cell.outcome?.tone ?? "neutral"],
      };
    case "observed": {
      const [after, ...rest] = cell.evidence;
      return { text: [`${cell.words} ${after}`, ...rest].join(" · "), className: "tone-bad" };
    }
    case "notKnown":
      return {
        text: [outcome, ...cell.evidence].filter(Boolean).join(" · "),
        className: "tone-attention",
      };
    case "noCode": {
      const text = [outcome, ...cell.evidence].filter(Boolean).join(" · ");
      if (!text) return null;
      return { text, className: cell.receipt === null ? "muted sentence" : "sentence" };
    }
  }
}

function CompareRow({ line }: { line: CompareLine }) {
  const { text, muted } = compareText(line);
  return (
    <div className={`cell-line cell-compare${muted ? " muted" : ""}`}>
      {text}
    </div>
  );
}

function compareText(line: CompareLine): { text: string; muted: boolean } {
  switch (line.status) {
    case "pending":
      return { text: "comparing…", muted: true };
    case "slow":
      return { text: "still comparing (git is slow)", muted: true };
    case "done":
      return { text: line.text, muted: !line.available };
  }
}

/** A live cell's outcome line, e.g. "Deploying… 2m 14s". */
export function liveText(run: Pick<RunInfo, "operation" | "startedAtMs">, now: number): string {
  const doing = { deploy: "Deploying…", up: "Bringing app up…", attach: "Attaching…" }[run.operation];
  return `${doing} ${elapsed(now - run.startedAtMs)}`;
}

/** What a screen reader says for a cell: where it is, then the cell's lines
 * in reading order, e.g. "shop, production, recorded code a1b2c3d, 3d ago,
 * Succeeded". The target and the receipt's details are left to the pane. */
export function cellName(
  label: string,
  env: string,
  cell: CellView,
  compareLine?: CompareLine,
  live?: string,
): string {
  if (cell.kind === "absent") return `${label}, ${env}, none`;
  const parts = [label, env];
  if (live) parts.push(live);
  parts.push(cell.shortSha ? `${markLabel[cell.mark]} ${cell.shortSha}` : cell.words);
  if (cell.age) parts.push(cell.age);
  parts.push(...cell.qualifiers);
  const state = stateLine(cell);
  if (state) parts.push(state.text);
  if (compareLine) parts.push(compareText(compareLine).text);
  parts.push(...cell.flags.map((flag) => flag.text));
  if (cell.targetMoved) parts.push(cell.targetMoved);
  const last = cell.lastAttempt;
  if (last) {
    parts.push(`last attempt: ${lowerFirst(last.outcome.text)}, ${last.age}`);
    parts.push(...last.flags.filter((flag) => flag.bad).map((flag) => flag.text));
  }
  return parts.join(", ");
}

interface Props {
  cell: EnvCellView;
  selected?: boolean;
  /** Only in the source column of a picked pair. */
  compareLine?: CompareLine;
  /** A run going now: its outcome line. */
  live?: string;
  /** The selected row says why each cell reads as it does. */
  expanded?: boolean;
}

/** One board line: mark, status word, commit, age. */
export function Cell({ cell, selected = false, compareLine, live, expanded = false }: Props) {
  const never = cell.mark === "noCode" && cell.receipt === null;
  const why = expanded ? reason(cell, live) : null;
  return (
    <div
      className={`cell${cell.mark === "notKnown" ? " cell-hatched" : ""}${never ? " cell-empty" : ""}${selected ? " cell-selected" : ""}`}
    >
      <div className="cell-head">
        <Mark kind={live ? "live" : cell.mark} muted={never && !live} />
        <Flap status={status(cell, live)} />
        <span className="cell-sha mono" title={cell.sha ?? undefined}>
          {cell.shortSha ?? "—"}
        </span>
        <span className="cell-age muted">{cell.age ?? ""}</span>
      </div>
      {live && !expanded && <div className="cell-line muted cell-live">{live}</div>}
      {compareLine && <CompareRow line={compareLine} />}
      {why && (
        <div className={`cell-line cell-reason ${why.className}`} title={why.text}>
          {why.text}
        </div>
      )}
    </div>
  );
}
