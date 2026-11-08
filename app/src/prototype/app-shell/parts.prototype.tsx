// PROTOTYPE (throwaway). Form pieces every variant reuses; layout lives in the variants.

import { useEffect, useState, type ReactNode } from "react";
import { Flap } from "../../Cell";
import { Mark, StepIcon } from "../../Mark";
import {
  emptyData,
  folderChoices,
  hostBlock,
  identityFiles,
  scenario,
  sshHosts,
  type CheckLine,
  type Checkout,
  type EnvMark,
  type Wizard,
  nothingRecorded,
  useCheck,
} from "./data.prototype";

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="ps-field">
      <span className="ps-label">{label}</span>
      {children}
      {hint && <span className="ps-hint muted">{hint}</span>}
    </label>
  );
}

export function FolderPicker({ w }: { w: Wizard }) {
  return (
    <div className="ps-stack">
      <p className="muted ps-p">
        Prototype stand-in for the macOS folder panel. Pick one to see each path.
      </p>
      <div className="ps-choices" role="radiogroup" aria-label="Folder">
        {folderChoices.map((f) => (
          <label key={f.path} className={`ps-choice${w.folder?.path === f.path ? " is-on" : ""}`}>
            <input type="radio" name="folder" checked={w.folder?.path === f.path} onChange={() => w.setFolder(f)} />
            <span className="mono">{f.path.replace("/Users/you", "~")}</span>
            <span className="muted ps-small">
              {f.kind === "new" && "Git folder, no .shipslip.toml"}
              {f.kind === "existing" && "Has .shipslip.toml"}
              {f.kind === "notGit" && "Not a git folder"}
            </span>
          </label>
        ))}
      </div>
      {w.folder?.kind === "notGit" && (
        <p className="ps-refuse tone-attention">
          {w.folder.path.replace("/Users/you", "~")} is not inside a git repository. Shipslip deploys a git
          checkout, so this folder cannot be added.
        </p>
      )}
      {w.folder?.kind === "existing" && (
        <p className="muted ps-p">
          This folder already has a .shipslip.toml. Its Environments are read as they are; no questions.
        </p>
      )}
    </div>
  );
}

export function AnswersForm({ w }: { w: Wizard }) {
  const a = w.answers;
  return (
    <div className="ps-form">
      <Field label="Environment name" hint="Letters, digits, - and _">
        <input className="ps-input" value={a.env} onChange={(e) => w.setAnswers({ env: e.target.value })} />
      </Field>
      <Field label="Remote path" hint="Absolute path of the app's git checkout">
        <input className="ps-input mono" value={a.path} onChange={(e) => w.setAnswers({ path: e.target.value })} />
      </Field>
      <Field label="Branch">
        <input className="ps-input mono" value={a.branch} onChange={(e) => w.setAnswers({ branch: e.target.value })} />
      </Field>
      <label className="ps-check">
        <input type="checkbox" checked={a.production} onChange={(e) => w.setAnswers({ production: e.target.checked })} />
        <span>
          Production <span className="muted">· deploys ask you to type the Environment name</span>
        </span>
      </label>
      <label className="ps-check">
        <input type="checkbox" checked={a.maintenance} onChange={(e) => w.setAnswers({ maintenance: e.target.checked })} />
        <span>
          Maintenance mode <span className="muted">· php artisan down before the steps</span>
        </span>
      </label>
      <Field label="Smoke URL" hint="Optional. Requested from this Mac after the steps; 2xx passes">
        <input
          className="ps-input mono"
          placeholder="https://crm.example.com/health"
          value={a.smokeUrl}
          onChange={(e) => w.setAnswers({ smokeUrl: e.target.value })}
        />
      </Field>
    </div>
  );
}

export function AliasPicker({ w }: { w: Wizard }) {
  return (
    <div className="ps-stack">
      <p className="muted ps-p">Read from ~/.ssh/config. Shipslip never writes ssh files.</p>
      <div className="ps-choices" role="radiogroup" aria-label="SSH alias">
        {sshHosts.map((h) => (
          <label key={h.alias} className={`ps-choice${w.aliasMode === "pick" && w.answers.alias === h.alias ? " is-on" : ""}`}>
            <input
              type="radio"
              name="alias"
              checked={w.aliasMode === "pick" && w.answers.alias === h.alias}
              onChange={() => {
                w.setAliasMode("pick");
                w.setAnswers({ alias: h.alias });
              }}
            />
            <span className="mono">{h.alias}</span>
            <span className="muted ps-small mono">
              {h.user}@{h.host}
            </span>
          </label>
        ))}
        <label className={`ps-choice${w.aliasMode === "new" ? " is-on" : ""}`}>
          <input
            type="radio"
            name="alias"
            checked={w.aliasMode === "new"}
            onChange={() => {
              w.setAliasMode("new");
              w.setAnswers({ alias: w.host.alias });
            }}
          />
          <span>None of these fit</span>
        </label>
      </div>
    </div>
  );
}

