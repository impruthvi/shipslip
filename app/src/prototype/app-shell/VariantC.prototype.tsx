// PROTOTYPE C · "Master–detail + sheet": All projects is the first sidebar item
// and the start screen; checkouts are single dense lines with marks before the
// name; a checkout opens as an Environment list beside one Environment's detail
// (Deploy in its header); the wizard is a sheet over the app with a step rail.

import { useState, type ReactNode } from "react";
import { Cell, Flap } from "../../Cell";
import { Mark, StepIcon } from "../../Mark";
import { ProblemsStrip } from "../../ProblemsStrip";
import { ReleaseMap } from "../../ReleaseMap";
import {
  fullMap,
  receiptsFor,
  runningNow,
  stepTitles,
  useShell,
  useWizard,
  type Checkout,
  type Wizard,
} from "./data.prototype";
import {
  AliasPicker,
  AnswersForm,
  CheckIcon,
  CheckoutMenu,
  CommandLine,
  EnvMarks,
  FolderPicker,
  HostBlockBox,
  HostFields,
  MenuCheck,
  TodaySections,
  Warnings,
} from "./parts.prototype";

export function VariantC() {
  const shell = useShell(() => ({ kind: "all" }));
  const { list, view, setView } = shell;
  const [sheet, setSheet] = useState<{ resume?: string } | null>(null);
  const selected = view.kind === "checkout" ? list.find((c) => c.id === view.id) : undefined;
  const open = (id: string) => {
    const c = list.find((x) => x.id === id);
    if (c && c.state !== "ready") setSheet({ resume: id });
    else setView({ kind: "checkout", id });
  };
  return (
    <div className="window">
      <nav className="sidebar pc-sidebar" aria-label="Sidebar">
        <div className="titlebar-drag" />
        <button
          type="button"
          className={`side-item pc-line pc-all${view.kind === "all" ? " is-on" : ""}`}
          onClick={() => setView({ kind: "all" })}
        >
          <span className="side-line">
            <span className="pc-icon" aria-hidden>▦</span>
            <span className="side-name">All projects</span>
          </span>
        </button>
        <TodaySections list={list} onOpen={open} />
        {list.length > 0 && <ProblemsStrip problems={fullMap.problems} />}
        <section className="side-section">
          <h2 className="side-title pc-title">
            <span>Projects</span>
            <button type="button" className="pc-plus" aria-label="Add project" title="Add project" onClick={() => setSheet({})}>
              +
            </button>
          </h2>
          <ul className="side-list">
            {list.map((c) => (
              <li key={c.id}>
                <button
                  type="button"
                  className={`side-item pc-line${selected?.id === c.id ? " is-on" : ""}`}
                  onClick={() => open(c.id)}
                  title={c.path}
                >
                  <span className="side-line">
                    {c.state === "ready" ? (
                      <EnvMarks c={c} />
                    ) : (
                      <span className="ps-marks">
                        <span className={`pc-pending ${c.state === "needsApproval" ? "tone-attention" : "muted"}`}>
                          {c.state === "needsApproval" ? "!" : "…"}
                        </span>
                      </span>
                    )}
                    <span className="side-name">
                      {c.project}
                      {c.folder && <span className="muted"> · {c.folder}</span>}
                    </span>
                  </span>
                  {c.state !== "ready" && (
                    <span className={`pc-sub ${c.state === "needsApproval" ? "tone-attention" : "muted"}`}>
                      {c.state === "needsApproval" ? "Needs approval" : "Setup not finished"}
                    </span>
                  )}
                </button>
              </li>
            ))}
            <li>
              <button type="button" className="side-item pc-line pc-addline muted" onClick={() => setSheet({})}>
                + Add project
              </button>
            </li>
          </ul>
        </section>
      </nav>
      <main className="main">
        {view.kind === "all" && (
          <>
            <header className="toolbar">
              <h1 className="toolbar-title">All projects</h1>
              <p className="toolbar-subtitle muted">What this Mac recorded for each checkout and Environment</p>
            </header>
            {list.length === 0 ? (
              <section className="first-run">
                <p className="first-run-title">No projects on this Mac yet.</p>
                <p>
                  <button type="button" className="button button-primary" onClick={() => setSheet({})}>
                    Add project
                  </button>
                </p>
                <p className="muted">Or in a terminal: cd your-laravel-app && slip init, then slip trust staging.</p>
              </section>
            ) : (
              <div className="map-area">
                <ReleaseMap map={fullMap} onSelect={(row) => open(fullMap.rows[row].repoRoot)} />
              </div>
            )}
          </>
        )}
        {view.kind === "checkout" && selected && <CheckoutC key={selected.id} c={selected} onRemove={() => shell.remove(selected.id)} />}
      </main>
      {sheet && (
        <div className="pc-scrim" role="presentation">
          <WizardC
            key={sheet.resume ?? "new"}
            resume={sheet.resume}
            onFinish={(c) => {
              shell.add(c);
              setSheet(null);
            }}
            onCancel={() => setSheet(null)}
          />
        </div>
      )}
    </div>
  );
}

