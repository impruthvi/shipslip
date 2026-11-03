import { isTime, timeOfDay, utcStamp } from "./time";

/** A recorded local observation, with its UTC timestamp on hover. */
export function RecordedTime({ at }: { at: number }) {
  return isTime(at) ? (
    <time dateTime={new Date(at).toISOString()} title={utcStamp(at)}>{timeOfDay(at)}</time>
  ) : <>a time not known</>;
}
