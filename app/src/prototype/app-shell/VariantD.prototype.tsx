// PROTOTYPE D · the chosen mix (see DECISIONS.md): A's sidebar lines and
// checkout pane, All projects as the top item and start screen, C's sheet
// wizard, A's check rows with B's boxed hard stop.

import { useState } from "react";
import { ProblemsStrip } from "../../ProblemsStrip";
import { fullMap, useShell, type Wizard } from "./data.prototype";
import { CheckIcon, CommandLine, EnvMarks, TodaySections } from "./parts.prototype";
import { AllProjects, CheckoutA } from "./VariantA.prototype";
import { WizardC } from "./VariantC.prototype";

export function VariantD() {
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
      <nav className="sidebar pa-sidebar" aria-label="Sidebar">
        <div className="titlebar-drag" />
        <div className="pd-top">
          <button type="button" className="button pa-add" onClick={() => setSheet({})}>
            Add project
          </button>
          <button
            type="button"
            className={`side-item pa-row pd-all${view.kind === "all" ? " is-on" : ""}`}
            aria-current={view.kind === "all" ? "page" : undefined}
            onClick={() => setView({ kind: "all" })}
          >
            <span className="side-line">
              <span className="side-name">All projects</span>
              {list.length > 0 && <span className="muted ps-small">{list.length}</span>}
            </span>
          </button>
        </div>
        <TodaySections list={list} onOpen={open} />
        {list.length > 0 && <ProblemsStrip problems={fullMap.problems} />}
        <section className="side-section">
          <h2 className="side-title">Projects</h2>
          {list.length === 0 ? (
            <p className="side-empty muted">No checkouts yet</p>
          ) : (
            <ul className="side-list">
              {list.map((c) => (
                <li key={c.id}>
                  <button
                    type="button"
                    className={`side-item pa-row${selected?.id === c.id ? " is-on" : ""}`}
                    aria-current={selected?.id === c.id ? "page" : undefined}
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
          )}
        </section>
      </nav>
      <main className="main">
        {view.kind === "all" && <AllProjects list={list} onOpen={open} onAdd={() => setSheet({})} />}
        {view.kind === "checkout" && selected && (
          <CheckoutA key={selected.id} c={selected} onRemove={() => shell.remove(selected.id)} />
        )}
      </main>
      {sheet && (
        <div className="pc-scrim" role="presentation">
          <WizardC
            key={sheet.resume ?? "new"}
            resume={sheet.resume}
            check={(w) => <CheckD w={w} />}
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

function CheckD({ w }: { w: Wizard }) {
  const { lines, run, done } = w.check;
  const hard = lines?.find((l) => l.state === "fail" && l.hardStop);
  return (
    <div className="ps-stack">
      <div className="ps-row ps-between">
        <p className="muted ps-p">
          Read only. Connects as <span className="mono">{w.alias}</span> and looks; nothing on the Environment changes.
        </p>
        <button type="button" className="button" onClick={run} disabled={Boolean(lines) && !done}>
          {lines ? "Check again" : "Check connection"}
        </button>
      </div>
      {hard && (
        <div className="pb-stop" role="alert">
          <strong>Host key changed. Shipslip stops here.</strong>
          <p className="ps-p">{hard.reason}</p>
        </div>
      )}
      {lines && (
        <ul className="ps-checklist">
          {lines.map((l) => (
            <li key={l.label} className={`ps-checkline is-${l.state}`}>
              <CheckIcon line={l} />
              <span>{l.label}</span>
              <span className={`ps-small ${l.state === "fail" ? "tone-attention" : "muted"}`}>
                {l.state === "waiting" || l.state === "running" ? "" : l.hardStop ? "Host key changed" : l.reason}
              </span>
              {l.state === "fail" && l.command && (
                <span className="pd-cmd">
                  <CommandLine command={l.command} />
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
