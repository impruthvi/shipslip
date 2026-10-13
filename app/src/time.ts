/** Local wall-clock time, e.g. "10:42". */
export function clock(date: Date): string {
  return date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hour12: false });
}

function parts(ms: number, timeZone: string | undefined, zoneLocale: string | undefined) {
  const date = new Intl.DateTimeFormat("en-GB", {
    day: "2-digit",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
    timeZone,
  }).formatToParts(ms);
  const zone = new Intl.DateTimeFormat(zoneLocale, { timeZoneName: "short", timeZone })
    .formatToParts(ms)
    .find((part) => part.type === "timeZoneName")?.value;
  const get = (type: Intl.DateTimeFormatPartTypes) =>
    date.find((part) => part.type === type)?.value ?? "";
  return { day: get("day"), month: get("month"), year: get("year"), time: `${get("hour")}:${get("minute")}`, zone };
}

/** False for a millisecond count no Date can hold (a corrupt receipt). */
export function isTime(ms: number): boolean {
  return Number.isFinite(new Date(ms).getTime());
}

/** A moment in local time with its zone, e.g. "06 Oct 2026 · 19:51 IST". */
export function stamp(ms: number, timeZone?: string, zoneLocale?: string): string {
  const p = parts(ms, timeZone, zoneLocale);
  return `${p.day} ${p.month} ${p.year} · ${p.time}${p.zone ? ` ${p.zone}` : ""}`;
}

/** The same moment in UTC, for hover: "2026-10-06 14:21 UTC". */
export function utcStamp(ms: number): string {
  return `${new Date(ms).toISOString().slice(0, 16).replace("T", " ")} UTC`;
}