function CheckoutC({ c, onRemove }: { c: Checkout; onRemove: () => void }) {
  const [env, setEnv] = useState(c.envs[0]?.env);
  const [checking, setChecking] = useState(false);
  const e = c.envs.find((x) => x.env === env) ?? c.envs[0];
  const receipts = receiptsFor(c).filter((r) => r.env === e?.env);
  return (
    <>
      <header className="toolbar">
        <h1 className="toolbar-title">{c.project}</h1>
        <p className="toolbar-subtitle muted mono">{c.path.replace("/Users/you", "~")}</p>
        <CheckoutMenu c={c} onCheck={() => setChecking(true)} onRemove={onRemove} />
      </header>
      {checking && <MenuCheck c={c} onClose={() => setChecking(false)} />}
      <div className="pc-split">
        <ul className="pc-envlist" role="listbox" aria-label="Environments">
          {c.envs.map((x) => (
            <li key={x.env}>
              <button
                type="button"
                role="option"
                aria-selected={x.env === e?.env}
                className={`pc-envrow${x.env === e?.env ? " is-on" : ""}`}
                onClick={() => setEnv(x.env)}
              >
                <Mark kind={x.mark} />
                <span className="pc-envname">{x.env}</span>
                <Flap status={x.status} />
              </button>
            </li>
          ))}
        </ul>
        {e && (
          <section className="pc-detail">
            <header className="pc-detail-head">
              <h2 className="pc-detail-title">{e.env}</h2>
              {e.production && <span className="production-chip">production</span>}
              <span className="pb-spacer" />
              <button type="button" className="button">Watch log</button>
              <button type="button" className="button button-primary" disabled={e.mark === "live"}>
                {e.mark === "live" ? "Running…" : `Deploy to ${e.env}`}
              </button>
            </header>
            {e.cell ? (
              <Cell cell={e.cell} expanded live={c.path === runningNow.checkout && e.env === runningNow.env ? runningNow.text : undefined} />
            ) : (
              <p className="muted">Approved. Nothing deployed from this Mac yet.</p>
            )}
            {e.cell?.target && <p className="muted ps-small mono">Target {e.cell.target}</p>}
            <h3 className="side-title pc-receipts-title">Receipts · {e.env}</h3>
            <ol className="pc-timeline">
              {receipts.length === 0 && <li className="muted ps-small">None recorded</li>}
              {receipts.map((r) => (
                <li key={r.id}>
                  <span className="pc-dot" aria-hidden />
                  <span className={r.outcome.kind === "failed" ? "tone-bad" : r.outcome.tone === "bad" ? "tone-attention" : ""}>
                    {r.outcome.text}
                  </span>
                  <span className="muted ps-small">{r.age}</span>
                  <span className="mono muted ps-small">{r.sha ?? ""}</span>
                </li>
              ))}
            </ol>
          </section>
        )}
      </div>
    </>
  );
}

