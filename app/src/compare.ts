/** A picked comparison: env `a` (the source column) against env `b`. */
export interface Pair {
  a: string;
  b: string;
}

export type CompareLine =
  | { status: "pending" }
  /** Still waiting after 5 s. */
  | { status: "slow" }
  | { status: "done"; text: string; available: boolean };

const STORAGE_KEY = "shipslip.compare";
export const SLOW_AFTER_MS = 5000;

/** Every ordered pair of envs, for the picker. */
export function pairs(envs: string[]): Pair[] {
  return envs.flatMap((a) => envs.filter((b) => b !== a).map((b) => ({ a, b })));
}

/** The remembered pair, only when both envs are in this map. */
export function loadPair(envs: string[]): Pair | null {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
    if (typeof value?.a === "string" && typeof value?.b === "string") {
      return envs.includes(value.a) && envs.includes(value.b) && value.a !== value.b ? value : null;
    }
  } catch {
    // Unreadable: start empty.
  }
  return null;
}

export function storePair(pair: Pair | null) {
  if (pair) {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(pair));
  } else {
    localStorage.removeItem(STORAGE_KEY);
  }
}
