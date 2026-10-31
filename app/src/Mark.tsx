import type { Mark as MarkKind } from "./types";

export const markLabel: Record<MarkKind | "live", string> = {
  live: "running now",
  recorded: "recorded code",
  incomplete: "deploy incomplete",
  observed: "observed after a failed step",
  notKnown: "not known",
  noCode: "no code recorded",
};

/** Shapes and hues on one fixed severity ramp, so states never depend on color
 * alone: not known is an amber diamond and running a blue spinning ring, never
 * the same shape. The ring is the only mark that moves. */
const markTone: Record<MarkKind | "live", string> = {
  live: "mark-running",
  recorded: "mark-ink",
  incomplete: "mark-attention",
  observed: "mark-ink",
  notKnown: "mark-attention",
  noCode: "mark-muted",
};

export function Mark({
  kind,
  size = 10,
  muted = false,
}: {
  kind: MarkKind | "live";
  size?: number;
  muted?: boolean;
}) {
  return (
    <svg
      className={`mark ${muted ? "mark-muted" : markTone[kind]}`}
      width={size}
      height={size}
      viewBox="0 0 10 10"
      role="img"
      aria-label={markLabel[kind]}
    >
      {kind === "recorded" && <circle cx="5" cy="5" r="4.5" fill="currentColor" />}
      {kind === "incomplete" && (
        <>
          <circle cx="5" cy="5" r="4.25" fill="none" stroke="currentColor" strokeWidth="1.5" />
          <path d="M5 0 A5 5 0 0 0 5 10 Z" fill="currentColor" />
        </>
      )}
      {kind === "observed" && (
        <circle cx="5" cy="5" r="4.25" fill="none" stroke="currentColor" strokeWidth="1.5" />
      )}
      {kind === "notKnown" && <Diamond />}
      {kind === "live" && <Ring />}
      {kind === "noCode" && <path d="M1 5 H9" fill="none" stroke="currentColor" strokeWidth="1.5" />}
    </svg>
  );
}

function Diamond() {
  return (
    <>
      <path d="M5 0.9 L9.1 5 L5 9.1 L0.9 5 Z" fill="none" stroke="currentColor" strokeWidth="1.4" />
      <circle cx="5" cy="5" r="1.3" fill="currentColor" />
    </>
  );
}

function Ring() {
  return (
    <>
      <circle cx="5" cy="5" r="4" fill="none" stroke="currentColor" strokeWidth="1.5" opacity="0.3" />
      <path className="mark-spin" d="M5 1 A4 4 0 0 1 9 5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </>
  );
}

export type StepIconKind = "ok" | "failed" | "unknown" | "running" | "notRun" | "skipped";

const stepIconLabel: Record<StepIconKind, string> = {
  ok: "done",
  failed: "failed",
  unknown: "result unknown",
  running: "running",
  notRun: "did not run",
  skipped: "skipped",
};

const stepIconTone: Record<StepIconKind, string> = {
  ok: "mark-good",
  failed: "mark-bad",
  unknown: "mark-attention",
  running: "mark-running",
  notRun: "mark-muted",
  skipped: "mark-muted",
};

/** A step's outcome in receipts and the run screen, drawn on the same ramp as
 * the map marks. */
export function StepIcon({ kind, label = stepIconLabel[kind] }: { kind: StepIconKind; label?: string }) {
  return (
    <svg
      className={`mark step-icon ${stepIconTone[kind]}`}
      width="12"
      height="12"
      viewBox="0 0 10 10"
      role="img"
      aria-label={label}
    >
      {kind === "ok" && (
        <path d="M1.5 5.2 L4 7.6 L8.6 2.6" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
      )}
      {kind === "failed" && <rect x="1.5" y="1.5" width="7" height="7" rx="1" fill="currentColor" />}
      {kind === "unknown" && <Diamond />}
      {kind === "running" && <Ring />}
      {kind === "notRun" && <path d="M2.5 5 H7.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />}
      {kind === "skipped" && (
        <path d="M2 7 A3.2 3.2 0 0 1 8 5 M8 2.6 V5.2 H5.4" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
      )}
    </svg>
  );
}
