import { useState, type ReactNode } from "react";
import { CommandBox } from "./CommandBox";
import type { Context, PaneView } from "./slipTarget";
import { isTime, stamp, utcStamp } from "./time";
import type {
  EnvCellView,
  FlagView,
  GroupView,
  Guidance,
  ReceiptView,
  StepStatus,
  StepView,
  Tone,
  WatchView,
} from "./types";
import type { Opened } from "./useReceipt";

const toneClass: Record<Tone, string> = { good: "tone-good", bad: "tone-bad", neutral: "" };

const COMMITS_SHOWN = 5;

const stepGlyph: Record<StepStatus, { glyph: string; className: string }> = {
  ok: { glyph: "✓", className: "tone-good" },
  failed: { glyph: "✗", className: "tone-bad" },
  unknown: { glyph: "◌", className: "tone-bad" },
  running: { glyph: "◌", className: "muted" },
  notRun: { glyph: "–", className: "muted" },
  skipped: { glyph: "↷", className: "muted" },
};

interface Props {
  opened: Opened;
  cell: EnvCellView;
  /** The row's app label and the column's env, for slips without a receipt. */
  label: string;
  env: string;
  context: Context | null;
  onView: (view: PaneView) => void;
}

/** The receipt slip: header and outcome pinned on top, guidance pinned at the
 * bottom, the rest scrolling between the perforated edges. */
export function Slip({ opened, cell, label, env, context, onView }: Props) {
  const contextLine = context && (
    <p className="slip-context muted">
      {context.text}
      {context.links.map((link) => (
        <span key={link.view}>
          {" · "}
          <button type="button" className="link" aria-label={link.label} onClick={() => onView(link.view)}>
            {link.text}
          </button>
        </span>
      ))}
    </p>
  );
  const name = `${label} · ${env}`;
  switch (opened.kind) {
    case "receipt":
      return <ReceiptSlip receipt={opened} contextLine={contextLine} />;
    case "none":
      return (
        <Paper>
          <div className="slip-head">
            <h2 className="slip-title" tabIndex={-1}>{name}</h2>
            <p>
              No receipt for {name} on this Mac.
              {cell.evidence.length > 0 && ` ${sentence(cell.evidence.join(", "))}`}
            </p>
          </div>
        </Paper>
      );
    case "missing":
    case "notFound":
      return (
        <Paper>
          <div className="slip-head">
            {contextLine}
            <h2 className="slip-title" tabIndex={-1}>{name}</h2>
            <p className="tone-bad">
              The receipt file is no longer there. Refresh ⌘R to re-read the map.
            </p>
          </div>
        </Paper>
      );
    case "newerVersion":
      return (
        <Paper>
          <div className="slip-head">
            {contextLine}
            <h2 className="slip-title" tabIndex={-1}>{name}</h2>
            <p>Written by a newer Shipslip. Update slip to read it.</p>
          </div>
        </Paper>
      );
    case "unreadable":
    case "failed":
      return (
        <Paper>
          <div className="slip-head">
            {contextLine}
            <h2 className="slip-title" tabIndex={-1}>{name}</h2>
            <p className="tone-bad">Can't read this receipt.</p>
            <p className="mono slip-detail">{opened.message}</p>
          </div>
        </Paper>
      );
  }
}

function sentence(text: string): string {
  const upper = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?]$/.test(upper) ? upper : `${upper}.`;
}

function Paper({ children }: { children: ReactNode }) {
  return (
    <article className="slip">
      <Edge />
      <div className="slip-paper">{children}</div>
      <Edge bottom />
    </article>
  );
}

/** The torn-paper edge, drawn in the slip's own colour. */
function Edge({ bottom = false }: { bottom?: boolean }) {
  return <div className={`slip-edge${bottom ? " slip-edge-bottom" : ""}`} aria-hidden />;
}

function ReceiptSlip({ receipt, contextLine }: { receipt: ReceiptView; contextLine: ReactNode }) {
  const guidance = receipt.nextSteps;
  const hasGuidance = guidance.lines.length > 0 || guidance.copyWithheld;
  return (
    <Paper>
      <div className="slip-head">
        {contextLine}
        <div className="slip-id">
          <span className="caps mono muted">Receipt · {receipt.runId}</span>
          {isTime(receipt.startedAtMs) ? (
            <time
              className="mono muted slip-time"
              dateTime={new Date(receipt.startedAtMs).toISOString()}
              title={utcStamp(receipt.startedAtMs)}
            >
              {stamp(receipt.startedAtMs)}
            </time>
          ) : (
            <span className="mono muted slip-time">time not known</span>
          )}
        </div>
        <h2 className="slip-title" tabIndex={-1}>
          {receipt.project} · {receipt.env}
        </h2>
        <div className="slip-shas mono">
          <span className="muted slip-from" title={receipt.fromSha}>
            {receipt.fromSha.slice(0, 7)}
          </span>
          <span className="muted" aria-label="to">
            →
          </span>
          <span className="slip-to" title={receipt.targetSha}>
            {receipt.targetSha.slice(0, 7)}
          </span>
        </div>
      </div>
      <div className="slip-body">
        <Facts receipt={receipt} />
        <Steps steps={receipt.steps} />
        {receipt.commits.length > 0 && <Commits commits={receipt.commits} />}
        {receipt.watch && <LogWatch watch={receipt.watch} />}
        {receipt.notes.length > 0 && (
          <section className="slip-section">
            {receipt.notes.map((note, index) => (
              <p className="muted" key={index}>
                {note}
              </p>
            ))}
          </section>
        )}
      </div>
      {hasGuidance && <NextSteps guidance={guidance} />}
    </Paper>
  );
}

