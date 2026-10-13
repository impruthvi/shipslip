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

export interface Context {
  text: string;
  /** "open" link back to the receipt behind the recorded code. */
  backToCode: boolean;
}

/** The pencil line above the slip. */
export function context(cell: EnvCellView, role: Target["role"]): Context | null {
  const canGoBack = cell.receipt !== null && cell.mark !== "noCode";
  switch (role) {
    case "code":
      return { text: "Receipt behind the recorded code", backToCode: false };
    case "last":
      return canGoBack
        ? { text: "Latest attempt · recorded code comes from an earlier run", backToCode: true }
        : { text: "Latest attempt", backToCode: false };
    case "stale":
      return canGoBack
        ? { text: "Newer receipt that could not be read · recorded code comes from an earlier run", backToCode: true }
        : { text: "Newer receipt that could not be read", backToCode: false };
    case null:
      return null;
  }
}
