import { useEffect, useRef, useState, type KeyboardEvent, type Ref } from "react";
import { deploy } from "./deployApi";
import { withCode } from "./text";
import type { DeployEntry, RecoveryAction } from "./types";

export interface EntryAt {
  generation: number;
  row: number;
  env: string;
}

/** Asks Rust what the deploy button offers for the selected cell. A new
 * `version` asks again, e.g. after an approval or a run ends. */
export function useDeployEntry(at: EntryAt | null, version = 0): DeployEntry | null {
  const cell = at ? `${at.generation}\n${at.row}\n${at.env}` : null;
  const key = cell === null ? null : `${cell}\n${version}`;
  const [answer, setAnswer] = useState<{ cell: string; entry: DeployEntry } | null>(null);
  const generation = at?.generation;
  const row = at?.row;
  const env = at?.env;
  useEffect(() => {
    if (cell === null || generation === undefined || row === undefined || env === undefined) return;
    let gone = false;
    deploy.entry(generation, row, env).then(
      (entry) => !gone && setAnswer({ cell, entry }),
      // A stale map is reloaded, which asks again.
      () => !gone && setAnswer({ cell, entry: { kind: "hidden" } }),
    );
    return () => {
      gone = true;
    };
  }, [key, cell, generation, row, env]);
  // While asking again, the last answer for the same cell stays up.
  return answer?.cell === cell ? answer.entry : null;
}

export function actionLabel(action: RecoveryAction): string {
  switch (action.kind) {
    case "fromStep":
      return `Run from step ${action.step}…`;
    case "attach":
      return "Attach…";
    case "up":
      return "Bring app out of maintenance…";
  }
}

/** What a recovery choice does, so it can be picked without opening it. */
export function actionConsequence(action: RecoveryAction): string {
  switch (action.kind) {
    case "fromStep":
      return `Runs step ${action.step} and the steps after it on the commit already checked out. No fast-forward.`;
    case "attach":
      return "Follows the unfinished run again. A step that was running is never started twice.";
    case "up":
      return "Runs php artisan up to end maintenance mode. No deploy steps run.";
  }
}

export type EntryChoice =
  | { kind: "review" }
  | { kind: "rerun" }
  | { kind: "fromStep"; step: number }
  | { kind: "attach" }
  | { kind: "up" };

interface MenuItem {
  label: string;
  onSelect: () => void;
}

