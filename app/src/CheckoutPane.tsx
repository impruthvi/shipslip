import { useEffect, useRef, useState, type ReactNode, type Ref, type RefObject } from "react";
import { api } from "./api";
import { Cell } from "./Cell";
import { CheckConnectionDialog } from "./ConnectionCheck";
import { deploy } from "./deployApi";
import { Menu } from "./Menu";
import { outcomeClass } from "./Slip";
import type { DeployEntry, EnvCellView, MapView, ReadyEntry, ReceiptRowView } from "./types";

/** Asks what Deploy offers for each of the checkout's environments. The last
 * answers stay up while a newer map is asked again. */
function useEntries(generation: number, row: number, repoRoot: string, envs: string[], version: number) {
  const [answers, setAnswers] = useState<{ repoRoot: string; entries: Record<string, DeployEntry> }>({
    repoRoot,
    entries: {},
  });
  const key = envs.join("\n");
  useEffect(() => {
    let gone = false;
    for (const env of key.split("\n").filter(Boolean)) {
      deploy.entry(generation, row, env).then(
        (entry) =>
          !gone &&
          setAnswers((prev) => ({
            repoRoot,
            entries: { ...(prev.repoRoot === repoRoot ? prev.entries : {}), [env]: entry },
          })),
        // A stale map is reloaded, which asks again.
        () => {},
      );
    }
    return () => {
      gone = true;
    };
  }, [generation, row, repoRoot, key, version]);
  return answers.repoRoot === repoRoot ? answers.entries : {};
}

/** The checkout's receipts, newest first; null until the first answer. */
function useReceipts(generation: number, row: number, repoRoot: string) {
  const [answer, setAnswer] = useState<{ repoRoot: string; receipts: ReceiptRowView[]; older: boolean } | null>(
    null,
  );
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => {
    let gone = false;
    api.checkoutReceipts(generation, row).then(
      (result) => {
        if (gone) return;
        if (result.kind === "unreadable") setFailed(result.message);
        else if (result.kind === "listed") {
          setFailed(null);
          setAnswer({ repoRoot, receipts: result.receipts, older: result.older });
        }
        // Stale or gone: the map is read again, and that asks again.
      },
      (error: unknown) => !gone && setFailed(String(error)),
    );
    return () => {
      gone = true;
    };
  }, [generation, row, repoRoot]);
  const current = answer?.repoRoot === repoRoot ? answer : null;
  return { receipts: current?.receipts ?? null, older: current?.older ?? false, failed };
}

function message(error: unknown): string {
  return error && typeof error === "object" && "message" in error ? String(error.message) : String(error);
}

function EnvColumn({
  env,
  cell,
  entry,
  live,
  approved,
  onDeploy,
  buttonRef,
}: {
  env: string;
  cell: EnvCellView;
  entry: DeployEntry | undefined;
  live: string | undefined;
  /** Approved, and the checkout's config still matches its approvals. */
  approved: boolean;
  onDeploy: (entry: ReadyEntry) => void;
  buttonRef?: Ref<HTMLButtonElement>;
}) {
  const running = Boolean(live || cell.terminalRun) || entry?.kind === "running";
  const never = cell.receipt === null && cell.lastAttempt === null && !running;
  const reason = entry?.kind === "unavailable" ? entry.reason : null;
  const reasonId = `checkout-deploy-reason-${env}`;
  return (
    <section className="checkout-env" aria-labelledby={`checkout-env-${env}`}>
      <h2 id={`checkout-env-${env}`} className="checkout-env-name">
        {env}
      </h2>
      <Cell cell={cell} live={live} expanded={!never} />
      {never && (
        <p className="checkout-never muted">
          {approved ? "Approved. Nothing deployed from this Mac yet." : "Nothing deployed from this Mac yet."}
        </p>
      )}
      {cell.target && <p className="checkout-target mono muted">{cell.target}</p>}
      {entry?.kind !== "hidden" && (
        <button
          type="button"
          className="button checkout-deploy"
          ref={buttonRef}
          disabled={running || entry?.kind !== "ready"}
          aria-describedby={reason ? reasonId : undefined}
          onClick={() => entry?.kind === "ready" && onDeploy(entry)}
        >
          {running ? "Running…" : `Deploy to ${env}…`}
        </button>
      )}
      {reason && (
        <p id={reasonId} className="checkout-deploy-reason muted">
          {reason}
        </p>
      )}
    </section>
  );
}

interface Props {
  generation: number;
  map: MapView;
  row: number;
  /** Outcome lines of cells with a run going now, by `repoRoot\nenv`. */
  live: Record<string, string>;
  /** Shown after the name when another checkout has the same name. */
  folder: string | null;
  /** Asks for the Deploy entries again, e.g. after a run ends. */
  entryVersion: number;
  /** The receipt the pane shows, to mark its line. */
  openedReceipt: number | null;
  onDeploy: (env: string, entry: ReadyEntry) => void;
  onOpenReceipt: (env: string, id: number) => void;
  heading: RefObject<HTMLHeadingElement | null>;
  /** Back from the deploy flow: this environment's Deploy takes focus once
   * its entry is known, or the heading when Deploy is off. */
  focusDeploy: string | null;
  onDeployFocused: () => void;
  /** The checkout left the projects list. */
  onRemoved: () => void;
  /** More toolbar buttons, e.g. the receipt drawer's. */
  tools?: ReactNode;
}

/** One checkout: its environments side by side as the map draws them, a
 * Deploy under each, and its receipts below. */
