import type { CompareLine } from "./compare";
import { Mark } from "./Mark";
import { lowerFirst } from "./text";
import type { EnvCellView, FlagView, Tone } from "./types";
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
        className: "tone-bad",
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

function Warning({ text }: { text: string }) {
  return (
    <div className="cell-line cell-warning">
      <WarningIcon />
      <span className="sentence">{text}</span>
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

interface Props {
  cell: EnvCellView;
  selected?: boolean;
  /** Slot 3, only in the source column of a picked pair. */
  compareLine?: CompareLine;
}

export function Cell({ cell, selected = false, compareLine }: Props) {
  const never = cell.mark === "noCode" && cell.receipt === null;
  const state = stateLine(cell);
  const last = cell.lastAttempt;
  return (
    <div
      className={`cell${cell.mark === "notKnown" ? " cell-hatched" : ""}${never ? " cell-empty" : ""}${selected ? " cell-selected" : ""}`}
    >
      <div className="cell-head">
        <Mark kind={cell.mark} muted={never} />
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
      {cell.qualifiers.map((text) => (
        <Warning key={text} text={text} />
      ))}
      {state && <div className={`cell-line ${state.className}`}>{state.text}</div>}
      {compareLine && <CompareRow line={compareLine} />}
      {cell.flags.map((flag) => (
        <Flag key={flag.text} flag={flag} />
      ))}
      {cell.target && <div className="cell-line cell-target mono muted">{cell.target}</div>}
      {cell.targetMoved && <Warning text={cell.targetMoved} />}
      {last && (
        <>
          <div className="cell-tear" />
          <div className="cell-line cell-last muted">
            Last: {lowerFirst(last.outcome.text)} · {last.age}
          </div>
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
