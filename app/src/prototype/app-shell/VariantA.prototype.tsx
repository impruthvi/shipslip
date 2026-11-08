// PROTOTYPE A · "Ledger": dense one-line sidebar rows with a mark per Environment,
// the checkout as a one-row map with receipts below, and the wizard as one
// screen whose steps fold up as they are done. All projects is the start screen.

import { useState } from "react";
import { Cell, Flap } from "../../Cell";
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
  type WizardStep,
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

export function VariantA() {
  const shell = useShell((list) => (list.length ? { kind: "all" } : { kind: "all" }));
  const { list, view, setView } = shell;
  const selected = view.kind === "checkout" ? list.find((c) => c.id === view.id) : undefined;
  const open = (id: string) => {
    const c = list.find((x) => x.id === id);
    if (c && c.state !== "ready") setView({ kind: "wizard", resume: id });
    else setView({ kind: "checkout", id });
  };
  return (
    <div className="window">
      <nav className="sidebar pa-sidebar" aria-label="Sidebar">
        <div className="titlebar-drag" />
        <button type="button" className="button pa-add" onClick={() => setView({ kind: "wizard" })}>
          + Add project
        </button>
        <TodaySections list={list} onOpen={open} />
        {list.length > 0 && <ProblemsStrip problems={fullMap.problems} />}
        <section className="side-section">
          <h2 className="side-title">Projects</h2>
          <ul className="side-list">
            <li>
              <button
                type="button"
                className={`side-item pa-row${view.kind === "all" ? " is-on" : ""}`}
                onClick={() => setView({ kind: "all" })}
              >
                <span className="side-line">
                  <span className="side-name">All projects</span>
                  <span className="muted ps-small">{list.length}</span>
                </span>
              </button>
            </li>
            {list.map((c) => (
              <li key={c.id}>
                <button
                  type="button"
                  className={`side-item pa-row${selected?.id === c.id || (view.kind === "wizard" && view.resume === c.id) ? " is-on" : ""}`}
                  onClick={() => open(c.id)}
                  title={c.path}
                >
                  <span className="side-line">
                    <span className="side-name">
                      {c.project}
                      {c.folder && <span className="side-folder muted"> · {c.folder}</span>}
                    </span>
                    {c.state === "ready" ? (
                      <EnvMarks c={c} />
                    ) : (
                      <span className={`pa-state ${c.state === "needsApproval" ? "tone-attention" : "muted"}`}>
                        {c.state === "needsApproval" ? "Needs approval" : "Setup not finished"}
                      </span>
                    )}
                  </span>
                </button>
              </li>
            ))}
          </ul>
          {list.length === 0 && <p className="side-empty muted">No checkouts yet</p>}
        </section>
      </nav>
      <main className="main">
        {view.kind === "all" && <AllProjects list={list} onOpen={open} onAdd={() => setView({ kind: "wizard" })} />}
        {view.kind === "checkout" && selected && <CheckoutA c={selected} onRemove={() => shell.remove(selected.id)} />}
        {view.kind === "wizard" && (
          <WizardA key={view.resume ?? "new"} resume={view.resume} onFinish={shell.add} onCancel={() => setView({ kind: "all" })} />
        )}
      </main>
    </div>
  );
}

export function AllProjects({ list, onOpen, onAdd }: { list: Checkout[]; onOpen: (id: string) => void; onAdd: () => void }) {
  return (
    <>
      <header className="toolbar">
        <h1 className="toolbar-title">All projects</h1>
        <p className="toolbar-subtitle muted">What this Mac recorded for each checkout and Environment</p>
      </header>
      {list.length === 0 ? (
        <section className="pa-empty">
          <p className="pa-empty-title">No projects on this Mac yet.</p>
          <p className="muted">
            Add the folder of a Laravel app. Shipslip reads its .shipslip.toml, or asks seven questions to write one,
            then checks it can reach the Environment over your own ssh.
          </p>
          <button type="button" className="button button-primary" onClick={onAdd}>
            Add project
          </button>
          <p className="muted ps-small">Or from a terminal: cd your-app && slip init</p>
        </section>
      ) : (
        <div className="map-area">
          <ReleaseMap map={fullMap} onSelect={(row) => onOpen(fullMap.rows[row].repoRoot)} />
        </div>
      )}
    </>
  );
}