export function CheckoutPane({
  generation,
  map,
  row: rowIndex,
  live,
  folder,
  entryVersion,
  openedReceipt,
  onDeploy,
  onOpenReceipt,
  heading,
  focusDeploy,
  onDeployFocused,
  onRemoved,
  tools,
}: Props) {
  const row = map.rows[rowIndex];
  const gone = row.pathMissing;
  const unreadable = gone ? null : row.configUnreadable;
  // Deploy can't work from a gone folder or a config that can't be read.
  const canDeploy = !gone && !unreadable;
  const columns = row.cells.flatMap((cell, col) =>
    cell.kind === "present" ? [{ env: map.envs[col], cell }] : [],
  );
  const entries = useEntries(
    generation,
    rowIndex,
    row.repoRoot,
    canDeploy ? columns.map((c) => c.env) : [],
    entryVersion,
  );
  const { receipts, older, failed } = useReceipts(generation, rowIndex, row.repoRoot);
  const buttons = useRef<Record<string, HTMLButtonElement | null>>({});
  const focused = useRef(onDeployFocused);
  useEffect(() => {
    focused.current = onDeployFocused;
  });
  const focusEntry = focusDeploy === null ? undefined : entries[focusDeploy];
  useEffect(() => {
    if (focusDeploy === null || focusEntry === undefined) return;
    const button = buttons.current[focusDeploy];
    if (button && !button.disabled) button.focus();
    else heading.current?.focus();
    focused.current();
  }, [focusDeploy, focusEntry, heading]);
  const [menuError, setMenuError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const run = (action: () => Promise<void>) => {
    setMenuError(null);
    action().catch((error: unknown) => setMenuError(message(error)));
  };
  const menu = [
    ...(canDeploy && columns.length > 0 ? [{ label: "Check connection", onSelect: () => setChecking(true) }] : []),
    ...(gone
      ? []
      : [{ label: "Open folder in Finder", onSelect: () => run(() => deploy.openFolder(generation, rowIndex)) }]),
    ...(!gone && (unreadable || columns.length > 0)
      ? [
          {
            label: "Open .shipslip.toml",
            // The config is the checkout's; any of its environments finds it,
            // and the checkout alone does when it can't be read.
            onSelect: () =>
              run(() => deploy.openConfig(generation, rowIndex, unreadable ? null : columns[0].env)),
          },
        ]
      : []),
    ...(row.removable
      ? [
          {
            label: "Remove from sidebar",
            onSelect: () => run(() => api.removeCheckout(generation, rowIndex).then(onRemoved)),
          },
        ]
      : []),
  ];
  return (
    <div className="checkout">
      <header className="toolbar" data-tauri-drag-region>
        <h1 className="toolbar-title" ref={heading} tabIndex={-1} data-tauri-drag-region>
          {row.label}
        </h1>
        <p className="toolbar-subtitle muted mono" title={row.repoRoot} data-tauri-drag-region>
          {folder ? `in ${folder} · ` : ""}
          {row.repoRoot}
        </p>
        {gone ? (
          <span className="checkout-state muted">checkout not found</span>
        ) : unreadable ? (
          <span className="checkout-state tone-attention">.shipslip.toml can't be read</span>
        ) : null}
        {tools}
        {menu.length > 0 && (
          <Menu label={`${row.label} actions`} end items={menu}>
            <span aria-hidden>⋯</span>
          </Menu>
        )}
      </header>
      {checking && (
        <CheckConnectionDialog
          envs={columns.map((c) => c.env)}
          start={(env) => (onLines) => api.checkConnection(generation, rowIndex, env, onLines)}
          onClose={() => {
            setChecking(false);
            heading.current?.focus();
          }}
        />
      )}
      {menuError && (
        <p className="checkout-menu-error muted" role="status">
          {menuError}
        </p>
      )}
      <div className="checkout-body">
        {unreadable && (
          <section className="checkout-config-error" aria-label="Why .shipslip.toml can't be read">
            <p>Shipslip can't read this checkout's .shipslip.toml. Edit it to deploy again.</p>
            <pre className="sheet-detail mono">{unreadable.join("\n")}</pre>
          </section>
        )}
        <div className="checkout-envs">
          {columns.map(({ env, cell }) => (
            <EnvColumn
              key={env}
              env={env}
              cell={cell}
              entry={canDeploy ? entries[env] : { kind: "hidden" }}
              live={live[`${row.repoRoot}\n${env}`]}
              approved={cell.approved && !row.needsApproval && !unreadable}
              onDeploy={(entry) => onDeploy(env, entry)}
              buttonRef={(button) => {
                buttons.current[env] = button;
              }}
            />
          ))}
        </div>
        <section className="checkout-receipts" aria-labelledby="checkout-receipts-title">
          <h2 id="checkout-receipts-title" className="caps muted">
            Receipts
          </h2>
          {failed ? (
            <p className="muted">Couldn't list this checkout's receipts: {failed}</p>
          ) : receipts === null ? null : receipts.length === 0 ? (
            <p className="muted">No receipts for this checkout on this Mac yet.</p>
          ) : (
            <ul className="receipt-rows">
              {receipts.map((receipt) => (
                <li key={receipt.receipt}>
                  <button
                    type="button"
                    className="receipt-row"
                    aria-current={receipt.receipt === openedReceipt ? "true" : undefined}
                    onClick={() => onOpenReceipt(receipt.env, receipt.receipt)}
                  >
                    <span className="muted">{receipt.age}</span>
                    <span>{receipt.env}</span>
                    <span className={`receipt-row-outcome ${outcomeClass(receipt.outcome)}`}>
                      {receipt.outcome.text}
                    </span>
                    <span className="mono muted" title={receipt.sha}>
                      {receipt.shortSha}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          {!failed && older && <p className="muted">Older receipts not shown</p>}
        </section>
      </div>
    </div>
  );
}