export function WizardC({
  resume,
  onFinish,
  onCancel,
  check = (w) => <CheckC w={w} />,
}: {
  resume?: string;
  onFinish: (c: Checkout) => void;
  onCancel: () => void;
  check?: (w: Wizard) => ReactNode;
}) {
  const w = useWizard(onFinish, resume);
  return (
    <div className="pc-sheet" role="dialog" aria-modal="true" aria-label="Add project">
      <aside className="pc-rail">
        <h1 className="pc-rail-title">Add project</h1>
        {w.folder && <p className="mono muted ps-small">{w.folder.path.replace("/Users/you", "~")}</p>}
        <ol className="pc-rail-steps">
          {w.order.map((s, i) => (
            <li key={s} className={i < w.index ? "is-done" : i === w.index ? "is-current" : ""}>
              <button type="button" disabled={i > w.index} onClick={() => w.setStep(s)}>
                <span className="pc-rail-mark">{i < w.index ? <StepIcon kind="ok" label="done" /> : i + 1}</span>
                {stepTitles[s]}
              </button>
            </li>
          ))}
        </ol>
      </aside>
      <section className="pc-sheet-body">
        <div className="pc-sheet-scroll">
          {w.step === "folder" && <FolderPicker w={w} />}
          {w.step === "answers" && <AnswersForm w={w} />}
          {w.step === "ssh" && (
            <>
              <AliasPicker w={w} />
              {w.aliasMode === "new" && (
                <div className="pc-hostgrid">
                  <HostFields w={w} />
                  <HostBlockBox w={w} />
                </div>
              )}
            </>
          )}
          {w.step === "check" && check(w)}
          {w.step === "approval" && (
            <div className="pc-approval">
              <Warnings w={w} />
              <div className="pc-approval-grid">
                <pre className="ps-pre ps-toml">{w.toml}</pre>
                <div>
                  <h3 className="side-title">Connection check</h3>
                  <ul className="ps-checklist ps-compact">
                    {w.check.lines?.map((l) => (
                      <li key={l.label} className={`ps-checkline is-${l.state}`}>
                        <CheckIcon line={l} />
                        <span>{l.label}</span>
                      </li>
                    )) ?? <li className="muted ps-small">Not run</li>}
                  </ul>
                </div>
              </div>
            </div>
          )}
        </div>
        <footer className="pc-sheet-foot">
          <button type="button" className="button" onClick={onCancel}>
            {w.index === 0 ? "Cancel" : "Finish later"}
          </button>
          <span className="pb-spacer" />
          {w.index > 0 && (
            <button type="button" className="button" onClick={w.back}>
              Back
            </button>
          )}
          {w.step === "approval" ? (
            <button type="button" className="button button-primary" disabled={!w.canNext.approval} onClick={w.finish}>
              Approve and add
            </button>
          ) : (
            <button type="button" className="button button-primary" disabled={!w.canNext[w.step]} onClick={w.next}>
              Continue
            </button>
          )}
        </footer>
      </section>
    </div>
  );
}

/** The check as a terminal-like transcript, one line per probe. */
function CheckC({ w }: { w: Wizard }) {
  const { lines, run, done } = w.check;
  return (
    <div className="ps-stack">
      <div className="ps-row ps-between">
        <p className="muted ps-p">
          Read only, as <span className="mono">{w.alias}</span>.
        </p>
        <button type="button" className="button" onClick={run} disabled={Boolean(lines) && !done}>
          {lines ? "Check again" : "Check connection"}
        </button>
      </div>
      {lines && (
        <ol className="pc-transcript mono">
          {lines.map((l) => (
            <li key={l.label} className={`is-${l.state}`}>
              <CheckIcon line={l} />
              <span className="pc-t-label">{l.label}</span>
              <span className={`pc-t-reason ${l.state === "fail" ? "tone-attention" : "muted"}`}>
                {l.state === "waiting" || l.state === "running" ? "" : l.reason}
              </span>
              {l.state === "fail" && l.command && (
                <span className="pc-t-cmd">
                  <CommandLine command={l.command} />
                </span>
              )}
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}
