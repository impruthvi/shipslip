// PROTOTYPE B · "Cards": sidebar grouped by project with a status word per
// Environment, the checkout as one card per Environment (its receipts and
// Deploy inside the card), and the wizard as a sequence of full screens.
// The start screen is the first checkout; All projects sits under the list.

import { useState } from "react";
import { Flap } from "../../Cell";
import { Mark } from "../../Mark";
import { ProblemsStrip } from "../../ProblemsStrip";
import { ReleaseMap } from "../../ReleaseMap";
import {
  fullMap,
  receiptsFor,
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
  FolderPicker,
  HostBlockBox,
  HostFields,
  MenuCheck,
  TodaySections,
  Warnings,
} from "./parts.prototype";

export function VariantB() {
  const shell = useShell((list) => (list.length ? { kind: "checkout", id: list[0].id } : { kind: "all" }));
  const { list, view, setView } = shell;
  const selected = view.kind === "checkout" ? list.find((c) => c.id === view.id) : undefined;
  const open = (id: string) => {
    const c = list.find((x) => x.id === id);
    if (c && c.state !== "ready") setView({ kind: "wizard", resume: id });
    else setView({ kind: "checkout", id });
  };
  const projects = [...new Set(list.map((c) => c.project))];
  if (view.kind === "wizard") {
    return <WizardB key={view.resume ?? "new"} resume={view.resume} onFinish={shell.add} onCancel={() => setView(list.length ? { kind: "checkout", id: list[0].id } : { kind: "all" })} />;
  }
  return (
    <div className="window">
      <nav className="sidebar pb-sidebar" aria-label="Sidebar">
        <div className="titlebar-drag pb-titlebar">
          <button type="button" className="link pb-add" onClick={() => setView({ kind: "wizard" })}>
            + Add project
          </button>
        </div>
        <TodaySections list={list} onOpen={open} />
        {list.length > 0 && <ProblemsStrip problems={fullMap.problems} />}
        <section className="side-section">
          <h2 className="side-title">Projects</h2>
          {projects.map((p) => {
            const checkouts = list.filter((c) => c.project === p);
            return (
              <div key={p} className="pb-group">
                {checkouts.length > 1 && <div className="pb-group-name">{p}</div>}
                {checkouts.map((c) => (
                  <button
                    key={c.id}
                    type="button"
                    className={`side-item pb-item${selected?.id === c.id ? " is-on" : ""}${checkouts.length > 1 ? " pb-nested" : ""}`}
                    onClick={() => open(c.id)}
                    title={c.path}
                  >
                    <span className="side-line">
                      <span className="side-name">{checkouts.length > 1 ? c.path.replace("/Users/you/", "~/") : c.project}</span>
                    </span>
                    <span className="pb-envs">
                      {c.state === "ready" &&
                        c.envs.map((e) => (
                          <span key={e.env} className="pb-env">
                            <span className="muted">{e.env}</span> <Flap status={e.status} />
                          </span>
                        ))}
                      {c.state === "setupNotFinished" && <span className="muted">Setup not finished · Continue</span>}
                      {c.state === "needsApproval" && <span className="tone-attention">Needs approval · Review</span>}
                    </span>
                  </button>
                ))}
              </div>
            );
          })}
          {list.length === 0 && <p className="side-empty muted">Nothing added yet</p>}
        </section>
        <button
          type="button"
          className={`side-item pb-all${view.kind === "all" ? " is-on" : ""}`}
          onClick={() => setView({ kind: "all" })}
        >
          <span className="side-line">
            <span className="side-name">All projects</span>
            <span className="muted ps-small">map</span>
          </span>
        </button>
      </nav>
      <main className="main">
        {view.kind === "all" &&
          (list.length === 0 ? (
            <Welcome onAdd={() => setView({ kind: "wizard" })} />
          ) : (
            <>
              <header className="toolbar">
                <h1 className="toolbar-title">All projects</h1>
                <p className="toolbar-subtitle muted">What this Mac recorded for each checkout and Environment</p>
              </header>
              <div className="map-area">
                <ReleaseMap map={fullMap} onSelect={(row) => open(fullMap.rows[row].repoRoot)} />
              </div>
            </>
          ))}
        {view.kind === "checkout" && selected && <CheckoutB c={selected} onRemove={() => shell.remove(selected.id)} />}
      </main>
    </div>
  );
}

function Welcome({ onAdd }: { onAdd: () => void }) {
  return (
    <section className="pb-welcome">
      <h1 className="pb-welcome-title">Deploy with a receipt</h1>
      <p className="muted">Add a Laravel app's folder. Shipslip will:</p>
      <ol className="pb-welcome-list">
        <li>Read its .shipslip.toml, or ask seven questions and show the config it would write.</li>
        <li>Pick an alias from your ~/.ssh/config. It never writes ssh files.</li>
        <li>Check it can reach the Environment, read only.</li>
        <li>Ask you to approve the config before any deploy.</li>
      </ol>
      <button type="button" className="button button-primary" onClick={onAdd}>
        Add project
      </button>
    </section>
  );
}