export function HostFields({ w }: { w: Wizard }) {
  const [more, setMore] = useState(false);
  const h = w.host;
  return (
    <div className="ps-form">
      <Field label="Alias">
        <input
          className="ps-input mono"
          value={h.alias}
          onChange={(e) => {
            w.setHost({ alias: e.target.value });
            w.setAnswers({ alias: e.target.value });
          }}
        />
      </Field>
      <Field label="HostName">
        <input className="ps-input mono" placeholder="203.0.113.7" value={h.hostName} onChange={(e) => w.setHost({ hostName: e.target.value })} />
      </Field>
      <Field label="User">
        <input className="ps-input mono" placeholder="forge" value={h.user} onChange={(e) => w.setHost({ user: e.target.value })} />
      </Field>
      <Field label="IdentityFile">
        <select className="ps-input mono" value={h.identityFile} onChange={(e) => w.setHost({ identityFile: e.target.value })}>
          {identityFiles.map((f) => (
            <option key={f}>{f}</option>
          ))}
        </select>
      </Field>
      <button type="button" className="link ps-more" aria-expanded={more} onClick={() => setMore(!more)}>
        {more ? "Less" : "More"}
      </button>
      {more && (
        <Field label="Port">
          <input className="ps-input mono" value={h.port} onChange={(e) => w.setHost({ port: e.target.value })} />
        </Field>
      )}
    </div>
  );
}

export function HostBlockBox({ w }: { w: Wizard }) {
  const [copied, setCopied] = useState(false);
  const text = hostBlock(w.host);
  return (
    <div className="ps-hostblock">
      <pre className="ps-pre">{text}</pre>
      <div className="ps-row">
        <button
          type="button"
          className="button"
          onClick={() => {
            void navigator.clipboard?.writeText(text);
            setCopied(true);
          }}
        >
          {copied ? "Copied" : "Copy"}
        </button>
        <button type="button" className="button">
          Open ~/.ssh/config
        </button>
        <button type="button" className="button" onClick={w.checkHostAgain}>
          Check again
        </button>
      </div>
      {w.hostFound === "missing" && (
        <p className="ps-small tone-attention">
          ssh -G {w.host.alias} does not show HostName {w.host.hostName || "…"} yet. Paste the block, save the file,
          then check again.
        </p>
      )}
      {w.hostFound === "found" && <p className="ps-small">Found: ssh -G {w.host.alias} resolves to {w.host.hostName || "203.0.113.7"}.</p>}
    </div>
  );
}

export function CheckIcon({ line }: { line: CheckLine }) {
  if (line.state === "pass") return <StepIcon kind="ok" label="passed" />;
  if (line.state === "fail") return <StepIcon kind="unknown" label="did not pass" />;
  if (line.state === "running") return <StepIcon kind="running" />;
  return <StepIcon kind="notRun" label={line.state === "notRun" ? "not run" : "waiting"} />;
}

export function CommandLine({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="ps-command">
      <code className="mono">{command}</code>
      <button
        type="button"
        className="link"
        onClick={() => {
          void navigator.clipboard?.writeText(command);
          setCopied(true);
        }}
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </span>
  );
}

export function Warnings({ w }: { w: Wizard }) {
  if (!w.folder?.warnings.length) return null;
  return (
    <div className="ps-warn">
      <strong className="ps-warn-title">Read before approving</strong>
      {w.folder.warnings.map((t) => (
        <p key={t} className="ps-p">
          {t}
        </p>
      ))}
      <label className="ps-check">
        <input type="checkbox" checked={w.warningsRead} onChange={(e) => w.setWarningsRead(e.target.checked)} />
        <span>I read this. The recipe has no frontend steps until I add them.</span>
      </label>
    </div>
  );
}

/** One small mark per Environment, in env order. */
export function EnvMarks({ c }: { c: Checkout }) {
  return (
    <span className="ps-marks">
      {c.envs.map((e: EnvMark) => (
        <span key={e.env} title={`${e.env}: ${e.status.word}`} className={`ps-markbox ps-tone-${e.status.tone}`}>
          <Mark kind={e.mark} size={8} />
        </span>
      ))}
    </span>
  );
}

/* ---------- Floating variant switcher (prototype only) ---------- */