function Fact({ name, children }: { name: string; children: ReactNode }) {
  return (
    <div className="fact mono">
      <dt className="muted">{name}</dt>
      <dd>{children}</dd>
    </div>
  );
}

function Facts({ receipt }: { receipt: ReceiptView }) {
  const head = receipt.serverHead;
  return (
    <section className="slip-section">
      <dl className="facts">
        <Fact name="Outcome">
          <span className={`slip-outcome ${toneClass[receipt.outcome.tone]}`}>
            {receipt.outcome.text}
          </span>
        </Fact>
        {receipt.duration && <Fact name="Duration">{receipt.duration}</Fact>}
        {receipt.startedBy && <Fact name="By">{receipt.startedBy}</Fact>}
        <Fact name="Target">{receipt.target}</Fact>
        {receipt.smoke && <Fact name="Smoke">{receipt.smoke}</Fact>}
        {head && (
          <Fact name="Server after">
            {head.sha ? (
              <span title={head.sha}>{head.sha.slice(0, 7)}</span>
            ) : (
              <span className="muted">not read</span>
            )}
            {head.dirty && " · uncommitted changes"}
          </Fact>
        )}
      </dl>
      {receipt.outcome.tone !== "good" && receipt.summary && (
        <p className="slip-summary">{receipt.summary}</p>
      )}
      {receipt.flags.map((flag) => (
        <Flag key={flag.text} flag={flag} />
      ))}
    </section>
  );
}

function Flag({ flag }: { flag: FlagView }) {
  return (
    <p className="slip-flag">
      {flag.newErrors && <span className="chip">NEW</span>}
      <span className={`sentence ${flag.bad ? "tone-bad" : "muted"}`}>{flag.text}</span>
    </p>
  );
}

function Steps({ steps }: { steps: StepView[] }) {
  return (
    <section className="slip-section">
      <ol className="steps mono">
        {steps.map((step) => {
          const { glyph, className } = stepGlyph[step.status];
          const words = step.label.split(" ").slice(1).join(" ");
          const loud = step.status === "failed" || step.status === "unknown";
          return (
            <li key={step.index} className={`step${step.status === "notRun" ? " muted" : ""}`}>
              <span className={`step-glyph ${className}`} aria-hidden>
                {glyph}
              </span>
              <span className="step-index muted">{step.index}</span>
              <div className="step-body">
                <span className={loud ? "tone-bad step-loud" : ""}>{step.command}</span>
                {step.status !== "ok" && (
                  <span className="muted">
                    {" · "}
                    {words}
                    {step.exitCode !== null && step.status === "failed" && ` · exit ${step.exitCode}`}
                  </span>
                )}
                {step.output.length > 0 && (
                  <pre className="step-output muted">{step.output.join("\n")}</pre>
                )}
              </div>
            </li>
          );
        })}
      </ol>
    </section>
  );
}

function Commits({ commits }: { commits: string[] }) {
  const more = commits.length - COMMITS_SHOWN;
  return (
    <section className="slip-section">
      <h3 className="caps mono muted">Commits</h3>
      <ul className="slip-list mono">
        {commits.slice(0, COMMITS_SHOWN).map((commit) => (
          <li key={commit}>{commit}</li>
        ))}
      </ul>
      {more > 0 && <p className="muted">and {more} more</p>}
    </section>
  );
}

function LogWatch({ watch }: { watch: WatchView }) {
  const newGroups = watch.groups.length + watch.moreGroups;
  return (
    <section className="slip-section">
      <h3 className="slip-section-title">
        <span className="caps mono muted">Log watch</span>
        {newGroups > 0 && <span className="chip">NEW {newGroups}</span>}
      </h3>
      <p className="mono muted slip-watch-status">
        {watch.status} · {watch.duration} · {watch.linesRead} lines · {watch.logPath}
      </p>
      {watch.groups.map((group, index) => (
        <ErrorGroup key={index} group={group} />
      ))}
      {watch.moreGroups > 0 && (
        <p className="muted">
          and {watch.moreGroups} more {watch.moreGroups === 1 ? "group" : "groups"}
        </p>
      )}
      {watch.warnings.map((warning, index) => (
        <p className="muted" key={index}>
          {warning}
        </p>
      ))}
    </section>
  );
}

function ErrorGroup({ group }: { group: GroupView }) {
  const [open, setOpen] = useState(false);
  const top = group.variants[0];
  const where = top?.location ?? group.file;
  const more = group.variants.length - 1 + group.moreVariants;
  return (
    <div className="error-group">
      <button
        type="button"
        className={`error-text mono${open ? "" : " error-clamped"}`}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {top ? `${group.exception}: ${top.message}` : group.exception}
      </button>
      <p className="mono muted error-meta">
        ×{group.count}
        {where && ` · ${where}`}
        {more > 0 && ` · ${more} more ${more === 1 ? "variant" : "variants"}`}
      </p>
    </div>
  );
}

function NextSteps({ guidance }: { guidance: Guidance }) {
  return (
    <section className="slip-guidance">
      {guidance.lines.map((line, index) => {
        const commands = line.flatMap((span) => (span.kind === "command" ? [span] : []));
        return (
          <div className="guidance-line" key={index}>
            <p>
              {line.map((span, i) =>
                span.kind === "text" ? span.text : <code key={i}>`{span.command}`</code>,
              )}
            </p>
            {commands.map((span) => (
              <CommandBox key={span.copyText} {...span} />
            ))}
          </div>
        );
      })}
      {guidance.copyWithheld && (
        <p className="muted guidance-note">Env name has shell characters; copy by hand</p>
      )}
    </section>
  );
}
