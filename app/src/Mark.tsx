import type { Mark as MarkKind } from "./types";

export const markLabel: Record<MarkKind, string> = {
  recorded: "recorded code",
  incomplete: "deploy incomplete",
  observed: "observed after a failed step",
  notKnown: "not known",
  noCode: "no code recorded",
};

/** The shape that tells cell states apart without color. */
export function Mark({ kind, size = 10, muted = false }: { kind: MarkKind; size?: number; muted?: boolean }) {
  const color = muted ? "var(--color-text-muted)" : "var(--color-text)";
  return (
    <svg
      className="mark"
      width={size}
      height={size}
      viewBox="0 0 10 10"
      role="img"
      aria-label={markLabel[kind]}
    >
      {kind === "recorded" && <circle cx="5" cy="5" r="5" fill={color} />}
      {kind === "incomplete" && (
        <>
          <circle cx="5" cy="5" r="4.25" fill="none" stroke={color} strokeWidth="1.5" />
          <path d="M5 0 A5 5 0 0 0 5 10 Z" fill={color} />
        </>
      )}
      {kind === "observed" && (
        <circle cx="5" cy="5" r="4.25" fill="none" stroke={color} strokeWidth="1.5" />
      )}
      {kind === "notKnown" && (
        <circle
          cx="5"
          cy="5"
          r="4.25"
          fill="none"
          stroke={color}
          strokeWidth="1.5"
          strokeDasharray="2 2"
        />
      )}
      {kind === "noCode" && <path d="M0 5 H10" fill="none" stroke={color} strokeWidth="1.5" />}
    </svg>
  );
}