export function Switcher({ variants, current }: { variants: { key: string; name: string }[]; current: string }) {
  const i = Math.max(0, variants.findIndex((v) => v.key === current));
  const go = (key: string, extra: Record<string, string | null> = {}) => {
    const p = new URLSearchParams(window.location.search);
    p.set("variant", key);
    for (const [k, v] of Object.entries(extra)) {
      if (v === null) p.delete(k);
      else p.set(k, v);
    }
    window.location.search = p.toString();
  };
  const set = (k: string, v: string | null) => go(current, { [k]: v });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (t.closest("input, textarea, select, [contenteditable]")) return;
      if (e.key === "ArrowLeft") go(variants[(i - 1 + variants.length) % variants.length].key);
      if (e.key === "ArrowRight") go(variants[(i + 1) % variants.length].key);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  if (!import.meta.env.DEV) return null;
  return (
    <div className="ps-switcher" aria-label="Prototype switcher">
      <button type="button" onClick={() => go(variants[(i - 1 + variants.length) % variants.length].key)}>
        ←
      </button>
      <span className="ps-switcher-label">
        {variants[i].key} · {variants[i].name}
      </span>
      <button type="button" onClick={() => go(variants[(i + 1) % variants.length].key)}>
        →
      </button>
      <span className="ps-switcher-sep" />
      <select value={emptyData ? "empty" : "full"} onChange={(e) => set("data", e.target.value === "empty" ? "empty" : null)}>
        <option value="full">Data: some checkouts</option>
        <option value="empty">Data: first launch</option>
      </select>
      <select value={scenario} onChange={(e) => set("check", e.target.value === "pass" ? null : e.target.value)}>
        <option value="pass">Check: all pass</option>
        <option value="hostkey">Check: unknown host key</option>
        <option value="changed">Check: changed host key</option>
        <option value="branch">Check: wrong branch</option>
      </select>
    </div>
  );
}

/** Runs going now and Needs attention, as the sidebar has them today. */
export function TodaySections({ list, onOpen }: { list: Checkout[]; onOpen: (id: string) => void }) {
  const running = list.flatMap((c) => c.envs.filter((e) => e.mark === "live").map((e) => ({ c, e })));
  const attention = list.flatMap((c) =>
    c.envs
      .filter((e) => e.mark !== "live" && (e.status.tone === "bad" || e.status.tone === "attention"))
      .map((e) => ({ c, e })),
  );
  return (
    <>
      <section className="side-section">
        <h2 className="side-title">Deploys running</h2>
        {running.length === 0 ? (
          <p className="side-empty muted">None started here</p>
        ) : (
          <ul className="side-list">
            {running.map(({ c, e }) => (
              <li key={c.id + e.env}>
                <button type="button" className="side-item" onClick={() => onOpen(c.id)}>
                  <span className="side-line">
                    <span className="side-mark"><Mark kind="live" /></span>
                    <span className="side-name">{c.project}</span>
                  </span>
                  <span className="side-why"><span className="side-why-text muted">{e.env} · <span className="side-meta">1m 12s</span></span></span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      {list.length > 0 && (
        <section className="side-section">
          <h2 className="side-title">
            Needs attention{attention.length > 0 && <span className="side-count">{attention.length}</span>}
          </h2>
          <ul className="side-list">
            {attention.map(({ c, e }) => (
              <li key={c.id + e.env}>
                <button type="button" className="side-item" onClick={() => onOpen(c.id)}>
                  <span className="side-line">
                    <span className="side-mark"><Mark kind={e.mark} /></span>
                    <span className="side-name">{c.project}</span>
                  </span>
                  <span className="side-why">
                    <Flap status={e.status} />{" "}
                    <span className="side-why-text muted">{e.env}{c.folder && <span className="side-folder"> · in {c.folder}</span>}</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}

/** The checkout's menu: Check connection, Open folder, Remove (only when nothing is recorded). */
export function CheckoutMenu({ c, onCheck, onRemove }: { c: Checkout; onCheck: () => void; onRemove: () => void }) {
  const [open, setOpen] = useState(false);
  return (
    <span className="ps-menu">
      <button type="button" className="toolbar-button" aria-expanded={open} onClick={() => setOpen(!open)}>
        ⋯
      </button>
      {open && (
        <span className="ps-menu-list" role="menu">
          <button type="button" role="menuitem" onClick={() => { setOpen(false); onCheck(); }}>Check connection</button>
          <button type="button" role="menuitem" onClick={() => setOpen(false)}>Open folder in Finder</button>
          <button type="button" role="menuitem" onClick={() => setOpen(false)}>Open .shipslip.toml</button>
          {nothingRecorded(c) && (
            <button type="button" role="menuitem" onClick={() => { setOpen(false); onRemove(); }}>Remove from sidebar</button>
          )}
        </span>
      )}
    </span>
  );
}

/** A connection check run from the checkout's menu, outside the wizard. */
export function MenuCheck({ c, onClose }: { c: Checkout; onClose: () => void }) {
  const alias = c.envs[0]?.cell?.target?.split(":")[0] ?? `${c.project}-prod`;
  const check = useCheck(alias, `/var/www/${c.project}`, "main");
  useEffect(() => {
    check.run();
  }, []);
  return (
    <section className="ps-menucheck" aria-label="Connection check">
      <div className="ps-row ps-between">
        <strong>Connection check · {c.envs[0]?.env ?? "production"}</strong>
        <button type="button" className="link" onClick={onClose}>Close</button>
      </div>
      <ul className="ps-checklist">
        {check.lines?.map((l) => (
          <li key={l.label} className={`ps-checkline is-${l.state}`}>
            <CheckIcon line={l} />
            <span>{l.label}</span>
            <span className="muted ps-small">{l.state === "waiting" ? "" : l.reason}</span>
          </li>
        ))}
      </ul>
      <p className="muted ps-small">Read only. Checked now from this Mac; nothing on the Environment changed.</p>
    </section>
  );
}
