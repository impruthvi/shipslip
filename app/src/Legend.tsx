import { SourceMarker, TargetMarker } from "./ComparePicker";
import { Mark, markLabel } from "./Mark";
import type { Mark as MarkKind } from "./types";

const order: MarkKind[] = ["recorded", "incomplete", "observed", "notKnown", "noCode"];

export function Legend({ compare = false }: { compare?: boolean }) {
  return (
    <ul className="legend" aria-label="Key">
      {order.map((kind) => (
        <li key={kind}>
          <Mark kind={kind} size={9} muted={kind === "noCode"} />
          <span className="muted">{markLabel[kind]}</span>
        </li>
      ))}
      {compare && (
        <li className="legend-compare">
          <SourceMarker />
          <span className="muted">compared from</span>
          <TargetMarker />
          <span className="muted">compared to</span>
        </li>
      )}
    </ul>
  );
}
