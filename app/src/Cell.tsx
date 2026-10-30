import type { CompareLine } from "./compare";
import { Mark, markLabel } from "./Mark";
import type { PaneView } from "./slipTarget";
import { lowerFirst } from "./text";
import { elapsed } from "./time";
import type { CellView, EnvCellView, FlagView, RunInfo, Tone } from "./types";
import { WarningIcon } from "./Warning";

const toneClass: Record<Tone, string> = { good: "tone-good", bad: "tone-bad", neutral: "" };

/** Slot 2: how the code got there, in the run's own words. */
function stateLine(cell: EnvCellView): { text: string; className: string } | null {
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

function Flag({ flag }: { flag: FlagView }) {
  return (
    <div className="cell-line cell-flag">
      {flag.newErrors && <span className="chip">NEW</span>}
      <span className={flag.newErrors ? "" : flag.bad ? "tone-bad" : "muted"}>{flag.text}</span>
    </div>
  );
}

function Warning({ text, onOpen }: { text: string; onOpen?: () => void }) {
  return (
    <div className="cell-line cell-warning">
      <WarningIcon />
      {onOpen ? (
        <button type="button" className="cell-open sentence" tabIndex={-1} onClick={stop(onOpen)}>
          {text}
        </button>
      ) : (
        <span className="sentence">{text}</span>
      )}
    </div>
  );
}

/** A click on a line inside the cell opens that receipt, not the cell's own. */
function stop(handler: () => void) {
  return (event: { stopPropagation: () => void }) => {
    event.stopPropagation();
    handler();
  };
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
  /** Slot 3, only in the source column of a picked pair. */
  compareLine?: CompareLine;
  /** Opens one of the cell's other receipts in the pane. */
  onOpen?: (view: PaneView) => void;
  /** A run going now: its outcome line. */
  live?: string;
}

export function Cell({ cell, selected = false, compareLine, onOpen, live }: Props) {
  const never = cell.mark === "noCode" && cell.receipt === null;
  const state = stateLine(cell);
  const last = cell.lastAttempt;
  return (
    <div
      className={`cell${cell.mark === "notKnown" ? " cell-hatched" : ""}${never ? " cell-empty" : ""}${selected ? " cell-selected" : ""}`}
    >
      <div className="cell-head">
        <Mark kind={live ? "live" : cell.mark} muted={never && !live} />
        {cell.shortSha ? (
          <>
            <span className="cell-sha mono" title={cell.sha ?? undefined}>
              {cell.shortSha}
            </span>
            {cell.age && <span className="cell-age muted">{cell.age}</span>}
          </>
        ) : (
          <span className={`cell-words sentence${never ? " muted" : ""}`}>{cell.words}</span>
        )}
      </div>
      {cell.qualifiers.map((text, index) => (
        <Warning
          key={text}
          text={text}
          onOpen={index === 0 && cell.staleReceipt !== null && onOpen ? () => onOpen("stale") : undefined}
        />
      ))}
      {live ? (
        <div className="cell-line muted cell-live">{live}</div>
      ) : (
        state && <div className={`cell-line ${state.className}`}>{state.text}</div>
      )}
      {compareLine && <CompareRow line={compareLine} />}
      {cell.flags.map((flag) => (
        <Flag key={flag.text} flag={flag} />
      ))}
      {cell.target && <div className="cell-line cell-target mono muted">{cell.target}</div>}
      {cell.targetMoved && <Warning text={cell.targetMoved} />}
      {last && (
        <>
          <div className="cell-tear" />
          {onOpen ? (
            <button
              type="button"
              className="cell-line cell-last cell-open muted"
              tabIndex={-1}
              onClick={stop(() => onOpen("last"))}
            >
              Last: {lowerFirst(last.outcome.text)} · {last.age}
            </button>
          ) : (
            <div className="cell-line cell-last muted">
              Last: {lowerFirst(last.outcome.text)} · {last.age}
            </div>
          )}
          {last.flags
            .filter((flag) => flag.bad)
            .map((flag) => (
              <Warning key={flag.text} text={flag.text} />
            ))}
        </>
      )}
    </div>
  );
}
