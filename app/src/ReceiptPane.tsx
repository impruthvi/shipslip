import { useEffect, useRef, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { PANE_MAX, PANE_MIN } from "./pane";
import { context, type Context, type PaneView, type Target } from "./slipTarget";
import { Slip } from "./Slip";
import type { EnvCellView } from "./types";
import type { Opened, ReceiptState } from "./useReceipt";

const KEY_STEP = 20;

/** The cell a slip belongs to. */
export interface SlipMeta {
  cell: EnvCellView;
  label: string;
  env: string;
  role: Target["role"];
}

interface Props {
  /** The receipt being asked for; focus waits until the slip shows it. */
  requestKey: string;
  /** A keyboard open asked for focus on the slip. */
  focusSlip: boolean;
  onSlipFocused: () => void;
  /** The cell being opened: named while a slow read is under way, and its
   * receipts offered as tabs. */
  opening: SlipMeta;
  receipt: ReceiptState<SlipMeta>;
  onView: (view: PaneView) => void;
  width: number;
  onResize: (width: number) => void;
  /** Drawer mode: over the map instead of beside it. */
  drawer: boolean;
  /** The deploy button and recovery choices, above the slip. */
  entry?: ReactNode;
}

/** A new receipt starts with its own slip state (expanded errors, Copy). */
function slipKey(key: string, opened: Opened): string {
  return opened.kind === "receipt" ? `${key}\n${opened.runId}` : `${key}\n${opened.kind}`;
}

export function ReceiptPane({
  requestKey,
  focusSlip,
  onSlipFocused,
  opening: next,
  receipt,
  onView,
  width,
  onResize,
  drawer,
  entry,
}: Props) {
  const drag = useRef<{ x: number; width: number } | null>(null);
  const pane = useRef<HTMLElement>(null);
  const shownKey = receipt.shown?.key;

  useEffect(() => {
    if (!focusSlip || shownKey !== requestKey) return;
    pane.current?.querySelector<HTMLElement>(".slip-title")?.focus();
    onSlipFocused();
  }, [focusSlip, shownKey, requestKey, onSlipFocused]);

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    drag.current = { x: event.clientX, width };
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (drag.current) onResize(drag.current.width + drag.current.x - event.clientX);
  };
  const onPointerUp = () => {
    drag.current = null;
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "ArrowLeft") onResize(width + KEY_STEP);
    else if (event.key === "ArrowRight") onResize(width - KEY_STEP);
    else return;
    event.preventDefault();
  };

  const { shown, opening } = receipt;
  return (
    <aside
      ref={pane}
      className={`pane${drawer ? " pane-drawer" : ""}`}
      style={{ width }}
      aria-label="Receipt"
    >
      {!drawer && <div className="pane-drag" data-tauri-drag-region />}
      <div
        className="pane-handle"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize receipt"
        aria-valuemin={PANE_MIN}
        aria-valuemax={PANE_MAX}
        aria-valuenow={width}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onLostPointerCapture={onPointerUp}
        onKeyDown={onKeyDown}
      />
      {entry}
      <ReceiptTabs cell={next.cell} role={next.role} onView={onView} />
      {opening && (
        <p className="pane-opening muted">
          Opening receipt for {next.label} · {next.env}…
        </p>
      )}
      {shown && (
        <div
          id="receipt-panel"
          className={`pane-slip${opening ? " pane-slip-dim" : ""}`}
          aria-busy={opening}
          role={tabs(next.cell).length > 1 ? "tabpanel" : undefined}
          aria-labelledby={tabs(next.cell).length > 1 ? `receipt-tab-${next.role}` : undefined}
        >
          <Slip
            key={slipKey(shown.key, shown.opened)}
            opened={shown.opened}
            cell={shown.meta.cell}
            label={shown.meta.label}
            env={shown.meta.env}
            context={withoutLinks(context(shown.meta.cell, shown.meta.role), shown.meta.cell)}
            onView={onView}
          />
        </div>
      )}
    </aside>
  );
}

interface Tab {
  role: "last" | "code" | "stale";
  view: PaneView;
  text: string;
}

/** The cell's receipts, latest risk first. */
function tabs(cell: EnvCellView): Tab[] {
  const all: [boolean, Tab][] = [
    [cell.lastAttempt !== null, { role: "last", view: "last", text: "Latest attempt" }],
    [cell.receipt !== null && cell.mark !== "noCode", { role: "code", view: "code", text: "Recorded code" }],
    [cell.staleReceipt !== null, { role: "stale", view: "stale", text: "Unreadable newer" }],
  ];
  return all.flatMap(([ok, tab]) => (ok ? [tab] : []));
}

/** The tabs already lead to the other receipts; the slip keeps its words. */
function withoutLinks(found: Context | null, cell: EnvCellView): Context | null {
  return found && tabs(cell).length > 1 ? { ...found, links: [] } : found;
}

function ReceiptTabs({
  cell,
  role,
  onView,
}: {
  cell: EnvCellView;
  role: Target["role"];
  onView: (view: PaneView) => void;
}) {
  const list = tabs(cell);
  if (list.length < 2) return null;
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const at = list.findIndex((tab) => tab.role === role);
    const step = { ArrowRight: 1, ArrowLeft: -1 }[event.key];
    if (step === undefined) return;
    event.preventDefault();
    const next = list[(at + step + list.length) % list.length];
    onView(next.view);
    event.currentTarget.querySelector<HTMLElement>(`#receipt-tab-${next.role}`)?.focus();
  };
  return (
    <div className="tabs" role="tablist" aria-label="Receipts for this cell" onKeyDown={onKeyDown}>
      {list.map((tab) => {
        const selected = tab.role === role;
        return (
          <button
            type="button"
            role="tab"
            id={`receipt-tab-${tab.role}`}
            key={tab.role}
            className="tab"
            aria-selected={selected}
            aria-controls="receipt-panel"
            tabIndex={selected ? 0 : -1}
            onClick={() => onView(tab.view)}
          >
            {tab.text}
          </button>
        );
      })}
    </div>
  );
}
