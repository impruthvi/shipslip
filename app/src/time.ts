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

/** Time of day in local time with its zone, e.g. "19:51 IST". */
export function timeOfDay(ms: number, timeZone?: string, zoneLocale?: string): string {
  const p = parts(ms, timeZone, zoneLocale);
  return `${p.time}${p.zone ? ` ${p.zone}` : ""}`;
}

/** The same moment in UTC, for hover: "2026-10-06 14:21 UTC". */
export function utcStamp(ms: number): string {
  return `${new Date(ms).toISOString().slice(0, 16).replace("T", " ")} UTC`;
}

/** A step's running time: "8s", "2m 14s", "1h 3m". */
export function elapsed(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** Time left as m:ss, e.g. "1:42". */
export function countdown(ms: number): string {
  const seconds = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** For screen readers: "12 seconds", "2 minutes 14 seconds". */
export function spoken(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  const unit = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  if (seconds < 60) return unit(seconds, "second");
  const rest = seconds % 60;
  return `${unit(Math.floor(seconds / 60), "minute")}${rest ? ` ${unit(rest, "second")}` : ""}`;
}
