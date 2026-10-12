/** Local wall-clock time, e.g. "10:42". */
export function clock(date: Date): string {
  return date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hour12: false });
}