/** A menu button: arrows move between items, Esc and Tab close it. */
function Menu({ items }: { items: MenuItem[] }) {
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    list.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const onPointer = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!list.current?.contains(target) && !button.current?.contains(target)) setOpen(false);
    };
    document.addEventListener("pointerdown", onPointer);
    return () => document.removeEventListener("pointerdown", onPointer);
  }, [open]);
  const close = () => {
    setOpen(false);
    button.current?.focus();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const entries = [...(list.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
    const at = entries.indexOf(document.activeElement as HTMLElement);
    const next = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: entries.length - 1 }[event.key];
    if (next !== undefined) {
      event.preventDefault();
      entries[(next + entries.length) % entries.length]?.focus();
    } else if (event.key === "Escape") {
      // The pane's own Escape would close the drawer.
      event.preventDefault();
      event.stopPropagation();
      close();
    } else if (event.key === "Tab") {
      setOpen(false);
    }
  };
  return (
    <div className="menu">
      <button
        type="button"
        ref={button}
        className="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? "deploy-entry-more" : undefined}
        onClick={() => setOpen(!open)}
      >
        More <span aria-hidden>▾</span>
      </button>
      {open && (
        <div id="deploy-entry-more" className="menu-list" role="menu" aria-label="More" ref={list} onKeyDown={onKeyDown}>
          {items.map((item) => (
            <button
              type="button"
              role="menuitem"
              tabIndex={-1}
              className="menu-item"
              key={item.label}
              onClick={() => {
                close();
                item.onSelect();
              }}
            >
              {item.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** The panel's actions for the selected cell: what the env's newest run
 * left to do, each choice with what it does, then the deploy itself. */
export function DeployEntryBar({
  entry,
  target,
  notice,
  onChoose,
  onOpenConfig,
  buttonRef,
}: {
  entry: DeployEntry | null;
  /** The app and env the deploy button names. */
  target: { label: string; env: string };
  /** A pencil line: an approval, or a preview a reload closed. */
  notice?: string | null;
  onChoose: (choice: EntryChoice) => void;
  /** Opens `.shipslip.toml`; rejects with a `CommandError`. */
  onOpenConfig: () => Promise<void>;
  buttonRef?: Ref<HTMLButtonElement>;
}) {
  const [configError, setConfigError] = useState<string | null>(null);
  const openConfig = () => {
    setConfigError(null);
    onOpenConfig().catch((error: unknown) =>
      setConfigError(error && typeof error === "object" && "message" in error ? String(error.message) : String(error)),
    );
  };
  if (!entry || entry.kind === "hidden") return null;
  const reason =
    entry.kind === "unavailable"
      ? entry.reason
      : entry.kind === "running"
        ? "A deploy is running for this env"
        : null;
  const ready = entry.kind === "ready" ? entry : null;
  const recovery = ready?.recovery ?? null;
  const recoverFirst = recovery !== null && recovery.actions.length > 0;
  const more: MenuItem[] = [];
  if (ready && ready.steps.length > 0) {
    more.push({ label: "Rerun…", onSelect: () => onChoose({ kind: "rerun" }) });
    more.push({
      label: "Run from step…",
      onSelect: () => {
        const from = recovery?.actions.find((a) => a.kind === "fromStep");
        onChoose({ kind: "fromStep", step: from?.kind === "fromStep" ? from.step : 1 });
      },
    });
  }
  if (ready) more.push({ label: "Open config", onSelect: openConfig });
  return (
    <div className="deploy-entry">
      {notice && <p className="deploy-entry-notice muted">{notice}</p>}
      {recovery && (
        <section className="recovery" aria-labelledby="recovery-title">
          <p id="recovery-title" className="caps muted">
            What the last run left
          </p>
          <p className="recovery-sentence">{withCode(recovery.sentence)}</p>
          {recovery.advice && <p className="muted">{withCode(recovery.advice)}</p>}
          {recovery.actions.length > 0 && (
            <ul className="recovery-actions">
              {recovery.actions.map((action, i) => (
                <li className="recovery-action" key={action.kind}>
                  <button
                    type="button"
                    className={`button${i === 0 ? " button-primary" : ""}`}
                    aria-describedby={`recovery-does-${action.kind}`}
                    onClick={() =>
                      onChoose(
                        action.kind === "fromStep" ? { kind: "fromStep", step: action.step } : { kind: action.kind },
                      )
                    }
                  >
                    {actionLabel(action)}
                  </button>
                  <p id={`recovery-does-${action.kind}`} className="muted recovery-does">
                    {actionConsequence(action)}
                  </p>
                </li>
              ))}
            </ul>
          )}
          {recovery.actions.length > 0 && (
            <p className="muted recovery-note">Each choice opens a review. Nothing runs until you confirm.</p>
          )}
        </section>
      )}
      <div className="deploy-entry-row">
        <button
          type="button"
          ref={buttonRef}
          className={`button${recoverFirst || reason !== null ? "" : " button-primary"}`}
          disabled={reason !== null}
          aria-describedby={reason ? "deploy-entry-reason" : undefined}
          onClick={() => onChoose({ kind: "review" })}
        >
          Deploy {target.label} → {target.env}…
        </button>
        {more.length > 0 && <Menu items={more} />}
      </div>
      {configError && (
        <p className="deploy-entry-reason muted" role="status">
          {configError}
        </p>
      )}
      {reason && (
        <p id="deploy-entry-reason" className="deploy-entry-reason muted">
          {reason}
        </p>
      )}
    </div>
  );
}