export function CheckoutA({ c, onRemove }: { c: Checkout; onRemove: () => void }) {
  const [checking, setChecking] = useState(false);
  const receipts = receiptsFor(c);
  return (
    <>
      <header className="toolbar">
        <h1 className="toolbar-title">{c.project}</h1>
        <p className="toolbar-subtitle muted mono">{c.path.replace("/Users/you", "~")}</p>
        <CheckoutMenu c={c} onCheck={() => setChecking(true)} onRemove={onRemove} />
      </header>
      {checking && <MenuCheck c={c} onClose={() => setChecking(false)} />}
      <div className="pa-onerow">
        {c.envs.map((e) => (
          <div key={e.env} className="pa-envcol">
            <div className="pa-envhead">
              <span className="grid-env-name">{e.env}</span>
              {e.production && e.env !== "production" && <span className="production-chip">production</span>}
            </div>
            {e.cell ? (
              <Cell cell={e.cell} expanded live={c.path === runningNow.checkout && e.env === runningNow.env ? runningNow.text : undefined} />
            ) : (
              <p className="muted ps-small">Approved. Nothing deployed from this Mac yet.</p>
            )}
            {e.cell?.target && <p className="muted ps-small mono">{e.cell.target}</p>}
            <button type="button" className="button pa-deploy" disabled={e.mark === "live"}>
              {e.mark === "live" ? "Running…" : `Deploy to ${e.env}`}
            </button>
          </div>
        ))}
      </div>
      <section className="pa-receipts">
        <h2 className="side-title">Receipts</h2>
        {receipts.length === 0 ? (
          <p className="muted ps-small">No receipts recorded for this checkout.</p>
        ) : (
          <table className="pa-table">
            <tbody>
              {receipts.map((r) => (
                <tr key={r.id}>
                  <td className="muted">{r.age}</td>
                  <td>{r.env}</td>
                  <td className={r.outcome.kind === "failed" ? "tone-bad" : r.outcome.tone === "bad" ? "tone-attention" : ""}>
                    {r.outcome.text}
                  </td>
                  <td className="mono muted">{r.sha ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </>
  );
}

function summary(w: Wizard, s: WizardStep): string {
  switch (s) {
    case "folder":
      return w.folder ? w.folder.path.replace("/Users/you", "~") : "";
    case "answers":
      return `${w.answers.env} · ${w.answers.path} · ${w.answers.branch}${w.answers.production ? " · production" : ""}`;
    case "ssh":
      return w.answers.alias;
    case "check":
      return w.check.passed ? "All passed" : w.check.done ? "Done, with lines to read" : "";
    default:
      return "";
  }
}

function WizardA({ resume, onFinish, onCancel }: { resume?: string; onFinish: (c: Checkout) => void; onCancel: () => void }) {
  const w = useWizard(onFinish, resume);
  return (
    <>
      <header className="toolbar">
        <h1 className="toolbar-title">Add project</h1>
        <p className="toolbar-subtitle muted">Reads your files and ~/.ssh/config. Writes only .shipslip.toml, after approval.</p>
        <button type="button" className="toolbar-button" onClick={onCancel}>
          Cancel
        </button>
      </header>
      <ol className="pa-steps">
        {w.order.map((s, i) => {
          const state = i < w.index ? "done" : i === w.index ? "current" : "later";
          return (
            <li key={s} className={`pa-step is-${state}`}>
              <div className="pa-step-head">
                <span className="pa-step-num mono">{i + 1}</span>
                <span className="pa-step-title">{stepTitles[s]}</span>
                {state === "done" && (
                  <>
                    <span className="muted ps-small pa-step-sum">{summary(w, s)}</span>
                    <button type="button" className="link" onClick={() => w.setStep(s)}>
                      Change
                    </button>
                  </>
                )}
              </div>
              {state === "current" && (
                <div className="pa-step-body">
                  <StepBodyA w={w} />
                  <div className="ps-row">
                    {w.step === "approval" ? (
                      <button type="button" className="button button-primary" disabled={!w.canNext.approval} onClick={w.finish}>
                        Approve and add
                      </button>
                    ) : (
                      <button type="button" className="button button-primary" disabled={!w.canNext[w.step]} onClick={w.next}>
                        Continue
                      </button>
                    )}
                  </div>
                </div>
              )}
            </li>
          );
        })}
      </ol>
    </>
  );
}

function StepBodyA({ w }: { w: Wizard }) {
  switch (w.step) {
    case "folder":
      return <FolderPicker w={w} />;
    case "answers":
      return <AnswersForm w={w} />;
    case "ssh":
      return (
        <div className="pa-ssh">
          <AliasPicker w={w} />
          {w.aliasMode === "new" && (
            <div className="pa-hostgrid">
              <HostFields w={w} />
              <div>
                <p className="muted ps-small">Add this to ~/.ssh/config yourself, then check again.</p>
                <HostBlockBox w={w} />
              </div>
            </div>
          )}
        </div>
      );
    case "check":
      return <CheckA w={w} />;
    case "approval":
      return <ApprovalA w={w} />;
  }
}

export function CheckA({ w }: { w: Wizard }) {
  const { lines, run, done } = w.check;
  return (
    <div className="ps-stack">
      <p className="muted ps-p">Read only. Connects as {w.alias} and looks; changes nothing on the Environment.</p>
      {!lines ? (
        <div>
          <button type="button" className="button" onClick={run}>
            Check connection
          </button>
        </div>
      ) : (
        <ul className="ps-checklist">
          {lines.map((l) => (
            <li key={l.label} className={`ps-checkline is-${l.state}`}>
              <CheckIcon line={l} />
              <span>{l.label}</span>
              <span className={`ps-small ${l.state === "fail" ? "tone-attention" : "muted"}`}>
                {l.state === "waiting" || l.state === "running" ? "" : l.reason}
                {l.state === "fail" && l.command && (
                  <>
                    {" "}
                    <CommandLine command={l.command} />
                  </>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
      {done && (
        <div>
          <button type="button" className="button" onClick={run}>
            Check again
          </button>
        </div>
      )}
    </div>
  );
}

function ApprovalA({ w }: { w: Wizard }) {
  return (
    <div className="pa-approval">
      <div className="ps-stack">
        <Warnings w={w} />
        <pre className="ps-pre ps-toml">{w.existing ? "# .shipslip.toml as found in the folder (read only)\n" + w.toml : w.toml}</pre>
      </div>
      <aside className="pa-approval-side">
        <h3 className="side-title">Connection check</h3>
        <ul className="ps-checklist ps-compact">
          {w.check.lines?.map((l) => (
            <li key={l.label} className={`ps-checkline is-${l.state}`}>
              <CheckIcon line={l} />
              <span>{l.label}</span>
            </li>
          )) ?? <li className="muted ps-small">Not run</li>}
        </ul>
        <p className="ps-small muted">
          Approving records this config's hash on this Mac. Deploys refuse a config that changed since.
        </p>
        {w.check.lines?.some((l) => l.state === "fail") && (
          <p className="ps-small">
            <Flap status={{ word: "Check", tone: "attention" }} /> One line did not pass. You can still approve.
          </p>
        )}
      </aside>
    </div>
  );
}
