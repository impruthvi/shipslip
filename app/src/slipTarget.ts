import type { EnvCellView } from "./types";

/** Which of a cell's receipts the pane shows. Cells open the code run;
 * the "Last:" line and the unreadable-newer qualifier open the others. */
export type PaneView = "code" | "last" | "stale";

export interface Target {
  /** `null`: the cell has no receipt to open. */
  id: number | null;
  /** What the shown receipt is to the cell. */
  role: "code" | "last" | "stale" | null;
}

export function target(cell: EnvCellView, view: PaneView): Target {
  if (view === "stale" && cell.staleReceipt !== null) {
    return { id: cell.staleReceipt, role: "stale" };
  }
  if (view === "last" && cell.lastAttempt) {
    return { id: cell.lastAttempt.receipt, role: "last" };
  }
  if (cell.receipt !== null) {
    return { id: cell.receipt, role: cell.mark === "noCode" ? null : "code" };
  }
  if (cell.lastAttempt) {
    return { id: cell.lastAttempt.receipt, role: "last" };
  }
  return { id: null, role: null };
}

export interface ContextLink {
  text: string;
  /** The button's spoken name; "open" alone says too little. */
  label: string;
  view: PaneView;
}

export interface Context {
  text: string;
  /** The cell's other receipts. Keyboard users reach them only from here. */
  links: ContextLink[];
}

const TO_CODE: ContextLink = { text: "open", label: "Open the receipt behind the recorded code", view: "code" };
const TO_LAST: ContextLink = { text: "latest attempt", label: "Open the latest attempt", view: "last" };
const TO_STALE: ContextLink = {
  text: "newer unreadable receipt",
  label: "Open the newer receipt that could not be read",
  view: "stale",
};

/** The pencil line above the slip. */
export function context(cell: EnvCellView, role: Target["role"]): Context | null {
  const hasCode = cell.receipt !== null && cell.mark !== "noCode";
  const links = (...candidates: [boolean, ContextLink][]) =>
    candidates.flatMap(([ok, link]) => (ok ? [link] : []));
  const last = cell.lastAttempt !== null;
  const stale = cell.staleReceipt !== null;
  const earlier = hasCode ? " · recorded code comes from an earlier run" : "";
  switch (role) {
    case "code":
      return { text: "Receipt behind the recorded code", links: links([last, TO_LAST], [stale, TO_STALE]) };
    case "last":
      return { text: `Latest attempt${earlier}`, links: links([hasCode, TO_CODE], [stale, TO_STALE]) };
    case "stale":
      return {
        text: `Newer receipt that could not be read${earlier}`,
        links: links([hasCode, TO_CODE], [last, TO_LAST]),
      };
    case null:
      return null;
  }
}
