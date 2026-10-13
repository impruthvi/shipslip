import { useRef, type KeyboardEvent, type PointerEvent } from "react";
import { PANE_MAX, PANE_MIN } from "./pane";
import { context, type PaneView, type Target } from "./slipTarget";
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
  /** The cell being opened, named while a slow read is under way. */
  opening: { label: string; env: string };
  receipt: ReceiptState<SlipMeta>;
  onView: (view: PaneView) => void;
  width: number;
  onResize: (width: number) => void;
  /** Drawer mode: over the map instead of beside it. */
  drawer: boolean;
}

/** A new receipt starts with its own slip state (expanded errors, Copy). */
function slipKey(key: string, opened: Opened): string {
  return opened.kind === "receipt" ? `${key}\n${opened.runId}` : `${key}\n${opened.kind}`;
}

export function ReceiptPane({ opening: next, receipt, onView, width, onResize, drawer }: Props) {
  const drag = useRef<{ x: number; width: number } | null>(null);

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
      className={`pane${drawer ? " pane-drawer" : ""}`}
      style={{ width }}
      aria-label="Receipt"
    >
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
      {opening && (
        <p className="pane-opening muted">
          Opening receipt for {next.label} · {next.env}…
        </p>
      )}
      {shown && (
        <div className={`pane-slip${opening ? " pane-slip-dim" : ""}`}>
          <Slip
            key={slipKey(shown.key, shown.opened)}
            opened={shown.opened}
            cell={shown.meta.cell}
            label={shown.meta.label}
            env={shown.meta.env}
            context={context(shown.meta.cell, shown.meta.role)}
            onBackToCode={() => onView("code")}
          />
        </div>
      )}
    </aside>
  );
}
