import { isTime, timeOfDay, utcStamp } from "./time";
import type { StepTiming } from "./types";

/** A step's recorded duration, or when it was sent if its end was not seen. */
export function StepTime({ timing, className }: { timing: StepTiming; className: string }) {
  if (timing.kind === "took") return <span className={className}>{timing.duration}</span>;
  const at = timing.startedAtMs;
  return (
    <span className={className}>
      started{" "}
      {isTime(at) ? (
        <time dateTime={new Date(at).toISOString()} title={utcStamp(at)}>
          {timeOfDay(at)}
        </time>
      ) : (
        "at a time not known"
      )}
      , end not known
    </span>
  );
}