function CheckoutB({ c, onRemove }: { c: Checkout; onRemove: () => void }) {
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
      <div className="pb-cards">
        {c.envs.map((e) => {
          const mine = receipts.filter((r) => r.env === e.env).slice(0, 3);
          return (
            <article key={e.env} className="pb-card">
              <header className="pb-card-head">
                <Mark kind={e.mark} />
                <h2 className="pb-card-title">{e.env}</h2>
                {e.production && <span className="production-chip">production</span>}
                <span className="pb-spacer" />
                <Flap status={e.status} />
              </header>
              <dl className="pb-facts">
                <dt className="muted">Code</dt>
                <dd className="mono">{e.cell?.shortSha ?? "—"} <span className="muted">{e.cell?.age ?? ""}</span></dd>
                <dt className="muted">Target</dt>
                <dd className="mono">{e.cell?.target ?? "—"}</dd>
              </dl>
              {(e.cell?.qualifiers[0] || e.cell?.targetMoved) && (
                <p className="ps-small tone-attention sentence">{e.cell?.qualifiers[0] ?? e.cell?.targetMoved}</p>
              )}
              <h3 className="side-title pb-card-sub">Receipts</h3>
              <ul className="pb-receipts">
                {mine.length === 0 && <li className="muted ps-small">None yet</li>}
                {mine.map((r) => (
                  <li key={r.id}>
                    <span className={r.outcome.kind === "failed" ? "tone-bad" : r.outcome.tone === "bad" ? "tone-attention" : ""}>
                      {r.outcome.text}
                    </span>
                    <span className="muted">{r.age}</span>
                  </li>
                ))}
              </ul>
              <footer className="pb-card-foot">
                <button type="button" className="button button-primary" disabled={e.mark === "live"}>
                  {e.mark === "live" ? "Running…" : "Deploy"}
                </button>
                <button type="button" className="button">Watch log</button>
              </footer>
            </article>
          );
        })}
        <button type="button" className="pb-card pb-card-add">
          + Add an Environment
          <span className="muted ps-small">Edit .shipslip.toml, then approve it here</span>
        </button>
      </div>
    </>
  );
}

function WizardB({ resume, onFinish, onCancel }: { resume?: string; onFinish: (c: Checkout) => void; onCancel: () => void }) {
  const w = useWizard(onFinish, resume);
  const titles: Record<string, string> = {
    folder: "Which folder?",
    answers: "Describe the first Environment",
    ssh: "How does ssh reach it?",
    check: "Can this Mac reach it?",
    approval: "Approve the config",
  };
  return (
    <div className="pb-wizard">
      <header className="toolbar pb-wizard-bar">
        <button type="button" className="toolbar-button" onClick={onCancel}>
          Cancel
        </button>
        <p className="toolbar-subtitle muted">
          Add project · Step {w.index + 1} of {w.order.length} · {stepTitles[w.step]}
        </p>
      </header>
      <div className="pb-progress" aria-hidden>
        {w.order.map((s, i) => (
          <span key={s} className={i <= w.index ? "is-done" : ""} />
        ))}
      </div>
      <section className="pb-page">
        <h1 className="pb-page-title">{titles[w.step]}</h1>
        {w.step === "folder" && <FolderPicker w={w} />}
        {w.step === "answers" && <AnswersForm w={w} />}
        {w.step === "ssh" && (
          <>
            <AliasPicker w={w} />
            {w.aliasMode === "new" && (
              <>
                <h2 className="pb-sub">Describe the host</h2>
                <HostFields w={w} />
                <h2 className="pb-sub">Add this block to ~/.ssh/config</h2>
                <HostBlockBox w={w} />
              </>
            )}
          </>
        )}
        {w.step === "check" && <CheckB w={w} />}
        {w.step === "approval" && (
          <>
            <Warnings w={w} />
            <div className="pb-approval-check">
              <strong>Connection check</strong>
              <span className="muted">
                {w.check.passed ? "All six lines passed" : w.check.lines ? `${w.check.lines.filter((l) => l.state === "pass").length} of 6 passed` : "Not run"}
              </span>
              <button type="button" className="link" onClick={() => w.setStep("check")}>
                See lines
              </button>
            </div>
            <pre className="ps-pre ps-toml">{w.toml}</pre>
          </>
        )}
      </section>
      <footer className="pb-wizard-foot">
        <button type="button" className="button" disabled={w.index === 0} onClick={w.back}>
          Back
        </button>
        <span className="pb-spacer" />
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
    </div>
  );
}

function CheckB({ w }: { w: Wizard }) {
  const { lines, run, done } = w.check;
  const hard = lines?.find((l) => l.state === "fail" && l.hardStop);
  return (
    <div className="ps-stack">
      <p className="muted ps-p">
        Connects as <span className="mono">{w.alias}</span> and only looks. Nothing on the Environment changes.
      </p>
      {hard && (
        <div className="pb-stop" role="alert">
          <strong>Host key changed. Shipslip stops here.</strong>
          <p className="ps-p">{hard.reason}</p>
        </div>
      )}
      {lines && (
        <ul className="pb-checklist">
          {lines.map((l) => (
            <li key={l.label} className={`is-${l.state}`}>
              <CheckIcon line={l} />
              <div>
                <div className="pb-check-label">{l.label}</div>
                {l.state !== "waiting" && l.state !== "running" && !l.hardStop && (
                  <div className={`ps-small ${l.state === "fail" ? "tone-attention" : "muted"}`}>{l.reason}</div>
                )}
                {l.state === "fail" && l.command && (
                  <div className="pb-command">
                    <span className="ps-small muted">Run in Terminal:</span> <CommandLine command={l.command} />
                  </div>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
      <div>
        <button type="button" className="button" onClick={run}>
          {lines ? (done ? "Check again" : "Checking…") : "Check connection"}
        </button>
      </div>
    </div>
  );
}
