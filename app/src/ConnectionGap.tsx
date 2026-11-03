import { RecordedTime } from "./RecordedTime";
import { elapsed } from "./time";
import type { ConnectionGapView, ConnectionTargetView } from "./types";

function targetName(target: ConnectionTargetView): string {
  switch (target.kind) {
    case "step": return `step ${target.index}`;
    case "maintenance": return target.phase === "Down" ? "maintenance on" : "maintenance off";
    case "logWatch": return "the log watch";
  }
}

export function ConnectionGap({ gap, now }: { gap: ConnectionGapView; now: number }) {
  return (
    <span className="tone-attention">
      Connection lost during {targetName(gap.target)} at <RecordedTime at={gap.lostAtMs} />
      {" · Connection gap · "}{elapsed(now - gap.lostAtMs)}
    </span>
  );
}

export function ReattachedNote({ at }: { at: number }) {
  return <span className="muted">re-attached at <RecordedTime at={at} /></span>;
}
